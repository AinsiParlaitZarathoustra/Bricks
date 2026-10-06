//! A local model server speaking `chat_completions` (SSE), and a temporary
//! Bricks home + project for the `bricks` binary. No key, no network.

#![allow(dead_code)]

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

pub fn text(t: &str) -> String {
    let first =
        json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "content": t } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
                       "usage": { "prompt_tokens": 50, "completion_tokens": 5, "total_tokens": 55 } });
    format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

pub fn tool(id: &str, name: &str, args: Value) -> String {
    let call = json!({ "choices": [{ "index": 0, "delta": { "role": "assistant", "tool_calls": [
        { "index": 0, "id": id, "type": "function", "function": { "name": name, "arguments": args.to_string() } }
    ] } }] });
    let last = json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }],
                       "usage": { "prompt_tokens": 50, "completion_tokens": 5, "total_tokens": 55 } });
    format!("data: {call}\n\ndata: {last}\n\ndata: [DONE]\n\n")
}

/// Never answers (cancellation tests).
pub const HANG: &str = "HANG";

pub struct Model {
    pub url: String,
    pub seen: Arc<Mutex<Vec<Value>>>,
}

/// Answers `replies` in order, then `text("ok")`.
pub fn model(replies: Vec<String>) -> Model {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let replies = Arc::new(Mutex::new(replies.into_iter()));
    std::thread::spawn(move || {
        while let Ok((mut sock, _)) = listener.accept() {
            let replies = replies.clone();
            let s2 = s2.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let start = loop {
                    let n = sock.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..start]).to_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0);
                while buf.len() < start + len {
                    let n = sock.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                if let Ok(v) = serde_json::from_slice::<Value>(&buf[start..]) {
                    s2.lock().unwrap().push(v);
                }
                let body = replies.lock().unwrap().next().unwrap_or_else(|| text("ok"));
                if body == HANG {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                    return;
                }
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes());
            });
        }
    });
    Model {
        url: format!("http://127.0.0.1:{port}/v1"),
        seen,
    }
}

pub struct Project {
    pub home: tempfile::TempDir,
    pub dir: tempfile::TempDir,
}

impl Project {
    /// A Bricks home with a providers file for `url`, and a project whose
    /// bricks.toml selects the model and holds `extra`.
    pub fn new(url: &str, extra: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("providers.toml"),
            format!(
                r#"schema_version = 1

[[providers]]
id = "local"
name = "Local test server"
endpoint = "{url}"
protocol = "chat_completions"
auth = "none"

[[providers.models]]
id = "m"
name = "Test model"
api_model = "test-model"
streaming = true
tool_calls = true

[providers.models.limits]
max_input_tokens = 100000
max_output_tokens = 4096

[providers.models.reasoning]
profiles = [{{ id = "deep" }}]
"#
            ),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("bricks.toml"),
            format!("[agent]\nmodel = \"local/m\"\n\n{extra}"),
        )
        .unwrap();
        Self { home, dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_bricks"));
        c.args(args)
            .current_dir(self.dir.path())
            .env("BRICKS_HOME", self.home.path())
            .env_remove("OPENAI_API_KEY")
            .env_remove("GEMINI_API_KEY");
        c
    }

    /// Run with stdin from `input` (or empty).
    pub fn run(&self, args: &[&str], input: Option<&str>) -> Output {
        let mut c = self.command(args);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        if let Some(i) = input {
            stdin.write_all(i.as_bytes()).unwrap();
        }
        drop(stdin);
        child.wait_with_output().unwrap()
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.home.path().join("sessions")
    }
}

/// Parse stdout as JSONL envelopes.
pub fn envelopes(stdout: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON ({e}): {l}")))
        .collect()
}
