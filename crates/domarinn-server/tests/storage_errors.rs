//! Access-control-bearing reads must not swallow storage errors into a `404`.
//!
//! Every query in here appends the run-set visibility predicate, so its answer
//! is a security answer: "you may not see this" is indistinguishable on the
//! wire from "this does not exist", by design. That design only holds while a
//! *failed* query is loud. A swallowed error turns a locked database, a corrupt
//! page, or a schema the server cannot read into a confident "not found" — the
//! operator sees a clean 404 log and never learns their storage is broken.
//!
//! Each case below breaks exactly the table its target query needs, leaving the
//! rest of the request path intact, and asserts the endpoint answers `500`
//! rather than the `404` it used to. This is the pattern documented on
//! `project_has_visible_runs` in `storage/sets.rs`.
//!
//! **SQLite only.** The fault injection works by opening the app's `.db` file
//! behind its back and dropping a table — there is no equivalent back door
//! into the per-test Postgres database, so under
//! `DOMARINN_TEST_BACKEND=postgres` every test here returns early. The
//! error-propagation contract itself is backend-neutral (the queries and their
//! `?`-not-`.ok()` plumbing are shared), so SQLite coverage covers the code
//! path for both.

mod common;

/// `true` (after logging why) when this binary's sqlite-file fault injection
/// cannot reach the app because it is Postgres-backed.
fn skipped_on_postgres() -> bool {
    if common::pg::backend_is_postgres() {
        eprintln!("skipping on postgres: exercises sqlite file corruption");
        return true;
    }
    false
}

use axum::http::StatusCode;
use axum::Router;
use common::*;
use domarinn_core::result::RunResult;
use rusqlite::Connection;
use serde_json::json;
use tempfile::TempDir;

use domarinn_server::Settings;

/// A seeded open-mode app plus the run it holds. Anonymous callers resolve to
/// the `Public` visibility class, which is what puts the restriction subquery
/// into every statement under test.
async fn seeded() -> (Router, TempDir, RunResult) {
    let (app, dir) = test_app(Settings::default()).await;
    let run = simple_run("r-broken");
    let posted = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(
        posted.status,
        StatusCode::CREATED,
        "seed: {:?}",
        posted.json()
    );
    (app, dir, run)
}

/// Drop `table` from the runs database behind the app's back, standing in for
/// any storage fault the query cannot recover from.
fn break_table(dir: &TempDir, table: &str) {
    let conn = Connection::open(dir.path().join("domarinn.db")).expect("open raw runs db");
    conn.execute_batch(&format!("DROP TABLE {table};"))
        .unwrap_or_else(|e| panic!("dropping {table}: {e}"));
}

#[tokio::test]
async fn a_broken_runs_table_is_a_500_not_a_missing_run() {
    if skipped_on_postgres() {
        return;
    }
    // `get_run_detail`.
    let (app, dir, _run) = seeded().await;
    break_table(&dir, "runs");
    assert_eq!(
        get(&app, "/api/v1/runs/r-broken").await.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "run detail must not report a broken table as a missing run"
    );
}

#[tokio::test]
async fn a_broken_runs_table_is_a_500_from_the_run_existence_gate() {
    if skipped_on_postgres() {
        return;
    }
    // `run_exists`, the gate the child endpoints share.
    let (app, dir, _run) = seeded().await;
    break_table(&dir, "runs");
    for child in ["cases", "matrix"] {
        assert_eq!(
            get(&app, &format!("/api/v1/runs/r-broken/{child}"))
                .await
                .status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "/{child} must not report a broken table as a missing run"
        );
    }
}

#[tokio::test]
async fn a_broken_runs_table_is_a_500_when_pinning_a_baseline() {
    if skipped_on_postgres() {
        return;
    }
    // `set_baseline`'s existence check. The set gate ahead of it reads the
    // policy tables, which are untouched, so the 500 can only come from the
    // check itself.
    let (app, dir, _run) = seeded().await;
    break_table(&dir, "runs");
    let pinned = send(
        &app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&json!({ "run_id": "r-broken" })).unwrap(),
    )
    .await;
    assert_eq!(
        pinned.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "pinning must not report a broken table as a missing run"
    );
}

#[tokio::test]
async fn a_broken_baselines_table_is_a_500_not_an_unpinned_suite() {
    if skipped_on_postgres() {
        return;
    }
    // `read_baseline`. Only the baselines table is gone, so the suite listing
    // gets as far as reading the pin and no further.
    let (app, dir, _run) = seeded().await;
    break_table(&dir, "baselines");
    assert_eq!(
        get(&app, "/api/v1/projects/proj/suites").await.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "a suite must not read as unpinned when the pin cannot be read"
    );
}

#[tokio::test]
async fn a_broken_cases_table_is_a_500_not_a_missing_case() {
    if skipped_on_postgres() {
        return;
    }
    // `get_case_detail`. `runs` survives, so the visibility subquery resolves
    // and the failure is the case read itself.
    let (app, dir, run) = seeded().await;
    let case_key = run.cases[0].case_key.clone();
    break_table(&dir, "cases");
    assert_eq!(
        get(&app, &format!("/api/v1/runs/r-broken/cases/{case_key}"))
            .await
            .status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "case detail must not report a broken table as a missing case"
    );
}
