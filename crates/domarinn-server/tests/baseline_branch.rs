//! Branch-pinned baselines over HTTP: the extended PUT body, the pin metadata
//! GET, and `GET .../baseline/export` — the one endpoint `--against
//! server:baseline` and `server:branch:<name>` resolve through. A branch
//! resolves to a *composite*: per case_key, the newest run on the branch wins,
//! so a filtered newest run cannot shrink the gate's coverage.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::Settings;
use serde_json::json;

/// Two runs on `main` — the newest a partial (`t1` only, now failing), the
/// older full (`t1`+`t2` passing) — plus one run on another branch that must
/// never leak into `main`'s composite.
async fn seed(app: &axum::Router) {
    let runs = [
        make_run(
            "m1",
            Some("proj"),
            Some("suite"),
            vec![],
            Some("main"),
            0,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Pass),
            ],
        ),
        make_run(
            "m2",
            Some("proj"),
            Some("suite"),
            vec![],
            Some("main"),
            10,
            &[CaseSpec::new("openai", "t1", CaseStatus::Fail)],
        ),
        make_run(
            "f1",
            Some("proj"),
            Some("suite"),
            vec![],
            Some("feat/x"),
            20,
            &[CaseSpec::new("openai", "t3", CaseStatus::Pass)],
        ),
    ];
    for run in &runs {
        let reply = post_json(app, "/api/v1/runs", None, &run_value(run)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "seed {}", run.run_id);
    }
}

async fn put_pin(app: &axum::Router, body: serde_json::Value) -> Reply {
    send(
        app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&body).unwrap(),
    )
    .await
}

/// The statuses of an exported document's cases, keyed by test id — the shape
/// assertions about a composite actually care about.
fn statuses(doc: &serde_json::Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["cell"]["test_id"].as_str().unwrap().to_string(),
                c["status"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn putting_a_branch_pin_round_trips_through_get() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let set = put_pin(&app, json!({ "branch": "main" })).await;
    assert_eq!(set.status, StatusCode::OK);
    assert_eq!(set.json()["branch"], "main");

    let got = get(&app, "/api/v1/projects/proj/suites/suite/baseline").await;
    assert_eq!(got.status, StatusCode::OK);
    let body = got.json();
    assert_eq!(body["branch"], "main");
    assert!(body["run_id"].is_null());
    assert!(body["set_at"].is_i64() || body["set_at"].is_string());
}

#[tokio::test]
async fn a_put_with_both_run_id_and_branch_is_rejected() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let set = put_pin(&app, json!({ "run_id": "m1", "branch": "main" })).await;
    assert_eq!(set.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_put_with_neither_run_id_nor_branch_is_rejected() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    assert_eq!(
        put_pin(&app, json!({})).await.status,
        StatusCode::BAD_REQUEST
    );
    // Whitespace is not a branch name.
    assert_eq!(
        put_pin(&app, json!({ "branch": "  " })).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_run_pin_export_returns_that_runs_document() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    assert_eq!(
        put_pin(&app, json!({ "run_id": "m1" })).await.status,
        StatusCode::OK
    );

    let export = get(&app, "/api/v1/projects/proj/suites/suite/baseline/export").await;
    assert_eq!(export.status, StatusCode::OK);
    let doc = export.json();
    assert_eq!(doc["run_id"], "m1");
    assert!(
        doc.get("composite").is_none() || doc["composite"].is_null(),
        "a fixed-run export is the stored document, not a composite"
    );
}

#[tokio::test]
async fn a_branch_pin_export_is_a_composite_of_recent_runs_on_the_branch() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    assert_eq!(
        put_pin(&app, json!({ "branch": "main" })).await.status,
        StatusCode::OK
    );

    let export = get(&app, "/api/v1/projects/proj/suites/suite/baseline/export").await;
    assert_eq!(export.status, StatusCode::OK);
    let doc = export.json();

    // t1 from the newest run (m2, failing); t2 filled from the older full run.
    // Nothing from the other branch.
    assert_eq!(
        statuses(&doc),
        vec![
            ("t1".to_string(), "fail".to_string()),
            ("t2".to_string(), "pass".to_string()),
        ]
    );
    assert_eq!(doc["composite"]["branch"], "main");
    assert_eq!(
        doc["composite"]["contributing_run_ids"],
        json!(["m2", "m1"])
    );
    assert_eq!(doc["project"], "proj");
    assert_eq!(doc["suite"], "suite");
}

#[tokio::test]
async fn a_pinless_branch_export_resolves_without_a_pin() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let export = get(
        &app,
        "/api/v1/projects/proj/suites/suite/baseline/export?branch=main",
    )
    .await;
    assert_eq!(export.status, StatusCode::OK);
    assert_eq!(export.json()["composite"]["branch"], "main");
}

#[tokio::test]
async fn the_export_excludes_the_run_named_by_exclude() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    // Excluding the newest main run (the one being gated, re-uploaded early),
    // the composite falls back to the older full run alone.
    let export = get(
        &app,
        "/api/v1/projects/proj/suites/suite/baseline/export?branch=main&exclude=m2",
    )
    .await;
    assert_eq!(export.status, StatusCode::OK);
    let doc = export.json();
    assert_eq!(doc["composite"]["contributing_run_ids"], json!(["m1"]));
    assert_eq!(
        statuses(&doc),
        vec![
            ("t1".to_string(), "pass".to_string()),
            ("t2".to_string(), "pass".to_string()),
        ]
    );
}

#[tokio::test]
async fn a_branch_with_no_runs_exports_a_coded_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let export = get(
        &app,
        "/api/v1/projects/proj/suites/suite/baseline/export?branch=ghost",
    )
    .await;
    assert_eq!(export.status, StatusCode::NOT_FOUND);
    assert_eq!(export.json()["code"], "no_runs_on_branch");
}

#[tokio::test]
async fn an_unpinned_suite_exports_a_coded_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let export = get(&app, "/api/v1/projects/proj/suites/suite/baseline/export").await;
    assert_eq!(export.status, StatusCode::NOT_FOUND);
    assert_eq!(export.json()["code"], "baseline_unpinned");
}

#[tokio::test]
async fn fully_cached_runs_still_contribute_to_a_composite() {
    let (app, _dir) = test_app(Settings::default()).await;
    // Every case a cache hit: verdicts are re-derived from cached requests and
    // are as real as any other run's — the runs-list hiding of fully-cached
    // runs must not leak into baseline resolution.
    let cached = make_run(
        "warm",
        Some("proj"),
        Some("cachy"),
        vec![],
        Some("main"),
        0,
        &[CaseSpec {
            cached: true,
            ..CaseSpec::new("openai", "t1", CaseStatus::Pass)
        }],
    );
    let reply = post_json(&app, "/api/v1/runs", None, &run_value(&cached)).await;
    assert_eq!(reply.status, StatusCode::CREATED);

    let export = get(
        &app,
        "/api/v1/projects/proj/suites/cachy/baseline/export?branch=main",
    )
    .await;
    assert_eq!(export.status, StatusCode::OK);
    assert_eq!(
        export.json()["composite"]["contributing_run_ids"],
        json!(["warm"])
    );
}

#[tokio::test]
async fn an_invisible_runs_cases_never_enter_a_composite() {
    let (app, storage, _dir) =
        test_app_with_storage(Settings::default(), domarinn_server::AuthMode::Open).await;
    seed(&app).await;
    storage
        .restrict_run_set("proj".into(), None, Some("root".into()))
        .await
        .unwrap();

    // Anonymous caller: the whole restricted project is invisible, so the
    // branch has no visible runs at all — an absence, not a leak.
    let export = get(
        &app,
        "/api/v1/projects/proj/suites/suite/baseline/export?branch=main",
    )
    .await;
    assert_eq!(export.status, StatusCode::NOT_FOUND);
    assert_eq!(export.json()["code"], "no_runs_on_branch");
}

#[tokio::test]
async fn pinning_a_branch_then_a_run_replaces_the_pin() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    assert_eq!(
        put_pin(&app, json!({ "branch": "main" })).await.status,
        StatusCode::OK
    );
    assert_eq!(
        put_pin(&app, json!({ "run_id": "m1" })).await.status,
        StatusCode::OK
    );

    let got = get(&app, "/api/v1/projects/proj/suites/suite/baseline").await;
    assert_eq!(got.json()["run_id"], "m1");
    assert!(got.json()["branch"].is_null());

    // And back again: a branch pin clears the run.
    assert_eq!(
        put_pin(&app, json!({ "branch": "main" })).await.status,
        StatusCode::OK
    );
    let got = get(&app, "/api/v1/projects/proj/suites/suite/baseline").await;
    assert!(got.json()["run_id"].is_null());
    assert_eq!(got.json()["branch"], "main");
}
