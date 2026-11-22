//! Bridge mode: `dentalcraft --control <port>` lets `dentalcraft-cli mcp
//! --connect <port>` (or any client) drive the running app.
//!
//! # Protocol (`dentalcraft-control/1`)
//!
//! TCP on `127.0.0.1:<port>` only (port 0 picks a free one; the app prints
//! `control: listening on 127.0.0.1:<port>` and `control: token file
//! <path>` on stdout). Every connection starts with the handshake of
//! [`auth`], which proves the per-run token both ways; at most
//! [`MAX_CONNECTIONS`] connections at once. Then UTF-8, one JSON-RPC 2.0
//! message per line of at most [`MAX_LINE`] bytes; every request gets one
//! answer line, in order:
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
//! invalid requests `-32600` (both also close the connection), unknown
//! methods `-32601`, bad parameters `-32602`.
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
//! see `dc_diagnostics::commands`): `set` only turns the switch of the
//! Settings dialog off (`enabled: true` fails with `consent-only-in-app`:
//! consent is the user's checkbox), `send` sends the waiting reports now
//! and answers when the server did.
//!
//! `list` also has the settings commands `app.settings.get` and
//! `app.settings.set` (source `app`, see `dc_diagnostics::settings`):
//! `get` answers `{advanced, language, theme, check_updates,
//! diagnostics}`; `set` takes `advanced` (the switch "Advanced options and
//! parameters" of the Settings dialog), changes the window at once and
//! saves it. Hidden options stay reachable here: the commands keep all
//! their parameters.
//!
//! `list` also has `app.menu.select` (source `app`, parameter `id`: a menu
//! entry, or the command its inspector runs): it selects that entry of the
//! open project as a click does, so the inspector shows; it never runs
//! anything (the menu law). Then `execute` runs the function, as its
//! inspector's button does. `app.window.screenshot` (parameter `path`)
//! writes the window as the user sees it to a PNG file (a GPU read-back of
//! the next frame; it does not wait for the jobs). `app.window.layout`
//! (parameters `width` + `height`, `menu`, `inspector`, each optional) sizes
//! the window and the side panels, as the user does by dragging, and
//! answers where the menu, the inspector and the 3D view are.
//!
//! `list` also has the scanner's commands `scan.*` and the turntable's
//! `turntable.*` (`dc_scanner::control`): they run on the scan controller
//! of the open workspace, the one its scan window shows, in the same queue
//! as the window's buttons; an agent's command that acts on the scanner or
//! the turntable opens that window. `scan.import` writes the surface to a
//! temporary file and imports it with the workspace's import (one undoable
//! change); it answers with the import report.
//!
//! `list` also has the printing commands `print.*` (`dc_print::commands`,
//! as in the headless session): they work on files, not on the case, so
//! they do not wait for the workspace jobs; each runs on a thread of its
//! own (`print.prepare` takes minutes) and answers when it ends, while the
//! window keeps drawing.
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
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;
use dc_case::CaseState;
use dc_command::{Command, CommandError, JobCtx};
use dc_module_api::{CommandOutput, CommandSpec};
use dc_raster::RenderRequest;
use dc_scene::SceneEntry;
use dc_session::builtins;
use dc_workspace::{PanelWidths, ScanTicket, Workspace};
use egui::{Rect, Ui, Vec2};
use serde_json::{Map, Value, json};

use crate::DentalCraftApp;

/// Protocol name answered by `hello`.
pub const PROTOCOL: &str = "dentalcraft-control/1";

/// Longest line after the handshake; a longer one closes the connection.
pub const MAX_LINE: u64 = 16 * 1024 * 1024;
/// Longest handshake line (a hello is about 150 bytes).
const MAX_HANDSHAKE_LINE: u64 = 1024;
/// Connections served at once; more are closed as soon as they arrive.
pub const MAX_CONNECTIONS: usize = 4;
/// Time a new connection has for the whole handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// The secret of one `--control` run and the handshake that proves it.
///
/// At start the app makes a random 256-bit token and writes it, as 64 hex
/// digits, to `control-<port>.token` in [`token_dir`], a file only the
/// user can read; it deletes it when it ends. A client proves it read the
/// file and the app proves it wrote it, without sending the token:
///
/// ```text
/// -> {"jsonrpc":"2.0","id":1,"method":"auth.hello","params":{"protocol":"dentalcraft-control/1","nonce":"<64 hex: client nonce>"}}
/// <- {"jsonrpc":"2.0","id":1,"result":{"protocol":"dentalcraft-control/1","nonce":"<64 hex: server nonce>","proof":"<64 hex: server proof>"}}
/// -> {"jsonrpc":"2.0","id":2,"method":"auth.proof","params":{"proof":"<64 hex: client proof>"}}
/// <- {"jsonrpc":"2.0","id":2,"result":{"app":"DentalCraft","version":"…","protocol":"dentalcraft-control/1"}}
/// ```
///
/// A proof is the BLAKE3 keyed hash, under the token, of
/// `"dentalcraft-control/1 server"` (or `" client"`), the port as 2
/// big-endian bytes, the client nonce and the server nonce (32 bytes
/// each). Proofs are compared in constant time. The client checks the
/// app's proof before it sends its own, so a program that took the port
/// first learns nothing. Anything else as the first two lines, a wrong
/// proof, or a handshake not done in [`HANDSHAKE_TIMEOUT`] closes the
/// connection without an answer.
pub mod auth {
    use std::io::{self, BufRead, Write};
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use super::PROTOCOL;

    /// Method of the first line.
    pub const HELLO: &str = "auth.hello";
    /// Method of the second line.
    pub const PROOF: &str = "auth.proof";
    /// Environment variable with the path of the token file, for clients
    /// that do not find it from the port (`dentalcraft-cli` reads it too).
    pub const TOKEN_FILE_ENV: &str = "DC_CONTROL_TOKEN_FILE";

    /// A 256-bit secret or nonce.
    pub type Bytes = [u8; 32];

    /// Who proves.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Role {
        /// The app.
        Server,
        /// The client.
        Client,
    }

    /// 32 random bytes from the OS.
    pub fn random() -> io::Result<Bytes> {
        let mut b = [0; 32];
        getrandom::fill(&mut b).map_err(|e| io::Error::other(format!("no random bytes: {e}")))?;
        Ok(b)
    }

    /// 64 lowercase hex digits.
    pub fn to_hex(b: &Bytes) -> String {
        blake3::Hash::from_bytes(*b).to_hex().to_string()
    }

    /// The bytes of 64 hex digits.
    pub fn from_hex(s: &str) -> Option<Bytes> {
        blake3::Hash::from_hex(s).ok().map(|h| *h.as_bytes())
    }

    /// The proof of `role` for one connection to `port`.
    pub fn proof(
        token: &Bytes,
        role: Role,
        port: u16,
        client_nonce: &Bytes,
        server_nonce: &Bytes,
    ) -> blake3::Hash {
        let mut h = blake3::Hasher::new_keyed(token);
        h.update(PROTOCOL.as_bytes());
        h.update(match role {
            Role::Server => b" server",
            Role::Client => b" client",
        });
        h.update(&port.to_be_bytes());
        h.update(client_nonce);
        h.update(server_nonce);
        h.finalize()
    }

    /// `true` when `hex` is `expected` (constant time).
    pub fn proof_matches(hex: &str, expected: &blake3::Hash) -> bool {
        blake3::Hash::from_hex(hex).is_ok_and(|h| h == *expected)
    }

    /// The folder of the token files: `$XDG_RUNTIME_DIR/dentalcraft` on
    /// Linux (and the other Unix systems but macOS) when it is set, else
    /// `control` in the app's local data folder:
    /// `~/.local/share/dentalcraft/control` (Linux),
    /// `~/Library/Application Support/app.DentalCraft.dentalcraft/control`
    /// (macOS), `%LOCALAPPDATA%\DentalCraft\dentalcraft\data\control`
    /// (Windows).
    pub fn token_dir() -> Option<PathBuf> {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
        {
            return Some(d.join("dentalcraft"));
        }
        directories::ProjectDirs::from("app", "DentalCraft", "dentalcraft")
            .map(|d| d.data_local_dir().join("control"))
    }

    /// The token file of the bridge on `port` in `dir`.
    pub fn token_file(dir: &Path, port: u16) -> PathBuf {
        dir.join(format!("control-{port}.token"))
    }

    /// Reads a token file.
    pub fn read_token(path: &Path) -> io::Result<Bytes> {
        let text = std::fs::read_to_string(path)?;
        from_hex(text.trim()).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: not a control token", path.display()),
            )
        })
    }

    /// Writes `token` to the token file of `port` in `dir`, readable by
    /// the user only (folder 0700, file 0600 on Unix; on Windows the
    /// folder is in the user's private `%LOCALAPPDATA%`). A file left by
    /// a run that crashed is replaced: the port is this run's now.
    pub fn write_token(dir: &Path, port: u16, token: &Bytes) -> io::Result<PathBuf> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        std::fs::create_dir_all(dir)?;
        let path = token_file(dir, port);
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&path)?;
        f.write_all(to_hex(token).as_bytes())?;
        Ok(path)
    }

    /// The client side of the handshake on a new connection to `port`;
    /// answers the app's `auth.proof` result. Fails when the app's proof
    /// is wrong (not the app of this token) or the app closes the
    /// connection (it refused the client's proof).
    pub fn client_handshake(
        reader: &mut impl BufRead,
        writer: &mut impl Write,
        token: &Bytes,
        port: u16,
    ) -> io::Result<Value> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::PermissionDenied, m.to_owned());
        let mut call = |id: u64, method: &str, params: Value| -> io::Result<Value> {
            let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            writer.write_all(format!("{req}\n").as_bytes())?;
            writer.flush()?;
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err(bad("the app closed the connection: wrong control token"));
            }
            let v: Value = serde_json::from_str(&line).map_err(|_| bad("not a DentalCraft app"))?;
            match v.get("result") {
                Some(r) if v.get("id") == Some(&json!(id)) => Ok(r.clone()),
                _ => Err(bad("not a DentalCraft app")),
            }
        };
        let client_nonce = random()?;
        let hello = call(
            1,
            HELLO,
            json!({"protocol": PROTOCOL, "nonce": to_hex(&client_nonce)}),
        )?;
        let server_nonce = hello
            .get("nonce")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .ok_or_else(|| bad("not a DentalCraft app"))?;
        let expected = proof(token, Role::Server, port, &client_nonce, &server_nonce);
        if !hello
            .get("proof")
            .and_then(Value::as_str)
            .is_some_and(|p| proof_matches(p, &expected))
        {
            return Err(bad(
                "the program on this port does not know the control token: not this DentalCraft app",
            ));
        }
        let mine = proof(token, Role::Client, port, &client_nonce, &server_nonce);
        call(2, PROOF, json!({"proof": mine.to_hex().as_str()}))
    }
}

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
    token_file: PathBuf,
    rx: mpsc::Receiver<Request>,
    waker: Waker,
    waker_set: bool,
    active: Option<Active>,
}

impl ControlServer {
    /// Listens on `127.0.0.1:port` (0 = any free port) on a background
    /// thread, with its token file in [`auth::token_dir`]; each
    /// connection gets its own thread, at most [`MAX_CONNECTIONS`].
    pub fn bind(port: u16) -> std::io::Result<Self> {
        let dir = auth::token_dir().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no folder for the control token (no XDG_RUNTIME_DIR nor home folder)",
            )
        })?;
        Self::bind_in(port, &dir)
    }

    /// [`bind`](Self::bind) with the token file in `token_dir`.
    pub fn bind_in(port: u16, token_dir: &Path) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let addr = listener.local_addr()?;
        let token = auth::random()?;
        let token_file = auth::write_token(token_dir, addr.port(), &token)?;
        let keys = Arc::new(Keys {
            token,
            port: addr.port(),
        });
        let (tx, rx) = mpsc::channel();
        let waker: Waker = Arc::new(Mutex::new(None));
        let w = Arc::clone(&waker);
        let open = Arc::new(AtomicUsize::new(0));
        let spawned = std::thread::Builder::new()
            .name("dc-control".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        open.fetch_sub(1, Ordering::SeqCst);
                        continue; // dropping `stream` closes it
                    }
                    let slot = Slot(Arc::clone(&open));
                    let (tx, w, keys) = (tx.clone(), Arc::clone(&w), Arc::clone(&keys));
                    let _ = std::thread::Builder::new()
                        .name("dc-control-conn".into())
                        .spawn(move || {
                            let _slot = slot;
                            serve_connection(stream, &keys, &tx, &w);
                        });
                }
            });
        if let Err(e) = spawned {
            let _ = std::fs::remove_file(&token_file);
            return Err(e);
        }
        Ok(Self {
            addr,
            token_file,
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

    /// The file with this run's token (deleted when the server is dropped).
    pub fn token_file(&self) -> &Path {
        &self.token_file
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

impl Drop for ControlServer {
    fn drop(&mut self) {
        // the token is good only while this run listens
        let _ = std::fs::remove_file(&self.token_file);
    }
}

/// The token of a server and the port its proofs name.
struct Keys {
    token: auth::Bytes,
    port: u16,
}

/// A connection slot, freed when its thread ends.
struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The read half of a connection: past `deadline` reads fail, so a
/// silent or slow client cannot hold a slot through the handshake.
struct ReadHalf {
    stream: TcpStream,
    deadline: Option<Instant>,
}

impl Read for ReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(d) = self.deadline {
            let left = d.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

/// Reads one line of at most `max` bytes into `buf`, without its `\n` (or
/// `\r\n`). `None` at the end of the connection, on an error, on a longer
/// line (never buffered whole) and on a line that is not UTF-8.
fn read_line<'a>(r: &mut impl BufRead, buf: &'a mut Vec<u8>, max: u64) -> Option<&'a str> {
    buf.clear();
    let n = r.by_ref().take(max + 1).read_until(b'\n', buf).ok()?;
    if n == 0 || buf.pop() != Some(b'\n') {
        return None;
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    std::str::from_utf8(buf).ok()
}

fn write_line(w: &mut TcpStream, v: &Value) -> Option<()> {
    let mut text = v.to_string();
    text.push('\n');
    w.write_all(text.as_bytes()).ok()?;
    w.flush().ok()
}

/// The params of `msg` when it is the JSON-RPC 2.0 request `method`.
fn auth_params<'a>(msg: &'a Value, method: &str) -> Option<&'a Map<String, Value>> {
    let ok = msg.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && msg.get("method").and_then(Value::as_str) == Some(method)
        && msg.get("id").is_some();
    if !ok {
        return None;
    }
    msg.get("params")?.as_object()
}

/// The app side of the handshake ([`auth`]). `None` closes the connection
/// without an answer.
fn handshake(reader: &mut BufReader<ReadHalf>, writer: &mut TcpStream, keys: &Keys) -> Option<()> {
    let mut buf = Vec::new();
    let hello: Value =
        serde_json::from_str(read_line(reader, &mut buf, MAX_HANDSHAKE_LINE)?).ok()?;
    let params = auth_params(&hello, auth::HELLO)?;
    if params.get("protocol")?.as_str()? != PROTOCOL {
        return None;
    }
    let client_nonce = auth::from_hex(params.get("nonce")?.as_str()?)?;
    let server_nonce = auth::random().ok()?;
    let (token, port) = (&keys.token, keys.port);
    let mine = auth::proof(
        token,
        auth::Role::Server,
        port,
        &client_nonce,
        &server_nonce,
    );
    write_line(
        writer,
        &json!({"jsonrpc": "2.0", "id": hello["id"], "result": {
            "protocol": PROTOCOL,
            "nonce": auth::to_hex(&server_nonce),
            "proof": mine.to_hex().as_str(),
        }}),
    )?;
    let msg: Value = serde_json::from_str(read_line(reader, &mut buf, MAX_HANDSHAKE_LINE)?).ok()?;
    let params = auth_params(&msg, auth::PROOF)?;
    let expected = auth::proof(
        token,
        auth::Role::Client,
        port,
        &client_nonce,
        &server_nonce,
    );
    if !auth::proof_matches(params.get("proof")?.as_str()?, &expected) {
        return None;
    }
    write_line(
        writer,
        &json!({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "app": "DentalCraft",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL,
        }}),
    )
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

/// Runs the handshake, then reads request lines from `stream`, forwards
/// them to the UI thread and writes the answers. A line that is not a
/// JSON-RPC 2.0 request gets its error answer, then the connection closes.
fn serve_connection(stream: TcpStream, keys: &Keys, tx: &mpsc::Sender<Request>, waker: &Waker) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(ReadHalf {
        stream: read_half,
        deadline: Some(Instant::now() + HANDSHAKE_TIMEOUT),
    });
    let mut writer = stream;
    if handshake(&mut reader, &mut writer, keys).is_none() {
        return;
    }
    reader.get_mut().deadline = None;
    if reader.get_ref().stream.set_read_timeout(None).is_err() {
        return;
    }
    let mut buf = Vec::new();
    loop {
        let Some(line) = read_line(&mut reader, &mut buf, MAX_LINE) else {
            return;
        };
        if line.trim().is_empty() {
            continue;
        }
        let (answer, close) = match serde_json::from_str::<Value>(line) {
            Err(e) => (
                response(
                    Value::Null,
                    Err(RpcError::new(
                        codes::PARSE_ERROR,
                        format!("parse error: {e}"),
                    )),
                ),
                true,
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
                        (response(id, r), false)
                    }
                    _ => (
                        response(
                            id,
                            Err(RpcError::new(
                                codes::INVALID_REQUEST,
                                "expected a JSON-RPC 2.0 request with a method",
                            )),
                        ),
                        true,
                    ),
                }
            }
        };
        if write_line(&mut writer, &answer).is_none() || close {
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

/// `app.window.view`: what the 3D view of the window shows, as the toolbar
/// and the timeline set it.
pub const WINDOW_VIEW: &str = "app.window.view";

fn window_view_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "WindowViewParams",
        "type": "object",
        "properties": {
            "view": {
                "description": "Standard view of the camera.",
                "type": "string",
                "enum": ["occlusal", "front", "left", "right", "back"]
            },
            "stage": {
                "description": "Stage shown on the timeline (0 = initial position; at most the last one).",
                "type": "integer",
                "minimum": 0
            },
            "study": {
                "description": "The \"Study models\" layer.",
                "type": "boolean"
            },
            "target": {
                "description": "The \"Final position\" layer (the ghost of the teeth at their final position).",
                "type": "boolean"
            }
        },
        "additionalProperties": false
    })
}

/// The optional boolean parameter `key`.
fn opt_bool_param(params: &Value, key: &str) -> Result<Option<bool>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!("parameter `{key}` must be a boolean"),
        )),
    }
}

/// The optional stage number parameter `key`.
fn opt_u32_param(params: &Value, key: &str) -> Result<Option<u32>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                RpcError::new(
                    codes::INVALID_PARAMS,
                    format!("parameter `{key}` must be a whole number from 0"),
                )
            }),
    }
}

/// The parameters of `app.window.view`: view, stage, study, target.
type WindowViewArgs = (Option<String>, Option<u32>, Option<bool>, Option<bool>);

fn window_view_args(params: &Value) -> Result<WindowViewArgs, RpcError> {
    let known = ["view", "stage", "study", "target"];
    if let Some(k) = params
        .as_object()
        .and_then(|o| o.keys().find(|k| !known.contains(&k.as_str())))
    {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!("{WINDOW_VIEW}: unknown parameter `{k}`"),
        ));
    }
    Ok((
        opt_str_param(params, "view")?.map(str::to_owned),
        opt_u32_param(params, "stage")?,
        opt_bool_param(params, "study")?,
        opt_bool_param(params, "target")?,
    ))
}

/// `app.window.layout`: the size of the window and of the side panels of
/// the workspace, and where its parts are.
pub const WINDOW_LAYOUT: &str = "app.window.layout";

fn window_layout_params() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "WindowLayoutParams",
        "type": "object",
        "properties": {
            "width": {
                "description": "Width of the inside of the window (points); give it with height.",
                "type": "number",
                "exclusiveMinimum": 0
            },
            "height": {
                "description": "Height of the inside of the window (points); give it with width.",
                "type": "number",
                "exclusiveMinimum": 0
            },
            "menu": {
                "description": "Width of the left menu (points), as a drag of its edge sets it: kept within its range, and saved.",
                "type": "number"
            },
            "inspector": {
                "description": "Width of the inspector (points), as a drag of its edge sets it: kept within its range, and saved.",
                "type": "number"
            }
        },
        "additionalProperties": false
    })
}

/// The parameters of `app.window.layout`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct WindowLayoutArgs {
    window: Option<Vec2>,
    menu: Option<f32>,
    inspector: Option<f32>,
}

/// The optional number parameter `key`.
fn opt_f32_param(params: &Value, key: &str) -> Result<Option<f32>, RpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|n| n as f32)
            .filter(|n| n.is_finite())
            .map(Some)
            .ok_or_else(|| {
                RpcError::new(
                    codes::INVALID_PARAMS,
                    format!("parameter `{key}` must be a number"),
                )
            }),
    }
}

fn window_layout_args(params: &Value) -> Result<WindowLayoutArgs, RpcError> {
    let invalid = |e: String| RpcError::new(codes::INVALID_PARAMS, format!("{WINDOW_LAYOUT}: {e}"));
    let known = ["width", "height", "menu", "inspector"];
    if let Some(k) = params
        .as_object()
        .and_then(|o| o.keys().find(|k| !known.contains(&k.as_str())))
    {
        return Err(invalid(format!("unknown parameter `{k}`")));
    }
    let window = match (
        opt_f32_param(params, "width")?,
        opt_f32_param(params, "height")?,
    ) {
        (None, None) => None,
        (Some(w), Some(h)) if w > 0.0 && h > 0.0 => Some(Vec2::new(w, h)),
        (Some(_), Some(_)) => return Err(invalid("width and height must be above 0".into())),
        _ => return Err(invalid("give width and height together".into())),
    };
    Ok(WindowLayoutArgs {
        window,
        menu: opt_f32_param(params, "menu")?,
        inspector: opt_f32_param(params, "inspector")?,
    })
}

/// How long `app.window.layout` waits for the window to take its size.
const LAYOUT_WAIT: Duration = Duration::from_secs(3);

/// The answer of `app.window.layout`: the window, the parts of the
/// workspace as the last frame drew them, the widths chosen for the panels
/// and how far their contents run past them.
fn layout_json(window: Vec2, ws: &Workspace) -> Value {
    let l = ws.layout();
    let chosen = ws.panel_widths();
    let rect =
        |r: Rect| json!({ "x": r.min.x, "y": r.min.y, "width": r.width(), "height": r.height() });
    json!({
        "window": { "width": window.x, "height": window.y },
        "menu": rect(l.menu),
        "inspector": rect(l.inspector),
        "view": rect(l.view),
        "chosen": { "menu": chosen.menu, "inspector": chosen.inspector },
        "overflow": { "menu": l.menu_overflow, "inspector": l.inspector_overflow },
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
        CommandSpec {
            id: WINDOW_VIEW,
            label_key: "app-cmd-window-view",
            doc_key: "app-cmd-window-view-doc",
            params_schema: window_view_params,
            run: run_by_the_app,
        },
        CommandSpec {
            id: WINDOW_LAYOUT,
            label_key: "app-cmd-window-layout",
            doc_key: "app-cmd-window-layout-doc",
            params_schema: window_layout_params,
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

    /// `app.window.layout`: sets the panel widths (saved, as after a drag of
    /// their edges) and the window size, then answers the layout once the
    /// window has its size and drew with it (or after [`LAYOUT_WAIT`]).
    fn window_layout(&mut self, params: &Value) -> Step {
        let args = match window_layout_args(params) {
            Ok(a) => a,
            Err(e) => return Step::Done(Err(e)),
        };
        let Some(ctx) = self.ctx.clone() else {
            return Step::Done(Err(RpcError::host("the window has not drawn yet")));
        };
        let ws = match self.workspace() {
            Ok(ws) => ws,
            Err(e) => return Step::Done(Err(e)),
        };
        if args.menu.is_some() || args.inspector.is_some() {
            let now = ws.panel_widths();
            ws.set_panel_widths(PanelWidths {
                menu: args.menu.unwrap_or(now.menu),
                inspector: args.inspector.unwrap_or(now.inspector),
            });
            let widths = ws.panel_widths();
            self.app.remember_panel_widths(widths);
        }
        if let Some(size) = args.window {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }
        ctx.request_repaint();
        let deadline = std::time::Instant::now() + LAYOUT_WAIT;
        // frames drawn at the asked size (the layout is the last frame's)
        let mut drawn = 0u32;
        Step::Wait(Box::new(move |b: &mut Bridge| {
            let size = ctx.content_rect().size();
            let at_size = args
                .window
                .is_none_or(|w| (w - size).abs().max_elem() < 1.0);
            drawn = if at_size { drawn + 1 } else { 0 };
            if drawn < 3 && std::time::Instant::now() < deadline {
                ctx.request_repaint();
                return None;
            }
            Some(match b.app.workspace() {
                Some(ws) => Ok(layout_json(size, ws)),
                None => Err(RpcError::host("the workspace was closed")),
            })
        }))
    }

    /// The commands: the session built-ins, the scanner's (`scan.*`, run on
    /// the workspace's scan controller, the one its scan window shows), the
    /// printing ones (`print.*`), then the workspace module's (or the
    /// installed Aligners module's when no workspace is open), then the
    /// app's update commands (`app.update.check`, `app.update.apply`),
    /// diagnostics commands (`app.diagnostics.*`) and settings commands
    /// (`app.settings.*`).
    fn specs(&self) -> Vec<(&'static str, CommandSpec)> {
        let mut out: Vec<(&'static str, CommandSpec)> = builtins::specs()
            .into_iter()
            .chain(builtins::scan_specs())
            .chain(builtins::print_specs())
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
        out.extend(
            dc_diagnostics::settings::specs()
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
        if dc_print::commands::is_print_command(spec.id) {
            return Self::execute_print(spec.id, params);
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

    /// Runs the print command `id` (`dc_print::commands`) on a thread of its
    /// own and answers when it ends; the window keeps drawing meanwhile. It
    /// works on files, not on the case: it does not wait for the workspace.
    fn execute_print(id: &'static str, params: Value) -> Step {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("dc-control-print".into())
            .spawn(move || {
                let r = dc_print::commands::execute(id, &params, &|| false, &mut |_| {});
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            return Step::Done(Err(RpcError::host(format!("{id}: {e}"))));
        }
        Step::Wait(Box::new(move |_: &mut Bridge| match rx.try_recv() {
            Ok(Ok(v)) => Some(Ok(v)),
            Ok(Err(e)) if e.is_invalid_call() => Some(Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("{id}: {e}"),
            ))),
            Ok(Err(e)) => Some(Err(RpcError::host(format!("{id}: {e}")))),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(RpcError::host(format!(
                "{id}: the print work stopped unexpectedly"
            )))),
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
            // the toolbar and the timeline of the 3D view
            WINDOW_VIEW => {
                let (view, stage, study, target) = match window_view_args(params) {
                    Ok(p) => p,
                    Err(e) => return Step::Done(Err(e)),
                };
                self.workspace().and_then(|ws| {
                    ws.set_window_view(view.as_deref(), stage, study, target)
                        .map_err(invalid)
                })
            }
            // the window and its side panels, as the user sizes them
            WINDOW_LAYOUT => return self.window_layout(params),
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
            // only off: consent is the user's checkbox in the app
            dc_diagnostics::commands::SET => match dc_diagnostics::commands::off_param(params) {
                Ok(()) => {
                    self.app.set_diagnostics(false);
                    Ok(self.app.diagnostics_status())
                }
                Err(e) => Err(invalid(e)),
            },
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
            // the installation, not the case: the Settings dialog
            dc_diagnostics::settings::GET => {
                if let Err(e) = dc_diagnostics::commands::no_params_given(params) {
                    return Step::Done(Err(invalid(e)));
                }
                Ok(self.app.settings_summary())
            }
            dc_diagnostics::settings::SET => match dc_diagnostics::settings::change_param(params) {
                Ok(change) => {
                    if let Some(on) = change.advanced {
                        self.app.set_advanced(on);
                    }
                    Ok(self.app.settings_summary())
                }
                Err(e) => Err(invalid(e)),
            },
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

    /// The bridge, to act as the user does in the window (tests).
    pub fn bridge_mut(&mut self) -> &mut Bridge {
        &mut self.bridge
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
            ui.ctx()
                .request_repaint_after(dc_ui::pacing::FRAME_INTERVAL);
        }
        self.bridge.app.show(ui);
    }
}

impl eframe::App for ControlledApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        self.bridge.app.pace_frame(raw_input);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    /// A server on a free port, its token file in a temporary folder.
    fn server() -> (ControlServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = ControlServer::bind_in(0, dir.path()).unwrap();
        (s, dir)
    }

    fn connect(s: &ControlServer) -> (BufReader<TcpStream>, TcpStream) {
        let w = TcpStream::connect(s.local_addr()).unwrap();
        w.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        (BufReader::new(w.try_clone().unwrap()), w)
    }

    /// The app closes the connection at once and sends nothing.
    fn closed_unanswered(r: &mut BufReader<TcpStream>) {
        let start = Instant::now();
        let mut all = Vec::new();
        if let Err(e) = r.read_to_end(&mut all) {
            assert!(
                matches!(
                    e.kind(),
                    ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted
                ),
                "not closed: {e}"
            );
        }
        assert!(all.is_empty(), "{}", String::from_utf8_lossy(&all));
        assert!(
            start.elapsed() < HANDSHAKE_TIMEOUT,
            "closed only by the timeout"
        );
    }

    #[test]
    fn the_handshake_proves_the_token_both_ways() {
        let (s, _dir) = server();
        let token = auth::read_token(s.token_file()).unwrap();
        let (mut r, mut w) = connect(&s);
        let ok = auth::client_handshake(&mut r, &mut w, &token, s.local_addr().port()).unwrap();
        assert_eq!(
            (ok["app"].as_str(), ok["protocol"].as_str()),
            (Some("DentalCraft"), Some(PROTOCOL))
        );
        // then a line that is not JSON-RPC gets its error and closes the connection
        writeln!(w, "GET / HTTP/1.1").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["error"]["code"], codes::PARSE_ERROR);
        closed_unanswered(&mut r);
        assert!(s.rx.try_recv().is_err());
    }

    #[test]
    fn a_wrong_token_fails_the_handshake() {
        let (s, _dir) = server();
        let port = s.local_addr().port();
        // the client sees that the app does not prove its token and stops
        let (mut r, mut w) = connect(&s);
        let e = auth::client_handshake(&mut r, &mut w, &[7; 32], port).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::PermissionDenied);
        // a client that sends back the app's own proof (it has no token) is closed
        let (mut r, mut w) = connect(&s);
        let hello = json!({"jsonrpc": "2.0", "id": 1, "method": auth::HELLO,
                           "params": {"protocol": PROTOCOL, "nonce": auth::to_hex(&[1; 32])}});
        writeln!(w, "{hello}").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let answer: Value = serde_json::from_str(&line).unwrap();
        let theirs = answer["result"]["proof"].as_str().unwrap().to_owned();
        let proof =
            json!({"jsonrpc": "2.0", "id": 2, "method": auth::PROOF, "params": {"proof": theirs}});
        writeln!(w, "{proof}").unwrap();
        closed_unanswered(&mut r);
        assert!(s.rx.try_recv().is_err());
    }

    #[test]
    fn anything_but_a_hello_first_closes_the_connection_unanswered() {
        let (s, _dir) = server();
        let port = s.local_addr().port();
        // what a web page's `fetch` sends: the request line and headers, then the body line
        let body = "\n{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"execute\",\"params\":{\"id\":\"app.window.screenshot\",\"params\":{\"path\":\"x.png\"}}}\n";
        let http = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let wrong_protocol = json!({"jsonrpc": "2.0", "id": 1, "method": auth::HELLO,
                                    "params": {"protocol": "other/1", "nonce": auth::to_hex(&[1; 32])}});
        for first in [
            http,
            "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"list\"}\n".to_owned(),
            "\n".to_owned(),
            format!("{wrong_protocol}\n"),
            "x".repeat(4096),
        ] {
            let (mut r, mut w) = connect(&s);
            w.write_all(first.as_bytes()).unwrap();
            closed_unanswered(&mut r);
        }
        // nothing reached the window
        assert!(s.rx.try_recv().is_err());
    }

    #[test]
    fn lines_and_connections_are_bounded() {
        let mut buf = Vec::new();
        let mut line =
            |text: &[u8]| read_line(&mut Cursor::new(text), &mut buf, 8).map(str::to_owned);
        assert_eq!(line(b"12345678\nnext").as_deref(), Some("12345678"));
        assert_eq!(line(b"ab\r\n").as_deref(), Some("ab"));
        assert_eq!(line(b"123456789\n"), None);
        assert_eq!(line(b"no end"), None);
        assert_eq!(line(b""), None);
        // silent clients hold every slot: the next connection is closed at once
        let (s, _dir) = server();
        let held: Vec<_> = (0..MAX_CONNECTIONS).map(|_| connect(&s)).collect();
        let (mut r, _w) = connect(&s);
        closed_unanswered(&mut r);
        drop(held);
    }

    #[test]
    fn the_token_file_is_private_replaced_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = dir.path().join("control");
        let stale = auth::write_token(&tokens, 1, &[1; 32]).unwrap();
        let fresh = auth::write_token(&tokens, 1, &[2; 32]).unwrap();
        assert_eq!(stale, fresh);
        assert_eq!(auth::read_token(&fresh).unwrap(), [2; 32]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!((mode(&fresh), mode(&tokens)), (0o600, 0o700));
        }
        let s = ControlServer::bind_in(0, &tokens).unwrap();
        let file = s.token_file().to_path_buf();
        assert_eq!(file, auth::token_file(&tokens, s.local_addr().port()));
        assert_ne!(auth::read_token(&file).unwrap(), [2; 32]);
        drop(s);
        assert!(!file.exists());
    }
}
