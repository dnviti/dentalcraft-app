//! [`RemoteHost`]: drives a running app started with `--control <port>`.
//!
//! # Bridge protocol (`dentalcraft-control/1`)
//!
//! TCP on `127.0.0.1:<port>`, UTF-8, one JSON-RPC 2.0 message per line
//! (`\n`-terminated, no embedded newlines, at most 16 MiB). Every
//! connection starts with a handshake that proves, in both directions,
//! knowledge of the per-run token the app wrote to its token file, without
//! sending it (see `dc_app::control::auth`):
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"auth.hello","params":{"protocol":"dentalcraft-control/1","nonce":"<64 hex>"}}
//! <- {"jsonrpc":"2.0","id":1,"result":{"protocol":"dentalcraft-control/1","nonce":"<64 hex>","proof":"<64 hex>"}}
//! -> {"jsonrpc":"2.0","id":2,"method":"auth.proof","params":{"proof":"<64 hex>"}}
//! <- {"jsonrpc":"2.0","id":2,"result":{"app":"DentalCraft","version":"…","protocol":"dentalcraft-control/1"}}
//! ```
//!
//! A proof is `BLAKE3-keyed(token, "dentalcraft-control/1 server" | "…
//! client", port as 2 big-endian bytes, client nonce, server nonce)`. The
//! client checks the app's proof before it sends its own. The token file
//! is `control-<port>.token` in `$XDG_RUNTIME_DIR/dentalcraft` (Linux,
//! when set), else in `control` of the app's local data folder, or the
//! file named by `DC_CONTROL_TOKEN_FILE`. A wrong first line, a wrong
//! proof or a slow handshake closes the connection without an answer; any
//! line that is not JSON-RPC closes it too. Then the client sends requests
//! and the app answers each one, in order:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":3,"method":"execute","params":{"id":"case.import","params":{"path":"upper.stl"}}}
//! <- {"jsonrpc":"2.0","id":3,"result":{"role":"upper", ...}}
//! <- {"jsonrpc":"2.0","id":4,"error":{"code":-32000,"message":"unknown command x"}}
//! ```
//!
//! Methods mirror [`dc_mcp::Host`]: `hello`, `list`, `describe {id}`,
//! `execute {id, params}`, `batch {commands: [{id, params}]}`,
//! `inspect {path?}`, `undo`, `redo`, `open {path}`, `save {path?}`,
//! `render {width?, height?, size?, view?, wireframe?, visible?, ...}` and
//! its alias `screenshot`. `render` answers `{"png": <base64>, "width",
//! "height"}`. Errors of the operation use code `-32000`; protocol errors
//! use the JSON-RPC codes (`-32700`, `-32600`, `-32601`, `-32602`).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use dc_mcp::{Host, HostError, HostResult};
use serde_json::{Value, json};

/// Protocol name answered by `hello`.
pub const PROTOCOL: &str = "dentalcraft-control/1";
/// Environment variable naming the token file, for an app whose token file
/// is not where the port says (the app prints its path at start).
pub const TOKEN_FILE_ENV: &str = "DC_CONTROL_TOKEN_FILE";

/// Address of `target`: a bare port means `127.0.0.1:<port>`.
pub fn address(target: &str) -> String {
    if target.chars().all(|c| c.is_ascii_digit()) {
        format!("127.0.0.1:{target}")
    } else {
        target.to_owned()
    }
}

/// The folder of the app's token files: `$XDG_RUNTIME_DIR/dentalcraft` on
/// Linux (and the other Unix systems but macOS) when it is set, else
/// `control` in the app's local data folder (as `dc_app::control::auth`).
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

/// The token file of the app on `port`: [`TOKEN_FILE_ENV`] when set.
pub fn token_file(port: u16) -> Option<PathBuf> {
    match std::env::var_os(TOKEN_FILE_ENV) {
        Some(f) if !f.is_empty() => Some(PathBuf::from(f)),
        _ => token_dir().map(|d| d.join(format!("control-{port}.token"))),
    }
}

/// The proof of `role` (`b" server"` or `b" client"`) for one connection.
fn proof(
    token: &[u8; 32],
    role: &[u8],
    port: u16,
    client: &[u8; 32],
    server: &[u8; 32],
) -> blake3::Hash {
    let mut h = blake3::Hasher::new_keyed(token);
    h.update(PROTOCOL.as_bytes());
    h.update(role);
    h.update(&port.to_be_bytes());
    h.update(client);
    h.update(server);
    h.finalize()
}

fn from_hex(s: &str) -> Option<[u8; 32]> {
    blake3::Hash::from_hex(s).ok().map(|h| *h.as_bytes())
}

fn denied(message: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, message)
}

/// Client of the bridge protocol, usable as an MCP [`Host`].
pub struct RemoteHost {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl RemoteHost {
    /// Connects to `target` (`PORT` or `host:port`) and runs the handshake
    /// with the token file of its port ([`token_file`]).
    pub fn connect(target: &str) -> std::io::Result<Self> {
        let sock = Self::socket(target)?;
        let file = token_file(sock.port()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no folder for the control token; set {TOKEN_FILE_ENV}"),
            )
        })?;
        Self::open(sock, &file)
    }

    fn socket(target: &str) -> std::io::Result<SocketAddr> {
        let addr = address(target);
        addr.to_socket_addrs()?.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address {addr}"))
        })
    }

    fn open(sock: SocketAddr, token_file: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(token_file).map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "cannot read the control token {}: {e} (is DentalCraft running with --control {}? \
                     {TOKEN_FILE_ENV} names another file)",
                    token_file.display(),
                    sock.port()
                ),
            )
        })?;
        let token = from_hex(text.trim()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{}: not a control token", token_file.display()),
            )
        })?;
        let stream = TcpStream::connect_timeout(&sock, Duration::from_secs(5))?;
        stream.set_nodelay(true)?;
        let mut host = Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
        };
        // a program that took the port and never answers must not hang the CLI
        host.writer
            .set_read_timeout(Some(Duration::from_secs(10)))?;
        host.handshake(&token, sock.port())?;
        host.writer.set_read_timeout(None)?;
        Ok(host)
    }

    /// Proves the token to the app after the app proved it (the app's
    /// proof is checked first, so a program that took the port learns
    /// nothing).
    fn handshake(&mut self, token: &[u8; 32], port: u16) -> std::io::Result<()> {
        let mut client_nonce = [0; 32];
        getrandom::fill(&mut client_nonce)
            .map_err(|e| std::io::Error::other(format!("no random bytes: {e}")))?;
        let not_the_app = || {
            denied(format!(
                "bridge handshake: the program on port {port} does not know the control token: \
                 it is not the DentalCraft app of this token file"
            ))
        };
        let hello = self
            .call(
                "auth.hello",
                json!({"protocol": PROTOCOL,
                       "nonce": blake3::Hash::from_bytes(client_nonce).to_hex().as_str()}),
            )
            .map_err(|_| not_the_app())?;
        let server_nonce = hello
            .get("nonce")
            .and_then(Value::as_str)
            .and_then(from_hex)
            .ok_or_else(not_the_app)?;
        let theirs = proof(token, b" server", port, &client_nonce, &server_nonce);
        // `blake3::Hash` compares in constant time
        if hello
            .get("proof")
            .and_then(Value::as_str)
            .and_then(|p| blake3::Hash::from_hex(p).ok())
            != Some(theirs)
        {
            return Err(not_the_app());
        }
        let mine = proof(token, b" client", port, &client_nonce, &server_nonce);
        self.call("auth.proof", json!({"proof": mine.to_hex().as_str()}))
            .map(drop)
            .map_err(|e| {
                denied(format!(
                    "bridge handshake: the app refused this token ({}): the token file is of another run",
                    e.0
                ))
            })
    }

    /// Sends one request and waits for its answer.
    pub fn call(&mut self, method: &str, params: Value) -> HostResult {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let io = |e: std::io::Error| HostError::new(format!("bridge connection: {e}"));
        let mut line = req.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).map_err(io)?;
        self.writer.flush().map_err(io)?;
        let mut resp = String::new();
        if self.reader.read_line(&mut resp).map_err(io)? == 0 {
            return Err(HostError::new("bridge connection closed by the app"));
        }
        let v: Value = serde_json::from_str(&resp)
            .map_err(|e| HostError::new(format!("bridge: invalid answer: {e}")))?;
        if v.get("id") != Some(&json!(id)) {
            return Err(HostError::new(format!(
                "bridge: answer to another request: {v}"
            )));
        }
        if let Some(e) = v.get("error") {
            let msg = e
                .get("message")
                .and_then(Value::as_str)
                .map_or_else(|| e.to_string(), str::to_owned);
            return Err(HostError(msg));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }
}

impl Host for RemoteHost {
    fn execute(&mut self, id: &str, params: Value) -> HostResult {
        self.call("execute", json!({ "id": id, "params": params }))
    }
    fn batch(&mut self, commands: Vec<(String, Value)>) -> HostResult {
        let commands: Vec<Value> = commands
            .into_iter()
            .map(|(id, params)| json!({ "id": id, "params": params }))
            .collect();
        self.call("batch", json!({ "commands": commands }))
    }
    fn list(&mut self) -> HostResult {
        self.call("list", json!({}))
    }
    fn describe(&mut self, id: &str) -> HostResult {
        self.call("describe", json!({ "id": id }))
    }
    fn inspect(&mut self, path: Option<&str>) -> HostResult {
        self.call("inspect", json!({ "path": path }))
    }
    fn undo(&mut self) -> HostResult {
        self.call("undo", json!({}))
    }
    fn redo(&mut self) -> HostResult {
        self.call("redo", json!({}))
    }
    fn open(&mut self, path: &str) -> HostResult {
        self.call("open", json!({ "path": path }))
    }
    fn save(&mut self, path: Option<&str>) -> HostResult {
        self.call("save", json!({ "path": path }))
    }
    fn render(&mut self, params: &Value) -> HostResult<Option<Vec<u8>>> {
        let r = self.call("render", params.clone())?;
        let Some(b64) = r.get("png").and_then(Value::as_str) else {
            return Ok(None);
        };
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map(Some)
            .map_err(|e| HostError::new(format!("bridge: invalid PNG data: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn bare_port_means_loopback() {
        assert_eq!(address("7300"), "127.0.0.1:7300");
        assert_eq!(address("localhost:7300"), "localhost:7300");
    }

    /// A token file in a temporary folder.
    fn token(dir: &Path, token: &[u8; 32]) -> PathBuf {
        let f = dir.join("control.token");
        std::fs::write(&f, blake3::Hash::from_bytes(*token).to_hex().as_str()).unwrap();
        f
    }

    fn read_json(r: &mut impl BufRead) -> Option<Value> {
        let mut line = String::new();
        (r.read_line(&mut line).ok()? > 0).then(|| serde_json::from_str(&line).unwrap())
    }

    /// The app side of the hello, with `key` for the app's proof.
    fn answer_hello(
        r: &mut impl BufRead,
        w: &mut TcpStream,
        key: &[u8; 32],
        port: u16,
    ) -> ([u8; 32], [u8; 32]) {
        let hello = read_json(r).unwrap();
        assert_eq!(hello["method"], "auth.hello");
        assert_eq!(hello["params"]["protocol"], PROTOCOL);
        let client = from_hex(hello["params"]["nonce"].as_str().unwrap()).unwrap();
        let server = [9; 32];
        let p = proof(key, b" server", port, &client, &server);
        let result = json!({"protocol": PROTOCOL, "nonce": blake3::Hash::from_bytes(server).to_hex().as_str(),
                            "proof": p.to_hex().as_str()});
        writeln!(
            w,
            "{}",
            json!({"jsonrpc": "2.0", "id": hello["id"], "result": result})
        )
        .unwrap();
        (client, server)
    }

    /// A fake app that knows the token answering two requests: one result, one error.
    #[test]
    fn requests_and_errors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let file = token(dir.path(), &[5; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            let (client, server) = answer_hello(&mut r, &mut w, &[5; 32], addr.port());
            let p = read_json(&mut r).unwrap();
            assert_eq!(p["method"], "auth.proof");
            let expected = proof(&[5; 32], b" client", addr.port(), &client, &server);
            assert_eq!(p["params"]["proof"], expected.to_hex().as_str());
            writeln!(
                w,
                "{}",
                json!({"jsonrpc": "2.0", "id": p["id"], "result": {"app": "DentalCraft"}})
            )
            .unwrap();
            for _ in 0..2 {
                let req = read_json(&mut r).unwrap();
                let id = req["id"].clone();
                let resp = if req["method"] == "list" {
                    json!({"jsonrpc": "2.0", "id": id, "result": [{"id": "x"}]})
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "nope"}})
                };
                writeln!(w, "{resp}").unwrap();
            }
        });
        let mut h = RemoteHost::open(addr, &file).unwrap();
        assert_eq!(h.list().unwrap(), json!([{"id": "x"}]));
        assert_eq!(h.undo().unwrap_err().0, "nope");
        server.join().unwrap();
    }

    /// A program that took the port first cannot prove the token: the
    /// client stops before it sends its own proof.
    #[test]
    fn a_fake_app_fails_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let file = token(dir.path(), &[5; 32]);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            answer_hello(&mut r, &mut w, &[6; 32], addr.port());
            // nothing more: the client closed
            read_json(&mut r)
        });
        let e = RemoteHost::open(addr, &file).err().unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            e.to_string().contains("does not know the control token"),
            "{e}"
        );
        assert_eq!(server.join().unwrap(), None);
    }

    /// Against the app's real bridge: its token passes, another one and a
    /// missing file fail with a clear error.
    #[test]
    fn the_app_bridge_takes_only_its_token() {
        let dir = tempfile::tempdir().unwrap();
        let app = dc_app::control::ControlServer::bind_in(0, dir.path()).unwrap();
        let addr = app.local_addr();
        let h = RemoteHost::open(addr, app.token_file()).unwrap();
        assert_eq!(h.next_id, 3);
        let other = tempfile::tempdir().unwrap();
        let e = RemoteHost::open(addr, &token(other.path(), &[5; 32]))
            .err()
            .unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
        let e = RemoteHost::open(addr, &other.path().join("none.token"))
            .err()
            .unwrap();
        assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
        assert!(e.to_string().contains("none.token"), "{e}");
        // the CLI finds the file of the port by itself
        assert_eq!(
            token_dir().map(|d| d.join(format!("control-{}.token", addr.port()))),
            dc_app::control::auth::token_dir()
                .map(|d| dc_app::control::auth::token_file(&d, addr.port()))
        );
    }
}
