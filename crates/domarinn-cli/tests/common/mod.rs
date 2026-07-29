//! Shared fixtures for the CLI's end-to-end tests.
//!
//! Each integration test file is its own crate, so anything two of them need
//! lives here rather than being copied — and each compiles the whole module,
//! so anything one of them skips reads as dead code.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

use assert_cmd::prelude::*;

/// Environment the engine reads when it records a run's provenance.
///
/// Cleared for every test invocation so the suite is hermetic. Since provenance
/// moved into the engine, `domarinn run` stamps whatever CI the *host* is in
/// onto the run it writes — so on a GitHub Actions runner these tests would
/// record the real workflow's URL and actor, and any assertion about a run's CI
/// metadata would pass locally and fail in CI (which is exactly how this was
/// found). A test that wants CI sets these itself, on the `run` invocation:
/// `ci-summary` prefers the run's own recorded URL over the ambient
/// environment, so setting them only at summary time proves nothing.
const CI_ENV: &[&str] = &[
    "GITHUB_ACTIONS",
    "GITHUB_SERVER_URL",
    "GITHUB_REPOSITORY",
    "GITHUB_RUN_ID",
    "GITHUB_ACTOR",
    "GITLAB_CI",
    "GITLAB_USER_LOGIN",
    "CI_JOB_URL",
    "JENKINS_URL",
    "BUILD_URL",
    "BUILD_USER",
    "BUILDKITE_BUILD_CREATOR",
    "CI",
];

pub fn bin() -> Command {
    let mut cmd = Command::cargo_bin("domarinn").unwrap();
    for key in CI_ENV {
        cmd.env_remove(key);
    }
    cmd
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

/// Offset just past the `\r\n\r\n` that ends a request head, once it has all
/// arrived.
fn head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// The `content-length` a request head declares, or `0` when it declares none.
///
/// Shared by both stubs rather than inlined twice: a drain loop is only correct
/// if it agrees with the sender about how many bytes are still coming, and two
/// copies of a header parse are one edit away from disagreeing.
fn content_length(head: &[u8]) -> usize {
    let head = String::from_utf8_lossy(head).to_lowercase();
    head.split("content-length:")
        .nth(1)
        .and_then(|rest| rest.split("\r\n").next())
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Read a whole request: headers, then however many body bytes they declare.
///
/// Draining the body before replying is not politeness. A server that answers
/// at end-of-headers and closes leaves the client writing into a closed pipe,
/// which surfaces as a transport error rather than the status under test — so a
/// stub that skips this can only serve request shapes that carry no body.
fn read_request(sock: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(start) = head_end(&buf) {
            if buf.len() >= start + content_length(&buf[..start]) {
                break;
            }
        }
        match sock.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf
}

/// One HTTP/1.1 response whose `content-length` cannot disagree with its body,
/// because it is computed from it.
///
/// The 404 in [`stub_script`] used to hardcode `11` for a 12-byte body, so a
/// client reading by length got truncated JSON and reported a parse error
/// instead of the 404.
fn http_response(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
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
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let response = http_response(&status, &body);

    let handle = std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().unwrap();
        // Drain the full request before replying — see `read_request`.
        let buf = read_request(&mut sock);
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
    // One body per route is the degenerate script, so there is a single
    // implementation of the socket handling rather than two that drift.
    stub_script(
        routes.into_iter().map(|(f, b)| (f, vec![b])).collect(),
        count,
        deadline,
    )
}

/// [`stub_routes`] where each route answers a *sequence* of bodies — one per
/// matching request, repeating the last once the script runs out.
///
/// A single fixed body per route cannot express a case whose whole point is
/// that two calls differ. The `similar` assertion embeds the output and the
/// reference and takes their cosine, so one repeated embedding scores 1.0
/// against itself and the threshold under test is never exercised at any value.
///
/// Order is deterministic because a run is serial by default
/// (`domarinn-core/src/runner.rs` resolves concurrency to 1); a suite that sets
/// `runner.concurrency` must be driven with `-j 1` to rely on a script.
pub fn stub_script(
    routes: Vec<(&'static str, Vec<String>)>,
    count: usize,
    deadline: std::time::Duration,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::Write;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());

    let handle = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let mut served = Vec::new();
        // How far into each route's script we have got.
        let mut cursor = vec![0usize; routes.len()];
        while served.len() < count && started.elapsed() < deadline {
            let Ok((mut sock, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            sock.set_nonblocking(false).ok();
            sock.set_read_timeout(Some(deadline)).ok();

            // Drain the body too, not just the head: model providers POST, and
            // replying early resets the connection mid-write (see
            // `read_request`).
            let buf = read_request(&mut sock);
            let line = String::from_utf8_lossy(&buf)
                .lines()
                .next()
                .unwrap_or_default()
                .to_string();

            let response = match routes
                .iter()
                .position(|(fragment, _)| line.contains(fragment))
            {
                Some(i) => {
                    let script = &routes[i].1;
                    let at = cursor[i].min(script.len().saturating_sub(1));
                    cursor[i] += 1;
                    http_response("200 OK", &script[at])
                }
                // Named, not blank: an unmatched request becomes an errored
                // cell, and the operator needs the path to know what went
                // unrouted.
                None => http_response(
                    "404 Not Found",
                    &format!("{{\"error\":\"no stub route for {line}\"}}"),
                ),
            };
            sock.write_all(response.as_bytes()).ok();
            sock.flush().ok();
            served.push(line);
        }
        served
    });
    (url, handle)
}
