//! `dentalcraft-cli`: drives DentalCraft from the command line and serves
//! MCP for AI agents (port plan B2.1).
//!
//! Subcommands: `info`, `commands`, `describe`, `run`, `render`, `mcp`,
//! `app` and `licence`. Everything runs on a headless engine session (`dc-session`
//! with the Aligners module) except `mcp --connect PORT`, which talks to a
//! running app started with `dentalcraft --control PORT` (protocol in
//! [`remote`]).

mod host;
mod remote;
mod script;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{ArgMatches, Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use dc_mcp::{Host, HostError};
use serde_json::{Value, json};

use host::CliHost;
use remote::RemoteHost;
use script::Step;

const ABOUT: &str = "dentalcraft-cli works on DentalCraft projects (.dcprj) and scans (STL, OBJ, PLY) without a window.";

/// Copyright line of `--version` and `licence`.
macro_rules! copyright {
    () => {
        "Copyright (c) 2026 Daniele Viti. All rights reserved."
    };
}

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    copyright!(),
    "\nFree of charge under the DentalCraft licence, as is and without warranty. Not a medical device: not for clinical use.\n`dentalcraft-cli licence` prints the licence, `dentalcraft-cli licence --third-party` the third-party notices."
);

/// The DentalCraft licence (EULA), embedded from the repository root.
const LICENCE: &str = include_str!("../../../LICENSE.md");

/// Notices of the bundled open source components.
const THIRD_PARTY_NOTICES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

const AFTER_HELP: &str = "\
A case is changed by running commands on it:

  dentalcraft-cli run --in upper.stl --cmd 'project.save={\"path\":\"case.dcprj\"}'

`dentalcraft-cli help SUBCOMMAND`, or `dentalcraft-cli SUBCOMMAND --help`, explains one subcommand;
`dentalcraft-cli help run` explains how to write a command.";

/// The `--in` / `--role` / import result paragraphs shared by the help of
/// `info`, `run` and `render`.
macro_rules! in_help {
    () => {
        "\
--in FILE is repeatable: a .dcprj project (only as the first --in, it is opened) and/or STL, OBJ or
PLY scans, all imported in one call into the case (a new one when there is no project). The import
recognises the arch of a scan from its file name, else from the geometry when the Aligners module
is there, else the scan takes a free arch and is marked provisional.

--role auto|upper|lower|bite sets the role of the scans. Given once, it applies to every scan file;
given once per scan file, it maps to them in --in order (a project is not counted); any other
count is an error. `auto` (the default) lets the import decide.

The import result is in the output under \"import\": {\"imported\": [{scan_id, role, role_source,
source_name, faces, ...}], \"failed\": [{path, code, reason}], \"warnings\": [...], \"detections\":
[...], \"moved\", \"cleanup\", ...}. A file that fails does not stop the others and is listed in
\"failed\"; the exit status is non-zero only when no scan could be imported."
    };
}

const INFO_HELP: &str = concat!(
    "\
Prints the engine, its modules and the number of commands as JSON; with --in, the summary of the
case made from the inputs (and the import result under \"import\") instead.

",
    in_help!()
);

const RUN_HELP: &str = concat!(
    "\
Runs commands, in order, on one case: FILE with --in (a .dcprj project, and/or STL/OBJ/PLY scans
imported into the case), else a new empty case. Then --save writes the project (.dcprj) and
--print adds the case summary to the output. Prints one JSON object: {\"results\": [{\"id\",
\"output\"}...], \"import\": ..., \"saved\": ..., \"case\": ...}.

  dentalcraft-cli run --in a.stl --in b.stl --role upper --role lower --print

",
    in_help!(),
    "

Each --cmd is one command: COMMAND is a command id, then `=`, then the command's parameters as one
JSON object. Quote it for the shell. A command that needs no parameters is just its id.

--cmd-file FILE reads commands from a UTF-8 text file, one per line: either {\"id\": ..., \"params\":
{...}} (JSON lines) or COMMAND=JSON. Blank lines and # comments are ignored. Repeat --cmd-file or
mix it with --cmd: commands execute in the order those options appear. Errors name the --cmd
position or FILE:LINE.

  dentalcraft-cli run --cmd 'project.new={\"patient_code\":\"P-001\"}' \\
    --cmd 'case.import={\"path\":\"upper.stl\",\"role\":\"upper\"}' --save case.dcprj --print

`dentalcraft-cli commands` lists the command ids; `dentalcraft-cli describe COMMAND` explains one
and its parameters. Coordinates are millimetres; changes to the case are undoable (edit.undo)."
);

const RENDER_HELP: &str = concat!(
    "\
Draws the case as a PNG image with the CPU renderer, after the same --in / --cmd / --cmd-file
steps as run. Scans are coloured by tooth label when the Aligners module has labels. An arch
whose teeth were built draws the 3D teeth and gingiva (--model teeth, the default) or its scan
(--model scan); --stage N poses the 3D teeth at treatment stage N (the scans stay at rest).

Views: occlusal (upper arch, or the lower one when only that is shown), occlusal_lower, frontal,
back, left, right, top, iso. Prints the image's path and size as JSON.

  dentalcraft-cli render --in case.dcprj --view frontal --size 1600x1000 --out front.png

",
    in_help!(),
    "

The output adds \"import\" to the image's path and size when scans were imported."
);

const MCP_HELP: &str = "\
Runs a Model Context Protocol server (JSON-RPC, one message per line) on stdin and stdout. It works
on its own headless case with the Aligners module, or with --connect PORT (or host:port) on the
DentalCraft app started with `dentalcraft --control PORT`, whose commands then run in the window
and are undoable there.";

const APP_HELP: &str = "\
Starts the DentalCraft app (`dentalcraft`), found next to dentalcraft-cli or on PATH, with ARGS.
`dentalcraft-cli app -- --control 7300` starts it with the bridge that `mcp --connect 7300` uses.
Prints the process id and the path as JSON and returns at once.";

/// Command line of `dentalcraft-cli`.
#[derive(Debug, Parser)]
#[command(
    name = "dentalcraft-cli",
    version,
    long_version = LONG_VERSION,
    about = ABOUT,
    after_help = AFTER_HELP
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Print the engine, its modules and commands, or a case summary, as JSON.
    #[command(long_about = INFO_HELP)]
    Info {
        #[command(flatten)]
        inputs: Inputs,
    },
    /// List the commands run can use, one per line.
    Commands {
        /// Print a JSON array of {id, source, label}.
        #[arg(long)]
        json: bool,
    },
    /// Explain one command and its parameters.
    Describe {
        /// Command id.
        id: String,
        /// Print the description as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run commands on a case, then save it.
    #[command(long_about = RUN_HELP)]
    Run(RunArgs),
    /// Draw the case as a PNG image.
    #[command(long_about = RENDER_HELP)]
    Render(RenderArgs),
    /// Serve the commands to an agent over MCP on stdio.
    #[command(long_about = MCP_HELP)]
    Mcp {
        /// Drive the app started with --control PORT (PORT or host:port).
        #[arg(long, value_name = "PORT")]
        connect: Option<String>,
    },
    /// Start the DentalCraft app.
    #[command(long_about = APP_HELP)]
    App {
        /// Print the app's path without starting it.
        #[arg(long)]
        print_path: bool,
        /// Arguments for the app (after `--`).
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            value_name = "ARGS"
        )]
        args: Vec<String>,
    },
    /// Print the DentalCraft licence, or the third-party notices.
    Licence {
        /// Print the copyright and licence notices of the bundled open source components.
        #[arg(long)]
        third_party: bool,
    },
}

/// The project and the scans a case starts from.
#[derive(Debug, Args)]
struct Inputs {
    /// Project (.dcprj, first only) or scan (STL, OBJ, PLY) to start from (repeatable).
    #[arg(long = "in", value_name = "FILE")]
    input: Vec<PathBuf>,
    /// Role of the scans: once for all, or once per scan file (repeatable).
    #[arg(
        long,
        value_name = "ROLE",
        value_parser = ["auto", "upper", "lower", "bite"]
    )]
    role: Vec<String>,
}

/// The case and the commands to run on it.
#[derive(Debug, Args)]
struct Script {
    #[command(flatten)]
    inputs: Inputs,
    /// A command: COMMAND or COMMAND=JSON (repeatable).
    #[arg(long, value_name = "COMMAND[=PARAMS]")]
    cmd: Vec<String>,
    /// A file of commands, one per line (repeatable).
    #[arg(long, value_name = "FILE")]
    cmd_file: Vec<PathBuf>,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[command(flatten)]
    script: Script,
    /// Save the project here (.dcprj).
    #[arg(long, value_name = "OUT")]
    save: Option<PathBuf>,
    /// Add the case summary to the output.
    #[arg(long)]
    print: bool,
}

#[derive(Debug, Args)]
struct RenderArgs {
    #[command(flatten)]
    script: Script,
    /// Camera view.
    #[arg(long, default_value = "occlusal")]
    view: String,
    /// The PNG to write.
    #[arg(long, value_name = "FILE")]
    out: PathBuf,
    /// Image size in pixels.
    #[arg(long, default_value = "1024x768", value_name = "WxH")]
    size: String,
    /// Draw the triangle edges.
    #[arg(long)]
    wireframe: bool,
    /// Supersampling per side (1-4).
    #[arg(long, default_value_t = dc_raster::DEFAULT_SUPERSAMPLE)]
    supersample: u32,
    /// Scan roles to draw, comma separated (upper, lower, bite); all by default.
    #[arg(long, value_delimiter = ',')]
    visible: Vec<String>,
    /// Background colour, #RRGGBB or #RRGGBBAA.
    #[arg(long)]
    background: Option<String>,
    /// What to draw of an arch whose teeth were built: its scan, or the 3D teeth and gingiva.
    #[arg(long, value_parser = ["scan", "teeth"], default_value = "teeth")]
    model: String,
    /// Treatment stage that poses the 3D teeth (0: the initial position; default: no stage).
    #[arg(long, value_name = "N")]
    stage: Option<u32>,
}

type Result<T, E = String> = std::result::Result<T, E>;

fn host_err(e: HostError) -> String {
    e.0
}

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    let sub = matches.subcommand().map(|(_, m)| m);
    let result = match cli.command {
        None => {
            // no subcommand: show what the tool is
            Cli::command().print_help().map_err(|e| e.to_string())
        }
        Some(Cmd::Info { inputs }) => info(&inputs),
        Some(Cmd::Commands { json }) => commands(json),
        Some(Cmd::Describe { id, json }) => describe(&id, json),
        Some(Cmd::Run(args)) => sub.ok_or_else(String::new).and_then(|m| run(&args, m)),
        Some(Cmd::Render(args)) => sub.ok_or_else(String::new).and_then(|m| render(&args, m)),
        Some(Cmd::Mcp { connect }) => mcp(connect.as_deref()),
        Some(Cmd::App { print_path, args }) => app(print_path, &args),
        Some(Cmd::Licence { third_party }) => {
            licence(third_party);
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_json(v: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    // a closed reader (`| head`) does not turn the work into a failure
    let _ = writeln!(out, "{text}");
    Ok(())
}

/// `licence`: the copyright line and the licence, or the third-party notices.
fn licence(third_party: bool) {
    let mut out = std::io::stdout().lock();
    // a closed reader (`| head`) is not an error
    let _ = if third_party {
        out.write_all(THIRD_PARTY_NOTICES.as_bytes())
    } else {
        writeln!(out, "{}\n\n{LICENCE}", copyright!())
    };
}

fn new_host() -> Result<CliHost> {
    CliHost::new().map_err(host_err)
}

/// `true` for a project file (`.dcprj`).
fn is_project(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dcprj"))
}

/// One scan file of `--in`, with its `--role`.
#[derive(Debug, PartialEq)]
struct ScanInput<'a> {
    path: &'a Path,
    role: Option<&'a str>,
}

/// The `--in` and `--role` values, checked: an optional project to open
/// (first `--in` only), then the scan files with their roles.
#[derive(Debug, PartialEq)]
struct InputPlan<'a> {
    project: Option<&'a Path>,
    scans: Vec<ScanInput<'a>>,
}

impl Inputs {
    fn plan(&self) -> Result<InputPlan<'_>> {
        let mut files = self.input.iter().map(PathBuf::as_path).peekable();
        let project = files.next_if(|p| is_project(p));
        let paths: Vec<&Path> = files.collect();
        if let Some(p) = paths.iter().find(|p| is_project(p)) {
            return Err(format!(
                "--in {}: a project (.dcprj) is only allowed as the first --in; give scans after it",
                p.display()
            ));
        }
        let roles = &self.role;
        let role_of = |i: usize| -> Option<&str> {
            match roles.len() {
                0 => None,
                1 => Some(roles[0].as_str()),
                _ => Some(roles[i].as_str()),
            }
        };
        if roles.len() > 1 && roles.len() != paths.len() {
            return Err(format!(
                "--role was given {} times for {} scan file(s): give it once (for all the scans) or once per scan file",
                roles.len(),
                paths.len()
            ));
        }
        if !roles.is_empty() && paths.is_empty() {
            return Err("--role needs a scan file given with --in".to_owned());
        }
        let scans = paths
            .into_iter()
            .enumerate()
            .map(|(i, path)| ScanInput {
                path,
                role: role_of(i),
            })
            .collect();
        Ok(InputPlan { project, scans })
    }
}

/// Id of the command that imports scans: the one a module registers with the
/// [`IMPORT_SCANS_SUFFIX`](dc_module_api::IMPORT_SCANS_SUFFIX) convention (it
/// can recognise the arch from the geometry), else `case.import`. Same choice
/// as the MCP `import_scan` tool.
fn import_command(host: &mut CliHost) -> String {
    host.list()
        .ok()
        .and_then(|list| {
            list.as_array()?
                .iter()
                .filter_map(|c| c.get("id").and_then(Value::as_str).or_else(|| c.as_str()))
                .find(|id| dc_module_api::is_import_scans_command(id))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "case.import".to_owned())
}

/// Opens the project and imports the scans of `inputs` into the case of
/// `host`, the scans in one call. Returns the output of the import, `None`
/// when there are no scans. An import that brings in nothing is an error.
fn load_inputs(host: &mut CliHost, inputs: &Inputs) -> Result<Option<Value>> {
    let plan = inputs.plan()?;
    if let Some(p) = plan.project {
        Host::open(host, &p.to_string_lossy())
            .map_err(|e| format!("--in {}: {}", p.display(), e.0))?;
    }
    if plan.scans.is_empty() {
        return Ok(None);
    }
    let files: Vec<Value> = plan
        .scans
        .iter()
        .map(|s| {
            let mut f = json!({ "path": s.path.to_string_lossy() });
            if let Some(role) = s.role {
                f["role"] = json!(role);
            }
            f
        })
        .collect();
    let id = import_command(host);
    let out = host
        .execute(&id, json!({ "files": files }))
        .map_err(|e| format!("--in: {}", e.0))?;
    Ok(Some(out))
}

fn info(inputs: &Inputs) -> Result<()> {
    let mut host = new_host()?;
    if !inputs.input.is_empty() || !inputs.role.is_empty() {
        let import = load_inputs(&mut host, inputs)?;
        let mut out = host.inspect(None).map_err(host_err)?;
        if let (Some(import), Some(obj)) = (import, out.as_object_mut()) {
            obj.insert("import".to_owned(), import);
        }
        return print_json(&out);
    }
    let modules: Vec<Value> = host
        .session
        .modules()
        .list()
        .map(|m| {
            json!({
                "id": m.id(),
                "name": dc_i18n::tr(m.name_key()),
                "version": m.version(),
            })
        })
        .collect();
    let commands = host.session.list_commands().len();
    print_json(&json!({
        "name": "DentalCraft",
        "cli": "dentalcraft-cli",
        "version": env!("CARGO_PKG_VERSION"),
        "mcp_protocol": dc_mcp::PROTOCOL_VERSION,
        "bridge_protocol": remote::PROTOCOL,
        "modules": modules,
        "commands": commands,
        "views": dc_raster::View::ALL.iter().map(|v| v.name()).collect::<Vec<_>>(),
    }))
}

fn commands(as_json: bool) -> Result<()> {
    let host = new_host()?;
    let list = host.session.list_commands();
    if as_json {
        return print_json(&serde_json::to_value(&list).map_err(|e| e.to_string())?);
    }
    let width = list.iter().map(|c| c.id.len()).max().unwrap_or(0);
    let mut out = std::io::stdout().lock();
    for c in list {
        let _ = writeln!(out, "{:width$}  {}", c.id, c.label);
    }
    Ok(())
}

fn describe(id: &str, as_json: bool) -> Result<()> {
    let host = new_host()?;
    let d = host.session.describe(id).map_err(|e| e.to_string())?;
    if as_json {
        return print_json(&serde_json::to_value(&d).map_err(|e| e.to_string())?);
    }
    let schema = serde_json::to_string_pretty(&d.params_schema).map_err(|e| e.to_string())?;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{} - {} ({})\n\n{}\n\nParameters (JSON Schema):\n{schema}",
        d.id, d.label, d.source, d.doc
    );
    Ok(())
}

/// The `--cmd` and `--cmd-file` steps, in command-line order.
fn steps(script: &Script, m: &ArgMatches) -> Result<Vec<Step>> {
    let idx = |name: &str| -> Vec<usize> {
        m.indices_of(name)
            .map(Iterator::collect)
            .unwrap_or_default()
    };
    let mut ordered: Vec<(usize, Result<Vec<Step>>)> = Vec::new();
    for (n, (text, i)) in script.cmd.iter().zip(idx("cmd")).enumerate() {
        let origin = format!("--cmd {}", n + 1);
        ordered.push((i, script::parse_cmd(text, &origin).map(|s| vec![s])));
    }
    for (path, i) in script.cmd_file.iter().zip(idx("cmd_file")) {
        ordered.push((i, script::read_cmd_file(path)));
    }
    ordered.sort_by_key(|(i, _)| *i);
    let mut out = Vec::new();
    for (_, r) in ordered {
        out.extend(r?);
    }
    Ok(out)
}

/// A case made by [`run_script`].
struct Ran {
    host: CliHost,
    /// The `{id, output}` of the steps.
    results: Vec<Value>,
    /// The output of the scan import, when there were scans.
    import: Option<Value>,
}

/// Loads the inputs and runs the steps.
fn run_script(script: &Script, m: &ArgMatches) -> Result<Ran> {
    let steps = steps(script, m)?;
    let mut host = new_host()?;
    let import = load_inputs(&mut host, &script.inputs)?;
    let mut results = Vec::with_capacity(steps.len());
    for s in steps {
        let out = host
            .execute(&s.id, s.params)
            .map_err(|e| format!("{} ({}): {}", s.origin, s.id, e.0))?;
        results.push(json!({ "id": s.id, "output": out }));
    }
    Ok(Ran {
        host,
        results,
        import,
    })
}

fn run(args: &RunArgs, m: &ArgMatches) -> Result<()> {
    let Ran {
        mut host,
        results,
        import,
    } = run_script(&args.script, m)?;
    let mut out = json!({ "results": results });
    if let Some(import) = import {
        out["import"] = import;
    }
    if let Some(p) = &args.save {
        let saved = Host::save(&mut host, Some(&p.to_string_lossy()))
            .map_err(|e| format!("--save {}: {}", p.display(), e.0))?;
        out["saved"] = saved;
    }
    if args.print {
        out["case"] = host.inspect(None).map_err(host_err)?;
    }
    print_json(&out)
}

fn render(args: &RenderArgs, m: &ArgMatches) -> Result<()> {
    let (w, h) = dc_raster::parse_size(&args.size)
        .ok_or_else(|| format!("--size must look like 1024x768, not {:?}", args.size))?;
    let Ran { host, import, .. } = run_script(&args.script, m)?;
    let mut params = json!({
        "view": args.view,
        "width": w,
        "height": h,
        "wireframe": args.wireframe,
        "supersample": args.supersample,
    });
    if !args.visible.is_empty() {
        params["visible"] = json!(args.visible);
    }
    if let Some(bg) = &args.background {
        params["background"] = json!(bg);
    }
    params["model"] = json!(args.model);
    if let Some(stage) = args.stage {
        params["stage"] = json!(stage);
    }
    let png = host.render_png(&params).map_err(host_err)?;
    std::fs::write(&args.out, &png).map_err(|e| format!("{}: {e}", args.out.display()))?;
    let mut out = json!({
        "path": args.out.display().to_string(),
        "width": w,
        "height": h,
        "view": args.view,
        "bytes": png.len(),
    });
    if let Some(import) = import {
        out["import"] = import;
    }
    print_json(&out)
}

fn mcp(connect: Option<&str>) -> Result<()> {
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let r = match connect {
        Some(target) => {
            let remote = RemoteHost::connect(target)
                .map_err(|e| format!("cannot reach the app at {}: {e}", remote::address(target)))?;
            dc_mcp::serve_stdio(remote, stdin, stdout)
        }
        None => dc_mcp::serve_stdio(new_host()?, stdin, stdout),
    };
    r.map_err(|e| e.to_string())
}

/// File name of the app binary.
const APP_BIN: &str = if cfg!(windows) {
    "dentalcraft.exe"
} else {
    "dentalcraft"
};

/// The app binary: next to this executable, else on `PATH`.
fn find_app() -> Option<PathBuf> {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(APP_BIN)))
        .filter(|p| p.is_file());
    beside.or_else(|| {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|d| d.join(APP_BIN))
                .find(|p| p.is_file())
        })
    })
}

fn app(print_path: bool, args: &[String]) -> Result<()> {
    let path = find_app().ok_or_else(|| {
        format!("cannot find the DentalCraft app ({APP_BIN}) next to dentalcraft-cli or on PATH")
    })?;
    if print_path {
        return print_json(&json!({ "path": path.display().to_string() }));
    }
    let child = std::process::Command::new(&path)
        .args(args)
        .spawn()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    print_json(&json!({
        "pid": child.id(),
        "path": path.display().to_string(),
        "args": args,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
        let help = Cli::command().render_help().to_string();
        assert!(help.contains("dentalcraft-cli"), "{help}");
        for sub in [
            "info", "commands", "describe", "run", "render", "mcp", "app",
        ] {
            assert!(help.contains(sub), "{sub}: {help}");
        }
    }

    #[test]
    fn cmd_and_cmd_file_keep_their_order() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("c.txt");
        std::fs::write(&f, "edit.undo\n").unwrap();
        let f = f.to_string_lossy().into_owned();
        let argv = [
            "dentalcraft-cli",
            "run",
            "--cmd",
            "project.new",
            "--cmd-file",
            &f,
            "--cmd",
            "edit.redo",
        ];
        let m = Cli::command().get_matches_from(argv);
        let Some(Cmd::Run(args)) = Cli::from_arg_matches(&m).unwrap().command else {
            panic!("not run");
        };
        let sub = m.subcommand_matches("run").unwrap();
        let ids: Vec<String> = steps(&args.script, sub)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["project.new", "edit.undo", "edit.redo"]);
    }
}
