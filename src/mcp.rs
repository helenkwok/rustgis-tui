//! A minimal MCP client for `rustgis-cli mcp`: newline-delimited JSON-RPC over stdio.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct Mcp {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

/// What a tool call returned: its text parts and the first image, if any.
pub struct Reply {
    pub text: String,
    pub png: Option<Vec<u8>>,
    pub is_error: bool,
}

impl Mcp {
    pub fn start(cli: &str, args: &[String]) -> Result<Self, String> {
        let mut child = Command::new(cli)
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {cli}: {e}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut m = Mcp { child, stdin, stdout, next_id: 0 };
        m.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "rustgis-tui", "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        m.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(m)
    }

    fn send(&mut self, msg: Value) -> Result<(), String> {
        writeln!(self.stdin, "{msg}").and_then(|_| self.stdin.flush()).map_err(|e| e.to_string())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let mut line = String::new();
        loop {
            line.clear();
            if self.stdout.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("rustgis-cli mcp exited".into());
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            if v["id"].as_u64() != Some(id) {
                continue; // a notification or a stray reply
            }
            if let Some(err) = v.get("error") {
                return Err(err["message"].as_str().unwrap_or("error").to_string());
            }
            return Ok(v["result"].clone());
        }
    }

    pub fn call(&mut self, tool: &str, args: Value) -> Result<Reply, String> {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}))?;
        let mut text = Vec::new();
        let mut png = None;
        for c in r["content"].as_array().into_iter().flatten() {
            match c["type"].as_str() {
                Some("text") => text.push(c["text"].as_str().unwrap_or("").to_string()),
                Some("image") if png.is_none() => {
                    png = c["data"].as_str().and_then(|d| STANDARD.decode(d).ok())
                }
                _ => {}
            }
        }
        Ok(Reply { text: text.join("\n"), png, is_error: r["isError"].as_bool().unwrap_or(false) })
    }

    pub fn cmd(&mut self, id: &str, params: Value) -> Result<Reply, String> {
        self.call("run_command", json!({"command": id, "params": params}))
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
