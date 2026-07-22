//! HTTP-surface tests for the migration-3 case cell fields/filters, the
//! `GET /runs/{id}/config` endpoint, and `RunDetailResponse.config_digest`.
//!
//! The `''`-sentinel and NULL cases are simulated with a raw `rusqlite`
//! connection straight to the runs database (the same approach `backfill.rs`
//! uses), since no public write path produces a failed-backfill row.

mod common;

use std::path::Path;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::{CaseStatus, RunResult};
use domarinn_server::{AuthMode, Settings};
use rusqlite::Connection;

/// Open a plain read/write connection to the runs database so a test can force
/// the sentinel/NULL states the public API can't produce. A busy timeout keeps
/// it from racing the app's (idle) writer connection.
fn raw(dir: &Path) -> Connection {
    let conn = Connection::open(dir.join("domarinn.db")).expect("open raw runs db");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

/// A three-case run exercising every cell field and filter:
/// idx0 openai/t1 (prompt `p-a`, pass, stop_reason `length`),
/// idx1 anthropic/t2 (no prompt, fail, no stop_reason),
/// idx2 openai/t2 (no prompt, pass, no stop_reason).
fn cells_run(id: &str) -> RunResult {
    let mut run = make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("anthropic", "t2", CaseStatus::Fail),
            CaseSpec::new("openai", "t2", CaseStatus::Pass),
        ],
    );
    // Give idx0 a prompt id + stop_reason. The case_key is derived from the
    // cell, so recompute it after mutating the cell to keep it consistent.
    run.cases[0].cell.prompt_id = Some("p-a".to_string());
    run.cases[0].case_key = run.cases[0].cell.case_key();
    run.cases[0].stop_reason = Some("length".to_string());
    run
}

#[tokio::test]
async fn case_list_exposes_cell_fields() {
    let (app, _dir) = test_app(Settings::default()).await;
    post_json(
        &app,
        "/api/v1/runs",
        None,
        &run_value(&cells_run("r-cells")),
    )
    .await;

    let list = get(&app, "/api/v1/runs/r-cells/cases").await;
    assert_eq!(list.status, StatusCode::OK);
    let cases = list.json()["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 3);

    // idx0: fully-populated cell, prompt + stop_reason present.
    let c0 = &cases[0];
    assert_eq!(c0["provider_id"], "openai");
    assert_eq!(c0["prompt_id"], "p-a");
    assert_eq!(c0["test_id"], "t1");
    assert_eq!(c0["repeat"], 0);
    assert_eq!(c0["score"], 1.0);
    assert_eq!(c0["stop_reason"], "length");

    // idx1: no prompt, no stop_reason -> explicit null (not omitted).
    let c1 = &cases[1];
    assert_eq!(c1["provider_id"], "anthropic");
    assert_eq!(c1["test_id"], "t2");
    assert_eq!(c1["score"], 0.0);
    assert!(c1.get("prompt_id").is_some() && c1["prompt_id"].is_null());
    assert!(c1.get("stop_reason").is_some() && c1["stop_reason"].is_null());
}

#[tokio::test]
async fn case_list_filters_narrow_by_cell_columns() {
    let (app, _dir) = test_app(Settings::default()).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&cells_run("r-f"))).await;

    let count = |reply: &Reply| reply.json()["cases"].as_array().unwrap().len();

    // provider filter.
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?provider=openai").await),
        2
    );
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?provider=anthropic").await),
        1
    );
    // prompt filter (only idx0 has one).
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?prompt=p-a").await),
        1
    );
    // test filter (t2 is idx1 + idx2).
    assert_eq!(count(&get(&app, "/api/v1/runs/r-f/cases?test=t2").await), 2);
    // stop_reason filter (only idx0).
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?stop_reason=length").await),
        1
    );

    // Combined with the existing status filter.
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?provider=openai&status=pass").await),
        2
    );
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?provider=openai&status=fail").await),
        0
    );
    assert_eq!(
        count(&get(&app, "/api/v1/runs/r-f/cases?test=t2&status=fail").await),
        1
    );

    // An unknown query key is a hard 400 (CaseQuery denies unknown fields).
    let bad = get(&app, "/api/v1/runs/r-f/cases?provdr=openai").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn sentinel_and_null_cell_columns_surface_as_null() {
    let (app, dir) = test_app(Settings::default()).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&cells_run("r-sent"))).await;

    // Simulate a failed-backfill row: provider_id gets the '' sentinel, the
    // rest land NULL (exactly what `backfill` stamps).
    {
        let conn = raw(dir.path());
        conn.execute(
            "UPDATE cases SET provider_id='', prompt_id=NULL, test_id=NULL,
                 repeat_idx=NULL, score=NULL, stop_reason=NULL
             WHERE run_id='r-sent' AND idx=0",
            [],
        )
        .unwrap();
    }

    let list = get(&app, "/api/v1/runs/r-sent/cases").await;
    let cases = list.json()["cases"].as_array().unwrap().clone();
    let c0 = &cases[0];
    for key in [
        "provider_id",
        "prompt_id",
        "test_id",
        "repeat",
        "score",
        "stop_reason",
    ] {
        assert!(c0.get(key).is_some(), "missing key {key}");
        assert!(c0[key].is_null(), "expected {key} null, got {:?}", c0[key]);
    }
    // The intact rows are unaffected.
    assert_eq!(cases[1]["provider_id"], "anthropic");
}

#[tokio::test]
async fn run_config_endpoint_returns_digest_and_snapshot() {
    let (app, _dir) = test_app(Settings::default()).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("r-cfg"))).await;

    let cfg = get(&app, "/api/v1/runs/r-cfg/config").await;
    assert_eq!(cfg.status, StatusCode::OK);
    let body = cfg.json();
    assert_eq!(body["run_id"], "r-cfg");
    assert_eq!(body["config_digest"], "sha256:deadbeef");
    assert_eq!(
        body["config"],
        serde_json::json!({"providers": [], "tests": []})
    );
}

#[tokio::test]
async fn run_config_endpoint_404s_for_unknown_run() {
    let (app, _dir) = test_app(Settings::default()).await;
    let cfg = get(&app, "/api/v1/runs/nope/config").await;
    assert_eq!(cfg.status, StatusCode::NOT_FOUND);
    assert!(cfg.json()["error"].is_string());
}

#[tokio::test]
async fn run_config_endpoint_is_read_scoped_like_export() {
    // Closed mode gates reads: the config endpoint is `Scoped<Read>`, exactly
    // like `/export`, so it needs a read token when reads are gated.
    let settings = Settings {
        tokens: Some("read:domarinn_view,write:domarinn_ci".to_string()),
        auth_mode: Some(AuthMode::Closed),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;
    // Ingest with a write token (writes are gated in closed mode).
    post_json(
        &app,
        "/api/v1/runs",
        Some("domarinn_ci"),
        &run_value(&simple_run("r-auth")),
    )
    .await;

    assert_eq!(
        get_auth(&app, "/api/v1/runs/r-auth/config", None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get_auth(&app, "/api/v1/runs/r-auth/config", Some("domarinn_view"))
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn run_detail_exposes_config_digest_and_nulls_sentinel() {
    let (app, dir) = test_app(Settings::default()).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("r-det"))).await;

    // Present after ingest.
    let detail = get(&app, "/api/v1/runs/r-det").await;
    assert_eq!(detail.json()["config_digest"], "sha256:deadbeef");

    // NULL column -> explicit null on the wire.
    {
        let conn = raw(dir.path());
        conn.execute("UPDATE runs SET config_digest=NULL WHERE id='r-det'", [])
            .unwrap();
    }
    let detail = get(&app, "/api/v1/runs/r-det").await;
    assert!(detail.json().get("config_digest").is_some());
    assert!(detail.json()["config_digest"].is_null());

    // '' sentinel column -> also null on the wire.
    {
        let conn = raw(dir.path());
        conn.execute("UPDATE runs SET config_digest='' WHERE id='r-det'", [])
            .unwrap();
    }
    let detail = get(&app, "/api/v1/runs/r-det").await;
    assert!(detail.json()["config_digest"].is_null());
}
