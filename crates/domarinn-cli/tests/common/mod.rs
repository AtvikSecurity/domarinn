//! Shared fixtures for the CLI's end-to-end tests.
//!
//! Each integration test file is its own crate, so anything two of them need
//! lives here rather than being copied — and each compiles the whole module,
//! so anything one of them skips reads as dead code.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;

pub fn bin() -> Command {
    Command::cargo_bin("domarinn").unwrap()
}

/// A suite whose single test echoes a fixed output and asserts it contains
/// `needle`.
pub fn suite(output: &str, needle: &str) -> String {
    format!(
        r#"
version: 1
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{{\"output\":\"{output}\"}}'"]
tests:
  - id: t1
    vars: {{}}
    assert:
      - {{type: contains, value: "{needle}"}}
"#
    )
}

pub fn run_to(dir: &Path, out_json: &str, suite_body: &str) {
    std::fs::write(dir.join("domarinn.yaml"), suite_body).unwrap();
    // The run itself may pass or fail (that is the point of the diff); we only
    // need it to produce the result file, so ignore the exit status.
    bin()
        .args(["run", "--format", "json", "--out", out_json, "--no-cache"])
        .current_dir(dir)
        .output()
        .expect("run command executes");
}

/// The `latest` pointer's current run id (the run just persisted).
pub fn latest_id(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".domarinn/runs/latest"))
        .unwrap()
        .trim()
        .to_string()
}

/// The stored `result.json` for the latest run, deserialized.
pub fn latest_run(dir: &Path) -> domarinn_core::result::RunResult {
    let path = dir
        .join(".domarinn/runs")
        .join(latest_id(dir))
        .join("result.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A one-shot stand-in for the results server: accepts a single request,
/// answers it with `body`, and hands back its base URL.
///
/// Hand-rolled rather than pulled from a mocking crate because the client under
/// test is a *subprocess* — all that has to be real is the socket, and an async
/// mock server would have to be driven from a runtime these blocking tests do
/// not have.
pub fn stub_server(status: &str, body: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );

    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        // Drain the full request before replying. Responding early would let the
        // client hit a closed pipe mid-write and report a transport error
        // instead of the status under test.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let n = sock.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let headers_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
            if let Some(start) = headers_end {
                let head = String::from_utf8_lossy(&buf[..start]).to_lowercase();
                let len: usize = head
                    .split("content-length:")
                    .nth(1)
                    .and_then(|r| r.split("\r\n").next())
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= start + len {
                    break;
                }
            }
        }
        sock.write_all(response.as_bytes()).unwrap();
        sock.flush().ok();
        buf
    });
    (url, handle)
}
