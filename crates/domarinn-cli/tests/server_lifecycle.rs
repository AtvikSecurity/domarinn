//! `domarinn server` / `domarinn healthcheck` / `domarinn completions`, driven
//! through the built binary: the probe is a real HTTP check, the port and bind
//! address follow the environment, and the server exits cleanly on SIGTERM.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn bin() -> Command {
    let mut cmd = Command::cargo_bin("domarinn").unwrap();
    // The ambient environment must not steer the probe or the server.
    cmd.env_remove("DOMARINN_PORT")
        .env_remove("DOMARINN_BIND")
        .env_remove("DOMARINN_DATABASE_URL")
        .env_remove("DOMARINN_AUTH_MODE");
    cmd
}

/// A port nothing is listening on (bound once by the OS, then released).
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Kills the server if a test assertion panics before it is reaped.
struct ServerGuard(Option<Child>);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Start `domarinn server` on loopback, with its port from `DOMARINN_PORT`
/// (exercising the env binding), and wait until `healthcheck` passes.
fn start_server(data_dir: &std::path::Path, port: u16) -> ServerGuard {
    let child = bin()
        .args(["server", "--bind", "127.0.0.1", "--data-dir"])
        .arg(data_dir)
        .env("DOMARINN_PORT", port.to_string())
        .env("DOMARINN_LOG_FORMAT", "compact")
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = ServerGuard(Some(child));

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let probe = bin()
            .args(["healthcheck", "--port", &port.to_string()])
            .output()
            .unwrap();
        if probe.status.success() {
            return guard;
        }
        let exited = guard.0.as_mut().unwrap().try_wait().unwrap();
        assert!(exited.is_none(), "server exited before becoming healthy");
        assert!(Instant::now() < deadline, "server never became healthy");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn healthcheck_fails_against_a_closed_port() {
    bin()
        .args(["healthcheck", "--port", &free_port().to_string()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("healthcheck failed"));
}

/// The probe is HTTP, not a TCP connect: a listener that accepts the
/// connection but answers 503 must fail the check.
#[test]
fn healthcheck_fails_on_a_non_2xx_response() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let responder = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 1024];
        let _ = stream.read(&mut buf);
        let request = String::from_utf8_lossy(&buf).to_string();
        stream
            .write_all(
                b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .unwrap();
        request
    });

    bin()
        .args(["healthcheck", "--port", &port.to_string()])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("503"));

    let request = responder.join().unwrap();
    assert!(
        request.starts_with("GET /api/v1/health "),
        "healthcheck must GET /api/v1/health, sent: {request}"
    );
}

#[test]
fn healthcheck_reads_the_port_from_domarinn_port() {
    // Nothing on this port either way; the error names the URL it tried, which
    // proves the env var — not the 8321 default — chose it.
    let port = free_port();
    bin()
        .arg("healthcheck")
        .env("DOMARINN_PORT", port.to_string())
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!(
            "127.0.0.1:{port}/api/v1/health"
        )));
}

#[test]
fn healthcheck_probes_a_specific_domarinn_bind_address() {
    let port = free_port();
    bin()
        .args(["healthcheck", "--port", &port.to_string()])
        .env("DOMARINN_BIND", "::1")
        .assert()
        .code(3)
        .stderr(predicate::str::contains(format!("[::1]:{port}")));
}

#[test]
fn server_rejects_an_invalid_bind_address() {
    bin()
        .args([
            "server",
            "--bind",
            "not-an-ip",
            "--data-dir",
            "/nonexistent",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--bind"));
}

/// The full lifecycle: start, pass the HTTP healthcheck, then stop on SIGTERM
/// — what `docker stop` sends to the image's PID 1 — with a clean exit and a
/// log line naming the signal.
#[cfg(unix)]
#[test]
fn server_passes_healthcheck_and_stops_cleanly_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut guard = start_server(dir.path(), port);

    let pid = guard.0.as_ref().unwrap().id();
    let killed = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .unwrap();
    assert!(killed.success(), "sending SIGTERM failed");

    let mut child = guard.0.take().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("server ignored SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(
        status.success(),
        "SIGTERM must be a clean exit, got {status}:\n{stderr}"
    );
    assert!(
        stderr.contains("SIGTERM"),
        "shutdown log must name the signal:\n{stderr}"
    );

    // And the port is released: the probe now fails.
    bin()
        .args(["healthcheck", "--port", &port.to_string()])
        .assert()
        .code(3);
}

#[test]
fn completions_prints_a_script_for_each_shell() {
    for (shell, needle) in [
        ("bash", "_domarinn"),
        ("zsh", "#compdef domarinn"),
        ("fish", "complete -c domarinn"),
        ("powershell", "domarinn"),
        ("elvish", "domarinn"),
    ] {
        bin()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains(needle))
            // Subcommands are in the script, including this one.
            .stdout(predicate::str::contains("healthcheck"));
    }
}

#[test]
fn completions_rejects_an_unknown_shell() {
    bin().args(["completions", "tcsh"]).assert().code(2);
}
