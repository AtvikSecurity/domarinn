mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::{CaseStatus, CellKey};
use domarinn_server::Settings;
use serde_json::json;

/// The stable `case_key` for the `(openai, -, t1, 0)` cell shared across every
/// run these tests ingest.
fn t1_case_key() -> String {
    CellKey {
        provider_id: "openai".to_string(),
        prompt_id: None,
        test_id: "t1".to_string(),
        repeat: 0,
    }
    .case_key()
    .as_str()
    .to_string()
}

/// Ingest four runs of `proj/suite`, each carrying the same `(openai, t1)`
/// cell so it has one shared `case_key`. Status/score/output vary run to run so
/// the history chain exercises the `output_changed` logic and NULL columns.
///
/// Chronological (oldest → newest):
/// * r1 (offset 0): pass, output None (NULL output_hash + git_commit, empty
///   config_digest sentinel)
/// * r2 (offset 10): fail, output "v2"
/// * r3 (offset 20): fail, output "v2" (same output as r2 → unchanged)
/// * r4 (offset 30): pass, output "v4" (changed vs r3)
async fn seed(app: &axum::Router) {
    // r1: no branch (NULL git_commit) and an empty config_digest sentinel.
    let mut r1 = make_run(
        "r1",
        Some("proj"),
        Some("suite"),
        vec![],
        None,
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass).output(None)],
    );
    r1.config_digest = String::new();

    let r2 = make_run(
        "r2",
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        10,
        &[CaseSpec::new("openai", "t1", CaseStatus::Fail).output(Some("v2"))],
    );
    let r3 = make_run(
        "r3",
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        20,
        &[CaseSpec::new("openai", "t1", CaseStatus::Fail).output(Some("v2"))],
    );
    let r4 = make_run(
        "r4",
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        30,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass).output(Some("v4"))],
    );

    for run in [&r1, &r2, &r3, &r4] {
        let reply = post_json(app, "/api/v1/runs", None, &run_value(run)).await;
        assert_eq!(reply.status, StatusCode::CREATED);
    }
}

#[tokio::test]
async fn history_is_newest_first_with_output_changed_chain() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let ck = t1_case_key();

    let resp = get(
        &app,
        &format!("/api/v1/projects/proj/suites/suite/cases/{ck}/history"),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);
    let body = resp.json();

    assert_eq!(body["project"], "proj");
    assert_eq!(body["suite"], "suite");
    assert_eq!(body["case_key"], ck);
    // No baseline set yet.
    assert!(body["baseline_run_id"].is_null());

    let points = body["points"].as_array().unwrap();
    assert_eq!(points.len(), 4, "expected all four runs");

    // Newest first: r4, r3, r2, r1.
    assert_eq!(points[0]["run_id"], "r4");
    assert_eq!(points[1]["run_id"], "r3");
    assert_eq!(points[2]["run_id"], "r2");
    assert_eq!(points[3]["run_id"], "r1");

    // Status / score follow each run's case.
    assert_eq!(points[0]["status"], "pass");
    assert_eq!(points[0]["score"], 1.0);
    assert_eq!(points[1]["status"], "fail");
    assert_eq!(points[1]["score"], 0.0);
    assert_eq!(points[2]["status"], "fail");
    assert_eq!(points[3]["status"], "pass");

    // output_changed is computed against the chronologically previous point,
    // i.e. points[i+1] (the next-older run):
    //  * r4 vs r3: "v4" != "v2" -> true
    //  * r3 vs r2: "v2" == "v2" -> false
    //  * r2 vs r1: r1 output_hash is NULL -> null
    //  * r1: oldest returned point -> null
    assert_eq!(points[0]["output_changed"], true);
    assert_eq!(points[1]["output_changed"], false);
    assert!(points[2]["output_changed"].is_null());
    assert!(points[3]["output_changed"].is_null());

    // r3 and r2 share the same output, so their hashes match; r4 differs.
    assert_eq!(points[1]["output_hash"], points[2]["output_hash"]);
    assert_ne!(points[0]["output_hash"], points[1]["output_hash"]);

    // Column-null fields on r1: NULL output_hash, NULL git_commit, and the
    // empty-string config_digest sentinel maps to null.
    assert!(points[3]["output_hash"].is_null());
    assert!(points[3]["git_commit"].is_null());
    assert!(points[3]["config_digest"].is_null());

    // Populated fields carry through on the other points.
    assert_eq!(points[0]["git_commit"], "abc123");
    assert_eq!(points[0]["config_digest"], "sha256:deadbeef");
    assert_eq!(points[0]["cost_usd"], 0.0025);
    assert_eq!(points[0]["prompt_tokens"], 10);
    assert_eq!(points[0]["completion_tokens"], 20);
    assert_eq!(points[0]["latency_ms"], 42);
    assert!(points[0]["created_at"].is_string());
}

#[tokio::test]
async fn history_limit_is_clamped() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let ck = t1_case_key();

    // limit=1 -> exactly the newest point.
    let one = get(
        &app,
        &format!("/api/v1/projects/proj/suites/suite/cases/{ck}/history?limit=1"),
    )
    .await;
    assert_eq!(one.status, StatusCode::OK);
    let one_points = one.json()["points"].as_array().unwrap().clone();
    assert_eq!(one_points.len(), 1);
    assert_eq!(one_points[0]["run_id"], "r4");

    // limit=0 clamps up to 1.
    let zero = get(
        &app,
        &format!("/api/v1/projects/proj/suites/suite/cases/{ck}/history?limit=0"),
    )
    .await;
    assert_eq!(zero.status, StatusCode::OK);
    assert_eq!(zero.json()["points"].as_array().unwrap().len(), 1);

    // limit=500 clamps down to the 1..=100 ceiling; we only have four runs.
    let big = get(
        &app,
        &format!("/api/v1/projects/proj/suites/suite/cases/{ck}/history?limit=500"),
    )
    .await;
    assert_eq!(big.status, StatusCode::OK);
    assert_eq!(big.json()["points"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn history_unknown_case_key_is_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let resp = get(
        &app,
        "/api/v1/projects/proj/suites/suite/cases/deadbeefdeadbeef/history",
    )
    .await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn history_unknown_project_or_suite_is_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let ck = t1_case_key();

    let bad_project = get(
        &app,
        &format!("/api/v1/projects/ghost/suites/suite/cases/{ck}/history"),
    )
    .await;
    assert_eq!(bad_project.status, StatusCode::NOT_FOUND);

    let bad_suite = get(
        &app,
        &format!("/api/v1/projects/proj/suites/ghost/cases/{ck}/history"),
    )
    .await;
    assert_eq!(bad_suite.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn history_reports_baseline_run_id_once_set() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let ck = t1_case_key();

    let set = send(
        &app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&json!({ "run_id": "r2" })).unwrap(),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK);

    let resp = get(
        &app,
        &format!("/api/v1/projects/proj/suites/suite/cases/{ck}/history"),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);
    assert_eq!(resp.json()["baseline_run_id"], "r2");
}
