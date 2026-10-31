//! [`RemoteHost`]: drives a running app started with `--control <port>`.
//!
//! # Bridge protocol (`dentalcraft-control/1`)
//!
//! TCP on `127.0.0.1:<port>`, UTF-8, one JSON-RPC 2.0 message per line
//! (`\n`-terminated, no embedded newlines). The client sends requests and
//! the app answers each one, in order, on the same connection:
//!
//! ```text
//! -> {"jsonrpc":"2.0","id":1,"method":"execute","params":{"id":"case.import","params":{"path":"upper.stl"}}}
//! <- {"jsonrpc":"2.0","id":1,"result":{"role":"upper", ...}}
//! <- {"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"unknown command x"}}
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
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine;
use dc_mcp::{Host, HostError, HostResult};
use serde_json::{Value, json};

/// Protocol name answered by `hello`.
pub const PROTOCOL: &str = "dentalcraft-control/1";

/// Address of `target`: a bare port means `127.0.0.1:<port>`.
pub fn address(target: &str) -> String {
    if target.chars().all(|c| c.is_ascii_digit()) {
        format!("127.0.0.1:{target}")
    } else {
        target.to_owned()
    }
}

/// Client of the bridge protocol, usable as an MCP [`Host`].
pub struct RemoteHost {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl RemoteHost {
    /// Connects to `target` (`PORT` or `host:port`).
    pub fn connect(target: &str) -> std::io::Result<Self> {
        let addr = address(target);
        let sock = addr.to_socket_addrs()?.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("no address {addr}"))
        })?;
        let stream = TcpStream::connect_timeout(&sock, Duration::from_secs(5))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            next_id: 1,
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

    /// A fake app answering two requests: one result, one error.
    #[test]
    fn requests_and_errors_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            for _ in 0..2 {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                let id = req["id"].clone();
                let resp = if req["method"] == "list" {
                    json!({"jsonrpc": "2.0", "id": id, "result": [{"id": "x"}]})
                } else {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32000, "message": "nope"}})
                };
                writeln!(w, "{resp}").unwrap();
            }
        });
        let mut h = RemoteHost::connect(&port.to_string()).unwrap();
        assert_eq!(h.list().unwrap(), json!([{"id": "x"}]));
        assert_eq!(h.undo().unwrap_err().0, "nope");
        server.join().unwrap();
    }
}
