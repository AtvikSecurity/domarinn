//! Ctrl-C during `domarinn run` stops the exec children it started.
//!
//! Exec providers run in their own process group, so the timeout can kill a
//! wrapper's grandchildren along with it. The cost of that is that a terminal's
//! SIGINT, delivered to the foreground group, no longer reaches them: if the
//! CLI simply died on the signal, a provider like `sh -c "setup; python
//! agent.py"` would keep running — and keep spending — after the user pressed
//! Ctrl-C. The CLI handles the signal instead and drops the run, whose guards
//! kill each child's group. This drives the real binary to prove it.

#![cfg(unix)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;

/// True while `pid` names a live (non-zombie) process.
fn alive(pid: &str) -> bool {
    let probe = Command::new("kill").args(["-0", pid]).status().unwrap();
    if !probe.success() {
        return false;
    }
    // A reaped-later zombie still answers `kill -0`; it is not running.
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| !s.contains(") Z "))
        .unwrap_or(true)
}

fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
        assert!(Instant::now() < deadline, "the exec provider never started");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn ctrl_c_kills_an_exec_providers_grandchildren_and_exits_130() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    // The provider backgrounds a long sleep (the grandchild), records its pid,
    // and waits on it — the shape of a wrapper script around a real agent.
    let suite = format!(
        r#"version: 1
suite: interrupt
providers:
  - id: wrapper
    type: exec
    command: ["sh", "-c", "sleep 300 & echo $! > {pidfile}; wait"]
    timeout_ms: 600000
tests:
  - id: one
    vars: {{}}
"#,
        pidfile = pidfile.display()
    );
    std::fs::write(dir.path().join("domarinn.yaml"), suite).unwrap();

    let mut child = Command::cargo_bin("domarinn")
        .unwrap()
        .args(["run", "--no-cache", "--no-retries"])
        .arg(dir.path())
        .env("DOMARINN_RUNS_DIR", dir.path().join("runs"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let grandchild = wait_for_file(&pidfile);
    assert!(alive(&grandchild), "the grandchild should be running");

    let sent = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success(), "sending SIGINT failed");

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("domarinn ignored SIGINT");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(status.code(), Some(130), "Ctrl-C exits 128 + SIGINT");

    let deadline = Instant::now() + Duration::from_secs(5);
    while alive(&grandchild) {
        if Instant::now() > deadline {
            let _ = Command::new("kill").args(["-9", &grandchild]).status();
            panic!("the exec provider's grandchild outlived Ctrl-C");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
