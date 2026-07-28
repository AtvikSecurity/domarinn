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
    stub_server_owned(status.to_string(), body.to_string())
}

/// [`stub_server`] over owned strings, for bodies computed at run time (a run
/// document read back off disk, say).
pub fn stub_server_owned(
    status: String,
    body: String,
) -> (String, std::thread::JoinHandle<Vec<u8>>) {
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

/// A stand-in for the results server that answers up to `count` requests,
/// choosing each reply by the first route whose fragment appears in the request
/// line. Unmatched requests get a 404.
///
/// [`stub_server`] answers exactly one request, which is all `share` needs.
/// Resolving `--against server:baseline` takes two — the suite listing, then the
/// baseline run's export — and they need different bodies, so routing on the
/// path is clearer than depending on their order.
///
/// Gives up after `deadline` rather than blocking forever, so a client that
/// makes *fewer* calls than expected fails the assertion instead of hanging the
/// suite. Returns the request lines it served, so a test can assert which
/// endpoints were actually called.
pub fn stub_routes(
    routes: Vec<(&'static str, String)>,
    count: usize,
    deadline: std::time::Duration,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    let handle = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut served = Vec::new();
        while served.len() < count && started.elapsed() < deadline {
            let Ok((mut sock, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            sock.set_nonblocking(false).ok();
            sock.set_read_timeout(Some(deadline)).ok();

            // These are GETs, so the headers are the whole request — no need to
            // drain a body before routing.
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match sock.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let line = String::from_utf8_lossy(&buf)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();

            let response = match routes.iter().find(|(fragment, _)| line.contains(fragment)) {
                Some((_, body)) => format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\ncontent-length: 11\r\nconnection: close\r\n\r\n{\"error\":\"\"}".to_string(),
            };
            sock.write_all(response.as_bytes()).ok();
            sock.flush().ok();
            served.push(line);
        }
        served
    });
    (url, handle)
}
