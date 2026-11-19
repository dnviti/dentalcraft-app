//! Bridge mode: `dentalcraft --control <port>` lets `dentalcraft-cli mcp
//! --connect <port>` (or any client) drive the running app.
//!
//! # Protocol (`dentalcraft-control/1`)
//!
//! TCP on `127.0.0.1:<port>` only (port 0 picks a free one; the app prints
//! `control: listening on 127.0.0.1:<port>` on stdout). UTF-8, one JSON-RPC
//! 2.0 message per line; every request gets one answer line, in order:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"execute","params":{"id":"aligners.demo","params":{"arch":"upper"}}}
//! <- {"jsonrpc":"2.0","id":1,"result":{...}}
//! <- {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"unknown command x"}}
//! ```
//!
//! Methods (they mirror `dc_mcp::Host`):
//!
//! | method | params | result |
//! |---|---|---|
//! | `hello` | - | `{app, version, protocol}` |
//! | `list` | - | `[{id, source, label}]` |
//! | `describe` | `{id}` | `{id, source, label_key, label, doc_key, doc, params_schema}` |
//! | `execute` | `{id, params}` | the command's output |
//! | `batch` | `{commands: [{id, params}]}` | array of outputs (stops at the first failure) |
//! | `inspect` | `{path?}` | case summary, or the part at `path` (dotted or JSON pointer) |
//! | `undo`, `redo` | - | `{done}` |
//! | `open` | `{path}` | project summary |
//! | `save` | `{path?}` | `{path, bytes}` |
//! | `render`, `screenshot` | render parameters (`dc_raster::RenderRequest`), plus `model` (`scan` / `teeth`, default what the viewport shows) and `stage` (default the stage shown) | `{png: <base64>, width, height}` |
//!
//! Errors of an operation use code `-32000`; malformed lines `-32700`,
//! invalid requests `-32600`, unknown methods `-32601`, bad parameters
//! `-32602`.
//!
//! `list` also has the app's commands `app.update.check` and
//! `app.update.apply` (source `app`, see `dc_update::commands`): they act
//! on the installation, through the update dialog, not on the case. A check
//! asked by an agent does not open the dialog; an apply shows the download
//! in it, and with `restart: true` the app then saves the project, closes
//! and opens the new version (the connection ends).
//!
//! `list` also has the anonymous diagnostics commands
//! `app.diagnostics.status`, `.set`, `.preview` and `.send` (source `app`,
//! see `dc_diagnostics::commands`): `set` is the account menu's checkbox
//! (the first-start question is answered), `send` sends the waiting
//! reports now and answers when the server did.
//!
//! `list` also has `app.menu.select` (source `app`, parameter `id`: a menu
//! entry, or the command its inspector runs): it selects that entry of the
//! open project as a click does, so the inspector shows; it never runs
//! anything (the menu law). Then `execute` runs the function, as its
//! inspector's button does. `app.window.screenshot` (parameter `path`)
//! writes the window as the user sees it to a PNG file (a GPU read-back of
//! the next frame; it does not wait for the jobs).
//!
//! `list` also has the scanner's commands `scan.*` and the turntable's
//! `turntable.*` (`dc_scanner::control`): they run on the scan controller
//! of the open workspace, the one its scan window shows, in the same queue
//! as the window's buttons; an agent's command that acts on the scanner or
//! the turntable opens that window. `scan.import` writes the surface to a
//! temporary file and imports it with the workspace's import (one undoable
//! change); it answers with the import report.
//!
//! Requests travel through a channel to the UI thread, which runs them in
//! [`ControlServer::pump`] at the start of a frame against the open
//! workspace: case commands go through the workspace's command path
//! (background job, then its undo stack), so the user sees them and can
//! undo them in the window. A request that starts a job is answered when
//! the job ends; the next request waits for it. Without a workspace, a case
//! command opens one on a new empty case (as the headless session).
//!
//! `screenshot` is a CPU render (`dc-raster`) of the current case, not a
//! read-back of the GPU viewport.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use base64::Engine;
use dc_case::CaseState;
use dc_command::{Command, CommandError, JobCtx};
use dc_module_api::{CommandOutput, CommandSpec};
use dc_raster::RenderRequest;
use dc_scene::SceneEntry;
use dc_session::builtins;
use dc_workspace::{ScanTicket, Workspace};
use egui::Ui;
use serde_json::{Map, Value, json};

use crate::DentalCraftApp;

/// Protocol name answered by `hello`.
pub const PROTOCOL: &str = "dentalcraft-control/1";

/// JSON-RPC error codes.
mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const HOST_ERROR: i64 = -32000;
}

/// A failed request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    /// JSON-RPC error code.
    pub code: i64,
    /// What went wrong.
    pub message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn host(message: impl Into<String>) -> Self {
        Self::new(codes::HOST_ERROR, message)
    }
}

type RpcResult = Result<Value, RpcError>;

/// A request on its way to the UI thread.
struct Request {
    method: String,
    params: Value,
    reply: mpsc::Sender<RpcResult>,
}

/// Polled every frame until it yields the answer.
type Wait = Box<dyn FnMut(&mut Bridge) -> Option<RpcResult>>;

/// What [`Bridge::handle`] does with a request.
enum Step {
    /// Answered at once.
    Done(RpcResult),
    /// Answered when the closure returns `Some`.
    Wait(Wait),
}

struct Active {
    reply: mpsc::Sender<RpcResult>,
    wait: Wait,
}

type Waker = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;

/// The TCP listener and the UI-thread side of the request channel.
pub struct ControlServer {
    addr: SocketAddr,
    rx: mpsc::Receiver<Request>,
    waker: Waker,
    waker_set: bool,
    active: Option<Active>,
}

impl ControlServer {
    /// Listens on `127.0.0.1:port` (0 = any free port) on a background
    /// thread; each connection gets its own thread.
    pub fn bind(port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let addr = listener.local_addr()?;
        let (tx, rx) = mpsc::channel();
        let waker: Waker = Arc::new(Mutex::new(None));
        let w = Arc::clone(&waker);
        std::thread::Builder::new()
            .name("dc-control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (tx, w) = (tx.clone(), Arc::clone(&w));
                    let _ = std::thread::Builder::new()
                        .name("dc-control-conn".into())
                        .spawn(move || serve_connection(stream, &tx, &w));
                }
            })?;
        Ok(Self {
            addr,
            rx,
            waker,
            waker_set: false,
            active: None,
        })
    }

    /// The address listened on.
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// Called from the connection threads when a request arrives (the app
    /// asks egui for a frame).
    pub fn set_waker(&mut self, f: impl Fn() + Send + 'static) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(Box::new(f));
        self.waker_set = true;
    }

    /// Runs the pending requests on the UI thread; returns `true` while a
    /// request waits for a background job (keep the frames coming).
    pub fn pump(&mut self, bridge: &mut Bridge) -> bool {
        loop {
            if let Some(active) = &mut self.active {
                match (active.wait)(bridge) {
                    Some(r) => {
                        let _ = active.reply.send(r);
                        self.active = None;
                    }
                    None => return true,
                }
            }
            let Ok(req) = self.rx.try_recv() else {
                return false;
            };
            match bridge.handle(&req.method, &req.params) {
                Step::Done(r) => {
                    let _ = req.reply.send(r);
                }
                Step::Wait(wait) => {
                    self.active = Some(Active {
                        reply: req.reply,
                        wait,
                    })
                }
            }
        }
    }
}

fn response(id: Value, r: RpcResult) -> Value {
    match r {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(e) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": e.code, "message": e.message },
        }),
    }
}

/// Reads request lines from `stream`, forwards them to the UI thread and
/// writes the answers.
fn serve_connection(stream: TcpStream, tx: &mpsc::Sender<Request>, waker: &Waker) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut writer = stream;
    for line in BufReader::new(read_half).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Err(e) => response(
                Value::Null,
                Err(RpcError::new(
                    codes::PARSE_ERROR,
                    format!("parse error: {e}"),
                )),
            ),
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                match (
                    msg.get("jsonrpc").and_then(Value::as_str),
                    msg.get("method").and_then(Value::as_str),
                ) {
                    (Some("2.0"), Some(method)) => {
                        let (reply, rx) = mpsc::channel();
                        let req = Request {
                            method: method.to_owned(),
                            params: msg.get("params").cloned().unwrap_or(Value::Null),
                            reply,
                        };
                        if tx.send(req).is_err() {
                            return;
                        }
                        if let Some(w) = waker
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .as_ref()
                        {
                            w();
                        }
                        let Ok(r) = rx.recv() else { return };
                        response(id, r)
                    }
                    _ => response(
                        id,
                        Err(RpcError::new(
                            codes::INVALID_REQUEST,
                            "expected a JSON-RPC 2.0 request with a method",
                        )),
                    ),
                }
            }
        };
        let mut text = answer.to_string();
        text.push('\n');
        if writer.write_all(text.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

/// `app.menu.select`: selects an entry of the menu of the open project, as a
/// click on it does: its inspector shows, nothing runs (the menu law).
pub const MENU_SELECT: &str = "app.menu.select";

fn menu_select_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "MenuSelectParams",
        "type": "object",
        "properties": {
            "id": {
                "description": "The menu entry: its id, or the id of the command its inspector runs (for example aligners.teeth.find).",
                "type": "string"
            }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

/// `app.window.screenshot`: the window as the user sees it, as a PNG file.
pub const WINDOW_SCREENSHOT: &str = "app.window.screenshot";

fn window_screenshot_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "WindowScreenshotParams",
        "type": "object",
        "properties": {
            "path": {
                "description": "The PNG file to write (absolute path).",
                "type": "string"
            }
        },
        "required": ["path"],
        "additionalProperties": false
    })
}

fn run_by_the_app(_: &CaseState, _: &Value, _: &JobCtx) -> Result<CommandOutput, CommandError> {
    Err(CommandError::Invalid(
        "app command: the app bridge runs it".into(),
    ))
}

/// The commands of the app window itself (source `app`).
fn window_specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: MENU_SELECT,
            label_key: "app-cmd-menu-select",
            doc_key: "app-cmd-menu-select-doc",
            params_schema: menu_select_params,
            run: run_by_the_app,
        },
        CommandSpec {
            id: WINDOW_SCREENSHOT,
            label_key: "app-cmd-window-screenshot",
            doc_key: "app-cmd-window-screenshot-doc",
            params_schema: window_screenshot_params,
            run: run_by_the_app,
        },
    ]
}

/// A [`CommandSpec`] bound to its parameters that keeps the JSON output of
/// its run (the workspace's undo stack only keeps the state).
struct Capturing {
    spec: CommandSpec,
    params: Value,
    out: Arc<Mutex<Option<Result<Value, String>>>>,
}

impl Command for Capturing {
    fn id(&self) -> &'static str {
        self.spec.id
    }

    fn label_key(&self) -> &str {
        self.spec.label_key
    }

    fn apply(&self, state: &CaseState, ctx: &JobCtx) -> Result<CaseState, CommandError> {
        let r = self.spec.execute(state, &self.params, ctx);
        let mut out = self.out.lock().unwrap_or_else(PoisonError::into_inner);
        match r {
            Ok((next, v)) => {
                *out = Some(Ok(v));
                Ok(next)
            }
            Err(e) => {
                *out = Some(Err(e.to_string()));
                Err(e)
            }
        }
    }
}

/// Sets the patient code of a new case (part of `project.new`).
struct SetPatientCode(String);

impl Command for SetPatientCode {
    fn id(&self) -> &'static str {
        builtins::PROJECT_NEW
    }

    fn label_key(&self) -> &str {
        "session-cmd-project-new"
    }

    fn apply(&self, state: &CaseState, _: &JobCtx) -> Result<CaseState, CommandError> {
        let mut next = state.clone();
        next.case.patient_code.clone_from(&self.0);
        Ok(next)
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, RpcError> {
    params.get(key).and_then(Value::as_str).ok_or_else(|| {
        RpcError::new(
            codes::INVALID_PARAMS,
            format!("missing string parameter `{key}`"),
        )
    })
}

fn opt_str_param<'a>(params: &'a Value, key: &str) -> Result<Option<&'a str>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!("parameter `{key}` must be a string"),
        )),
    }
}

/// `true` when the workspace has no job left (finished jobs are applied).
fn idle(ws: &mut Workspace) -> bool {
    ws.wait_for_jobs(Duration::ZERO)
}

/// How long `app.window.screenshot` waits for the window's image.
const SCREENSHOT_WAIT: Duration = Duration::from_secs(5);

/// Writes `img` to `path` as PNG; answers `{path, width, height}`.
fn write_png(path: &Path, img: &egui::ColorImage) -> Result<Value, String> {
    let [w, h] = img.size;
    let rgba: Vec<u8> = img
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    let image = image::RgbaImage::from_raw(w as u32, h as u32, rgba)
        .ok_or_else(|| "the window image has a wrong size".to_owned())?;
    image
        .save_with_format(path, image::ImageFormat::Png)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(json!({ "path": path.display().to_string(), "width": w, "height": h }))
}

/// The app as the bridge sees it: the shell plus the view parameters set
/// with `view.set`.
pub struct Bridge {
    /// The application.
    pub app: DentalCraftApp,
    view: Map<String, Value>,
    /// The context of the window, once it drew a frame.
    ctx: Option<egui::Context>,
}

impl Bridge {
    /// Bridge over `app`.
    pub fn new(app: DentalCraftApp) -> Self {
        Self {
            app,
            view: Map::new(),
            ctx: None,
        }
    }

    /// `app.window.screenshot`: the window as the user sees it, read back
    /// from the GPU at the next frame, written to `path` as PNG.
    fn window_screenshot(&mut self, params: &Value) -> Step {
        let path = match str_param(params, "path") {
            Ok(p) => PathBuf::from(p),
            Err(e) => return Step::Done(Err(e)),
        };
        let Some(ctx) = self.ctx.clone() else {
            return Step::Done(Err(RpcError::host("the window has not drawn yet")));
        };
        let (tx, rx) = mpsc::channel();
        ctx.request_screenshot(move |img| {
            let _ = tx.send(img);
        });
        ctx.request_repaint();
        let deadline = std::time::Instant::now() + SCREENSHOT_WAIT;
        Step::Wait(Box::new(move |_: &mut Bridge| match rx.try_recv() {
            Ok(img) => Some(write_png(&path, &img).map_err(RpcError::host)),
            Err(mpsc::TryRecvError::Empty) if std::time::Instant::now() < deadline => None,
            Err(_) => Some(Err(RpcError::host(
                "the window gave no image (no GPU read-back)",
            ))),
        }))
    }

    /// The commands: the session built-ins, the scanner's (`scan.*`, run on
    /// the workspace's scan controller, the one its scan window shows), then
    /// the workspace module's (or the installed Aligners module's when no
    /// workspace is open), then the app's update commands
    /// (`app.update.check`, `app.update.apply`) and diagnostics commands
    /// (`app.diagnostics.*`).
    fn specs(&self) -> Vec<(&'static str, CommandSpec)> {
        let mut out: Vec<(&'static str, CommandSpec)> = builtins::specs()
            .into_iter()
            .chain(builtins::scan_specs())
            .map(|s| (dc_session::SESSION_SOURCE, s))
            .collect();
        let module = match self.app.workspace() {
            Some(ws) => Some(ws.module()),
            None => self.app.registry().get(dca_model::MODULE_ID),
        };
        if let Some(m) = module {
            out.extend(m.command_specs().into_iter().map(|s| (m.id(), s)));
        }
        out.extend(
            dc_update::commands::specs()
                .into_iter()
                .map(|s| (dc_update::commands::SOURCE, s)),
        );
        out.extend(
            dc_diagnostics::commands::specs()
                .into_iter()
                .map(|s| (dc_diagnostics::commands::SOURCE, s)),
        );
        out.extend(window_specs().into_iter().map(|s| ("app", s)));
        out
    }

    fn find(&self, id: &str) -> Result<(&'static str, CommandSpec), RpcError> {
        self.specs()
            .into_iter()
            .find(|(_, s)| s.id == id)
            .ok_or_else(|| RpcError::host(format!("unknown command {id}")))
    }

    fn workspace(&mut self) -> Result<&mut Workspace, RpcError> {
        if self.app.workspace().is_none() && !self.app.new_workspace(dca_model::MODULE_ID) {
            return Err(RpcError::host("cannot open a workspace"));
        }
        self.app
            .workspace_mut()
            .ok_or_else(|| RpcError::host("no workspace is open"))
    }

    fn handle(&mut self, method: &str, params: &Value) -> Step {
        let r = match method {
            "hello" => Ok(json!({
                "app": "DentalCraft",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": PROTOCOL,
            })),
            "list" => Ok(self.list()),
            "describe" => str_param(params, "id").and_then(|id| self.describe(id)),
            "execute" => match str_param(params, "id") {
                Ok(id) => {
                    let p = params.get("params").cloned().unwrap_or(Value::Null);
                    return self.execute(id, p);
                }
                Err(e) => Err(e),
            },
            "batch" => return self.batch(params),
            "inspect" => opt_str_param(params, "path").and_then(|p| self.inspect(p)),
            "undo" => return self.execute(builtins::EDIT_UNDO, Value::Null),
            "redo" => return self.execute(builtins::EDIT_REDO, Value::Null),
            "open" => return self.execute(builtins::PROJECT_OPEN, params.clone()),
            "save" => return self.execute(builtins::PROJECT_SAVE, params.clone()),
            "render" | "screenshot" => self.render(params),
            _ => Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("method not found: {method}"),
            )),
        };
        Step::Done(r)
    }

    fn list(&self) -> Value {
        Value::Array(
            self.specs()
                .into_iter()
                .map(|(src, s)| {
                    json!({ "id": s.id, "source": src, "label": dc_i18n::tr(s.label_key) })
                })
                .collect(),
        )
    }

    fn describe(&self, id: &str) -> RpcResult {
        let (src, s) = self.find(id)?;
        Ok(json!({
            "id": s.id,
            "source": src,
            "label_key": s.label_key,
            "label": dc_i18n::tr(s.label_key),
            "doc_key": s.doc_key,
            "doc": dc_i18n::tr(s.doc_key),
            "params_schema": s.schema(),
        }))
    }

    /// `true` when nothing runs: no project loading, no workspace job.
    fn ready(&mut self) -> bool {
        if self.app.is_opening() {
            return false;
        }
        self.app.workspace_mut().is_none_or(idle)
    }

    /// Runs command `id` once the jobs already running are done (a window
    /// screenshot at once).
    fn execute(&mut self, id: &str, params: Value) -> Step {
        let spec = match self.find(id) {
            Ok((_, s)) => s,
            Err(e) => return Step::Done(Err(e)),
        };
        if dc_scanner::control::is_scan_command(spec.id) {
            return Self::execute_scan(spec.id, params);
        }
        if spec.id == WINDOW_SCREENSHOT {
            return self.window_screenshot(&params);
        }
        let mut running: Option<Wait> = None;
        Step::Wait(Box::new(move |b: &mut Bridge| {
            if running.is_none() {
                if !b.ready() {
                    return None;
                }
                match b.start(spec, &params) {
                    Step::Done(r) => return Some(r),
                    Step::Wait(w) => running = Some(w),
                }
            }
            running.as_mut().and_then(|w| w(b))
        }))
    }

    /// Runs the scan command `id` on the workspace's scan controller once
    /// the jobs already running are done (a workspace opens if none is):
    /// the same commands, in the same queue, as the scan window's buttons,
    /// so the user sees what the agent does. `scan.import` answers with the
    /// import report once the scan is in the case.
    fn execute_scan(id: &'static str, params: Value) -> Step {
        let mut ticket: Option<ScanTicket> = None;
        Step::Wait(Box::new(move |b: &mut Bridge| {
            let t = match ticket {
                Some(t) => t,
                None => {
                    if !b.ready() {
                        return None;
                    }
                    let ws = match b.workspace() {
                        Ok(ws) => ws,
                        Err(e) => return Some(Err(e)),
                    };
                    match ws.start_scan_command(id, params.clone()) {
                        Ok(t) => *ticket.insert(t),
                        Err(e) => return Some(Err(RpcError::new(codes::INVALID_PARAMS, e))),
                    }
                }
            };
            let Some(ws) = b.app.workspace_mut() else {
                return Some(Err(RpcError::host("the workspace was closed")));
            };
            ws.scan_result(t)
                .map(|r| r.map_err(|e| RpcError::host(format!("{id}: {e}"))))
        }))
    }

    /// Waits for the workspace jobs, then answers with `then`.
    fn after_jobs(mut then: impl FnMut(&mut Workspace) -> RpcResult + 'static) -> Step {
        Step::Wait(Box::new(move |b: &mut Bridge| {
            let Some(ws) = b.app.workspace_mut() else {
                return Some(Err(RpcError::host("the workspace was closed")));
            };
            idle(ws).then(|| then(ws))
        }))
    }

    /// Starts command `spec` (nothing else is running).
    fn start(&mut self, spec: CommandSpec, params: &Value) -> Step {
        let invalid = |e: String| RpcError::new(codes::INVALID_PARAMS, format!("{}: {e}", spec.id));
        let r = match spec.id {
            builtins::PROJECT_NEW => {
                let code = match opt_str_param(params, "patient_code") {
                    Ok(c) => c.unwrap_or_default().to_owned(),
                    Err(e) => return Step::Done(Err(e)),
                };
                let discarded = self.app.workspace().is_some_and(Workspace::is_dirty);
                if !self.app.new_workspace(dca_model::MODULE_ID) {
                    return Step::Done(Err(RpcError::host("cannot open a workspace")));
                }
                let out = json!({ "patient_code": code, "discarded_unsaved_changes": discarded });
                if code.is_empty() {
                    return Step::Done(Ok(out));
                }
                if let Some(ws) = self.app.workspace_mut() {
                    ws.run_command(Box::new(SetPatientCode(code)));
                }
                return Self::after_jobs(move |_| Ok(out.clone()));
            }
            builtins::PROJECT_OPEN => {
                let path = match str_param(params, "path") {
                    Ok(p) => PathBuf::from(p),
                    Err(e) => return Step::Done(Err(e)),
                };
                if !path.is_file() {
                    return Step::Done(Err(RpcError::host(format!(
                        "{}: no such file",
                        path.display()
                    ))));
                }
                self.app.open_project(path.clone());
                return Step::Wait(Box::new(move |b: &mut Bridge| {
                    if b.app.is_opening() {
                        return None;
                    }
                    let ws = b
                        .app
                        .workspace()
                        .filter(|w| w.path() == Some(path.as_path()));
                    Some(match ws {
                        Some(ws) => {
                            let st = ws.state();
                            Ok(json!({
                                "path": path.display().to_string(),
                                "patient_code": st.case.patient_code,
                                "scans": st.case.scans.len(),
                                "modules": st.modules.keys().collect::<Vec<_>>(),
                            }))
                        }
                        None => Err(RpcError::host(format!(
                            "cannot open {} (the app shows why)",
                            path.display()
                        ))),
                    })
                }));
            }
            builtins::PROJECT_SAVE => {
                let path = match opt_str_param(params, "path") {
                    Ok(p) => p.map(PathBuf::from),
                    Err(e) => return Step::Done(Err(e)),
                };
                let ws = match self.workspace() {
                    Ok(ws) => ws,
                    Err(e) => return Step::Done(Err(e)),
                };
                let Some(target) = path.or_else(|| ws.path().map(Path::to_path_buf)) else {
                    return Step::Done(Err(RpcError::host(
                        "the project has no file yet: give a path",
                    )));
                };
                let target = dc_session::with_extension(target);
                ws.save_to(target.clone());
                return Self::after_jobs(move |ws| {
                    if ws.path() == Some(target.as_path()) && !ws.is_dirty() {
                        let bytes = std::fs::metadata(&target).map_or(0, |m| m.len());
                        Ok(json!({ "path": target.display().to_string(), "bytes": bytes }))
                    } else {
                        Err(RpcError::host(format!(
                            "cannot save {} (the app shows why)",
                            target.display()
                        )))
                    }
                });
            }
            builtins::EDIT_UNDO | builtins::EDIT_REDO => self.workspace().map(|ws| {
                let done = if spec.id == builtins::EDIT_UNDO {
                    ws.undo()
                } else {
                    ws.redo()
                };
                json!({ "done": done })
            }),
            builtins::STEPS_LIST | builtins::STEPS_REWIND => Err(RpcError::host(format!(
                "{} is not available through the app bridge yet",
                spec.id
            ))),
            builtins::VIEW_SET => match params {
                Value::Null | Value::Object(_) => {
                    for (k, v) in params.as_object().into_iter().flatten() {
                        if v.is_null() {
                            self.view.remove(k);
                        } else {
                            self.view.insert(k.clone(), v.clone());
                        }
                    }
                    Ok(Value::Object(self.view.clone()))
                }
                _ => Err(invalid("parameters: expected an object".into())),
            },
            // a click on the menu: selects, never runs
            MENU_SELECT => {
                let id = match str_param(params, "id") {
                    Ok(id) => id.to_owned(),
                    Err(e) => return Step::Done(Err(e)),
                };
                self.workspace().and_then(|ws| match ws.select_entry(&id) {
                    Some(selected) => Ok(json!({ "selected": selected })),
                    None => Err(invalid(format!("no menu entry {id}"))),
                })
            }
            // the installation, not the case: through the update dialog
            dc_update::commands::CHECK => {
                if let Err(e) = self.app.agent_update_check() {
                    return Step::Done(Err(RpcError::host(e)));
                }
                return Step::Wait(Box::new(|b: &mut Bridge| {
                    b.app
                        .agent_update_check_poll()
                        .map(|r| r.map_err(RpcError::host))
                }));
            }
            dc_update::commands::APPLY => {
                let restart = match dc_update::commands::restart_param(params) {
                    Ok(r) => r,
                    Err(e) => return Step::Done(Err(invalid(e))),
                };
                if let Err(e) = self.app.agent_update_apply(restart) {
                    return Step::Done(Err(RpcError::host(e)));
                }
                return Step::Wait(Box::new(|b: &mut Bridge| {
                    b.app
                        .agent_update_apply_poll()
                        .map(|r| r.map_err(RpcError::host))
                }));
            }
            // the installation, not the case: the consent of the account menu
            dc_diagnostics::commands::SET => {
                match dc_diagnostics::commands::enabled_param(params) {
                    Ok(on) => {
                        self.app.set_diagnostics(on);
                        Ok(self.app.diagnostics_status())
                    }
                    Err(e) => Err(invalid(e)),
                }
            }
            dc_diagnostics::commands::STATUS
            | dc_diagnostics::commands::PREVIEW
            | dc_diagnostics::commands::SEND => {
                if let Err(e) = dc_diagnostics::commands::no_params_given(params) {
                    return Step::Done(Err(invalid(e)));
                }
                match spec.id {
                    dc_diagnostics::commands::STATUS => Ok(self.app.diagnostics_status()),
                    dc_diagnostics::commands::PREVIEW => Ok(self.app.diagnostics_preview()),
                    _ => {
                        let rx = match self.app.agent_diagnostics_send() {
                            Ok(rx) => rx,
                            Err(e) => return Step::Done(Err(RpcError::host(e))),
                        };
                        return Step::Wait(Box::new(move |_: &mut Bridge| match rx.try_recv() {
                            Ok(r) => Some(r.map_err(RpcError::host)),
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                Some(Err(RpcError::host("the send stopped")))
                            }
                        }));
                    }
                }
            }
            // case commands (case.import, module commands): the
            // workspace's command path, so they land on its undo stack
            _ => {
                let ws = match self.workspace() {
                    Ok(ws) => ws,
                    Err(e) => return Step::Done(Err(e)),
                };
                // the command of a menu leaf runs as the button of its
                // inspector: the window shows what a click shows (its
                // report, the study models just built, the new tooth colours)
                if ws.run_leaf_command(spec.id, params.clone()) {
                    return Self::after_jobs(move |ws| match ws.take_leaf_answer() {
                        Some(Ok(v)) => Ok(v),
                        Some(Err(e)) => Err(RpcError::host(format!("{}: {e}", spec.id))),
                        None => Err(RpcError::host(format!("{}: did not run", spec.id))),
                    });
                }
                let out = Arc::new(Mutex::new(None));
                ws.run_command(Box::new(Capturing {
                    spec,
                    params: params.clone(),
                    out: Arc::clone(&out),
                }));
                return Self::after_jobs(move |_| {
                    let r = out.lock().unwrap_or_else(PoisonError::into_inner).take();
                    match r {
                        Some(Ok(v)) => Ok(v),
                        Some(Err(e)) => Err(RpcError::host(format!("{}: {e}", spec.id))),
                        None => Err(RpcError::host(format!("{}: did not run", spec.id))),
                    }
                });
            }
        };
        Step::Done(r)
    }

    /// Runs the commands in order; stops at the first failure.
    fn batch(&mut self, params: &Value) -> Step {
        let Some(items) = params.get("commands").and_then(Value::as_array) else {
            return Step::Done(Err(RpcError::new(
                codes::INVALID_PARAMS,
                "missing array parameter `commands`",
            )));
        };
        let mut queue = VecDeque::new();
        for (i, c) in items.iter().enumerate() {
            let Some(id) = c.get("id").and_then(Value::as_str) else {
                return Step::Done(Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    format!("commands[{i}]: missing `id`"),
                )));
            };
            queue.push_back((
                id.to_owned(),
                c.get("params").cloned().unwrap_or(Value::Null),
            ));
        }
        let mut outputs = Vec::new();
        let mut current: Option<(String, Wait)> = None;
        Step::Wait(Box::new(move |b: &mut Bridge| {
            let fail = |index: usize, id: &str, e: RpcError| {
                Some(Err(RpcError::host(format!(
                    "command {index} ({id}) failed: {}",
                    e.message
                ))))
            };
            loop {
                if let Some((id, mut w)) = current.take() {
                    match w(b) {
                        None => {
                            current = Some((id, w));
                            return None;
                        }
                        Some(Ok(v)) => outputs.push(v),
                        Some(Err(e)) => return fail(outputs.len(), &id, e),
                    }
                }
                let Some((id, p)) = queue.pop_front() else {
                    return Some(Ok(Value::Array(std::mem::take(&mut outputs))));
                };
                match b.execute(&id, p) {
                    Step::Done(Ok(v)) => outputs.push(v),
                    Step::Done(Err(e)) => return fail(outputs.len(), &id, e),
                    Step::Wait(w) => current = Some((id, w)),
                }
            }
        }))
    }

    /// Summary of the app and the case, or the part at `path` (dotted or
    /// JSON pointer) in full.
    fn inspect(&self, path: Option<&str>) -> RpcResult {
        let ws = self.app.workspace();
        let mut tree = json!({
            "app": {
                "screen": format!("{:?}", self.app.screen()).to_lowercase(),
                "workspace_open": ws.is_some(),
                "busy": ws.is_some_and(Workspace::is_busy),
            },
            "view": self.view,
        });
        if let Some(ws) = ws {
            let st = ws.state();
            let mut case = serde_json::to_value(&st.case).unwrap_or(Value::Null);
            if let Some(o) = case.as_object_mut() {
                o.remove("scans");
            }
            let scans: Vec<Value> = st
                .case
                .scans
                .iter()
                .map(|s| {
                    let mesh = st.mesh(&s.mesh);
                    json!({
                        "id": s.id,
                        "role": s.role,
                        "source_name": s.source_name,
                        "source_units": s.source_units,
                        "mesh": s.mesh,
                        "vertices": mesh.map(|m| m.vertex_count()),
                        "faces": mesh.map(|m| m.face_count()),
                    })
                })
                .collect();
            let modules: Map<String, Value> = st
                .modules
                .iter()
                .map(|(id, m)| (id.clone(), m.to_json().unwrap_or(Value::Null)))
                .collect();
            tree["project"] = json!({
                "path": ws.path().map(|p| p.display().to_string()),
                "dirty": ws.is_dirty(),
            });
            tree["case"] = case;
            tree["scans"] = Value::Array(scans);
            tree["modules"] = Value::Object(modules);
        }
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            // the module states can be large: list them, `modules.<id>` has them
            if let Some(m) = tree.get_mut("modules") {
                let ids: Vec<Value> = m
                    .as_object()
                    .map(|o| o.keys().map(|k| json!(k)).collect())
                    .unwrap_or_default();
                *m = Value::Array(ids);
            }
            return Ok(tree);
        };
        let pointer = if path.starts_with('/') {
            path.to_owned()
        } else {
            path.split('.')
                .map(|seg| format!("/{}", seg.replace('~', "~0").replace('/', "~1")))
                .collect()
        };
        tree.pointer(&pointer)
            .cloned()
            .ok_or_else(|| RpcError::host(format!("nothing at {path}")))
    }

    /// CPU render of the current case (empty case: background only).
    fn render(&self, params: &Value) -> RpcResult {
        let empty = Map::new();
        let call = match params {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "render parameters must be an object",
                ));
            }
        };
        let req = RenderRequest::from_layers(&[&self.view, call])
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
        let blank = CaseState::new(dc_case::Case::new(""));
        let ws = self.app.workspace();
        let state: &CaseState = ws.map_or(&blank, |ws| ws.state().as_ref());
        let labels = state
            .module_data::<dca_model::AlignersState>(dca_model::MODULE_ID)
            .map(|s| s.labels.clone())
            .unwrap_or_default();
        // the built teeth and gingiva: what the viewport shows (the 3D teeth or
        // the scan, at its stage, the study models with their layer) unless
        // `model` / `stage` / `study` say otherwise
        let entries = match ws {
            Some(ws) => {
                let (entries, study) = render_entries(
                    &[&self.view, call],
                    ws.teeth_shown(),
                    Some(ws.stage_shown()),
                    ws.study_shown(),
                    |stage| ws.module().scene_at(state, stage),
                )
                .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
                dca_module::study_models_drawn(state, entries, study)
            }
            None => Vec::new(),
        };
        let png = req
            .render_case_png_with(state, &labels, &entries)
            .map_err(RpcError::host)?;
        Ok(json!({
            "png": base64::engine::general_purpose::STANDARD.encode(&png),
            "width": req.options.width,
            "height": req.options.height,
        }))
    }
}

/// What the render draws of the modules' entries (the built teeth and
/// gingiva), from the `model`, `stage` and `study` keys of the request (the
/// last of `layers` that has one wins; `view.set` stores them as defaults):
///
/// - `model`: `"teeth"` draws the entries (they replace the scan of their
///   arch), `"scan"` draws the scans and none of the entries of an arch
///   they replace.
/// - `stage`: the treatment stage that poses the 3D teeth and moves the
///   gingiva (`0` or absent: the initial position). The scans are drawn at
///   rest whatever the stage.
/// - `study`: the study models are drawn (`dca_module::study_models_drawn`),
///   only at the initial position; the flag returned says so.
///
/// `at` gives the entries at a stage; `default_teeth`, `default_stage` and
/// `default_study` apply when the keys are absent.
fn render_entries(
    layers: &[&Map<String, Value>],
    default_teeth: bool,
    default_stage: Option<u32>,
    default_study: bool,
    at: impl Fn(Option<u32>) -> Vec<SceneEntry>,
) -> Result<(Vec<SceneEntry>, bool), String> {
    let get = |key: &str| {
        layers
            .iter()
            .rev()
            .find_map(|l| l.get(key).filter(|v| !v.is_null()))
    };
    let teeth = match get("model") {
        None => default_teeth,
        Some(v) => match v.as_str().map(str::to_ascii_lowercase).as_deref() {
            Some("teeth") => true,
            Some("scan") => false,
            _ => return Err("`model` must be \"scan\" or \"teeth\"".into()),
        },
    };
    let stage = match get("stage") {
        None => default_stage,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or("`stage` must be a non-negative integer")?,
        ),
    };
    let study = match get("study") {
        None => default_study,
        Some(v) => v.as_bool().ok_or("`study` must be true or false")?,
    } && stage.unwrap_or(0) == 0;
    let rest = at(None);
    if teeth {
        let entries = match stage {
            Some(n) if n > 0 => at(Some(n)),
            _ => rest,
        };
        return Ok((entries, study));
    }
    let replaced: Vec<_> = rest
        .iter()
        .filter(|e| e.hides_scan)
        .filter_map(|e| e.arch)
        .collect();
    let entries = rest
        .into_iter()
        .filter(|e| !e.arch.is_some_and(|a| replaced.contains(&a)))
        .collect();
    Ok((entries, study))
}

/// The app with the bridge: pumps the control requests before each frame.
pub struct ControlledApp {
    bridge: Bridge,
    server: ControlServer,
}

impl ControlledApp {
    /// `app` driven by `server` as well as by the user.
    pub fn new(app: DentalCraftApp, server: ControlServer) -> Self {
        Self {
            bridge: Bridge::new(app),
            server,
        }
    }

    /// The bridge (and the app inside it).
    pub fn bridge(&self) -> &Bridge {
        &self.bridge
    }

    /// The address the bridge listens on.
    pub fn local_addr(&self) -> SocketAddr {
        self.server.local_addr()
    }

    /// Runs the pending requests, then draws one frame of the app.
    pub fn show(&mut self, ui: &mut Ui) {
        if !self.server.waker_set {
            let ctx = ui.ctx().clone();
            self.server.set_waker(move || ctx.request_repaint());
            self.bridge.ctx = Some(ui.ctx().clone());
        }
        if self.server.pump(&mut self.bridge) {
            // a request waits for a job: keep polling
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        self.bridge.app.show(ui);
    }
}

impl eframe::App for ControlledApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}
