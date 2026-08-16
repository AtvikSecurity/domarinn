//! The `domarinn baseline` subcommand: managing the server-side pin from a
//! terminal. `show`/`set`/`clear` address the suite named by the local
//! `domarinn.yaml` and talk to the same PUT/GET/DELETE endpoints the web UI
//! uses.

mod common;

use assert_cmd::prelude::*;
use common::{bin, latest_id, stub_routes_status};
use predicates::prelude::*;

fn named_suite(dir: &std::path::Path) {
    std::fs::write(
        dir.join("domarinn.yaml"),
        r#"
version: 1
project: p
suite: s
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    )
    .unwrap();
}

#[test]
fn baseline_set_pins_a_branch_on_the_server() {
    let (url, server) = stub_routes_status(
        vec![(
            "/baseline",
            "200 OK",
            r#"{"project":"p","suite":"s","branch":"main"}"#.to_string(),
        )],
        1,
        std::time::Duration::from_secs(30),
    );

    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin()
        .args(["baseline", "set", "--branch", "main"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("main"));

    let served = server.join().unwrap();
    assert!(
        served[0].starts_with("PUT ") && served[0].contains("\"branch\":\"main\""),
        "expected a PUT with a branch body: {}",
        served[0]
    );
}

#[test]
fn baseline_set_resolves_latest_to_a_run_id() {
    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin().arg("run").current_dir(dir.path()).assert().success();
    let id = latest_id(dir.path());

    let (url, server) = stub_routes_status(
        vec![(
            "/baseline",
            "200 OK",
            format!(r#"{{"project":"p","suite":"s","run_id":"{id}"}}"#),
        )],
        1,
        std::time::Duration::from_secs(30),
    );

    bin()
        .args(["baseline", "set", "latest"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success();

    let served = server.join().unwrap();
    assert!(
        served[0].contains(&format!("\"run_id\":\"{id}\"")),
        "`latest` must be resolved to the concrete run id before the PUT: {}",
        served[0]
    );
}

#[test]
fn baseline_set_requires_exactly_one_of_run_or_branch() {
    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());

    // Neither.
    bin()
        .args(["baseline", "set"])
        .env("DOMARINN_SERVER_URL", "http://127.0.0.1:1")
        .current_dir(dir.path())
        .assert()
        .code(2);

    // Both — clap's conflict.
    bin()
        .args(["baseline", "set", "some-run", "--branch", "main"])
        .env("DOMARINN_SERVER_URL", "http://127.0.0.1:1")
        .current_dir(dir.path())
        .assert()
        .code(2);
}

#[test]
fn baseline_show_reports_a_branch_pin_distinctly_from_a_run_pin() {
    let branch_pin =
        r#"{"project":"p","suite":"s","run_id":null,"branch":"main","set_at":1755300000000}"#;
    let (url, _server) = stub_routes_status(
        vec![("/baseline", "200 OK", branch_pin.to_string())],
        1,
        std::time::Duration::from_secs(30),
    );

    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin()
        .args(["baseline", "show"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("branch"))
        .stdout(predicate::str::contains("main"));
}

#[test]
fn baseline_show_on_an_unpinned_suite_is_not_an_error() {
    let body = r#"{"error":"no baseline pinned for p/s","code":"baseline_unpinned"}"#;
    let (url, _server) = stub_routes_status(
        vec![("/baseline", "404 Not Found", body.to_string())],
        1,
        std::time::Duration::from_secs(30),
    );

    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin()
        .args(["baseline", "show"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no baseline pinned"));
}

#[test]
fn baseline_clear_deletes_the_pin() {
    let (url, server) = stub_routes_status(
        vec![("/baseline", "204 No Content", String::new())],
        1,
        std::time::Duration::from_secs(30),
    );

    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin()
        .args(["baseline", "clear"])
        .env("DOMARINN_SERVER_URL", &url)
        .current_dir(dir.path())
        .assert()
        .success();

    let served = server.join().unwrap();
    assert!(served[0].starts_with("DELETE "), "{}", served[0]);
}

#[test]
fn baseline_needs_a_project_and_suite() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("domarinn.yaml"),
        r#"
version: 1
providers:
  - id: p
    type: exec
    command: ["sh", "-c", "cat >/dev/null; printf '{\"output\":\"hello\"}'"]
tests:
  - id: t1
    vars: {}
    assert:
      - {type: contains, value: "hello"}
"#,
    )
    .unwrap();

    bin()
        .args(["baseline", "show"])
        .env("DOMARINN_SERVER_URL", "http://127.0.0.1:1")
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("project"));
}

#[test]
fn baseline_needs_a_server() {
    let dir = tempfile::tempdir().unwrap();
    named_suite(dir.path());
    bin()
        .args(["baseline", "show"])
        .env_remove("DOMARINN_SERVER_URL")
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("server"));
}
