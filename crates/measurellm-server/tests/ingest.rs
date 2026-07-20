mod common;

use axum::http::StatusCode;
use common::*;
use measurellm_core::result::CaseStatus;
use measurellm_server::Settings;

#[tokio::test]
async fn ingest_new_run_returns_created_with_url() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = simple_run("run-1");
    let reply = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    let body = reply.json();
    assert_eq!(body["id"], "run-1");
    assert!(body["url"].as_str().unwrap().ends_with("/runs/run-1"));
}

#[tokio::test]
async fn ingest_is_idempotent_on_identical_repost() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = simple_run("run-2");
    let first = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(first.status, StatusCode::CREATED);

    let second = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(second.status, StatusCode::OK);
    assert_eq!(second.json()["id"], "run-2");

    // Only one run should exist.
    let list = get(&app, "/api/v1/runs").await;
    assert_eq!(list.json()["runs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ingest_same_id_different_content_conflicts() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = simple_run("run-3");
    let ok = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(ok.status, StatusCode::CREATED);

    // Same id, different content (extra failing case).
    let mutated = make_run(
        "run-3",
        Some("proj"),
        Some("suite"),
        vec!["nightly"],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("openai", "t2", CaseStatus::Fail),
        ],
    );
    let conflict = post_json(&app, "/api/v1/runs", None, &run_value(&mutated)).await;
    assert_eq!(conflict.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn ingest_accepts_gzip_encoded_body() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = simple_run("run-gz");
    let raw = serde_json::to_vec(&run_value(&run)).unwrap();
    let compressed = gzip(&raw);
    let reply = send(&app, "POST", "/api/v1/runs", None, Some("gzip"), compressed).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(reply.json()["id"], "run-gz");

    // And it is fully queryable afterwards.
    let detail = get(&app, "/api/v1/runs/run-gz").await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.json()["case_count"], 1);
}

#[tokio::test]
async fn ingest_rejects_unsupported_schema_version() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut value = run_value(&simple_run("run-bad"));
    value["schema_version"] = serde_json::json!(999);
    let reply = post_json(&app, "/api/v1/runs", None, &value).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
}
