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

/// Pins run-ingest content-hash idempotency against serde drift on the id
/// fields specifically: a run document is ingested, then re-derived through a
/// full serialize -> parse back into `RunResult` -> serialize round trip (the
/// same path a client that writes/reads `result.json`, or the RunId/CaseKey
/// newtype refactor, would exercise) and posted again. The round trip must
/// produce byte-for-byte identical canonical JSON, so the second post must be
/// recognized as the same content (200 "existing"), never 409 conflict.
#[tokio::test]
async fn ingest_twice_survives_a_serialize_deserialize_round_trip() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "run-roundtrip-guard",
        Some("proj"),
        Some("suite"),
        vec!["nightly"],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("anthropic", "t2", CaseStatus::Fail),
        ],
    );

    let first_body = run_value(&run);
    let first = post_json(&app, "/api/v1/runs", None, &first_body).await;
    assert_eq!(first.status, StatusCode::CREATED);

    // Round-trip through RunResult, exactly like a stored-then-reloaded run.
    let text = serde_json::to_string(&first_body).unwrap();
    let reparsed: measurellm_core::result::RunResult = serde_json::from_str(&text).unwrap();
    let second_body = run_value(&reparsed);
    assert_eq!(
        first_body, second_body,
        "round-trip must be byte-for-byte identical JSON"
    );

    let second = post_json(&app, "/api/v1/runs", None, &second_body).await;
    assert_eq!(
        second.status,
        StatusCode::OK,
        "round-tripped re-ingest must be Existing, not Conflict"
    );
    assert_eq!(second.json()["id"], "run-roundtrip-guard");

    // Still exactly one run stored.
    let list = get(&app, "/api/v1/runs").await;
    assert_eq!(list.json()["runs"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ingest_rejects_unsupported_schema_version() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut value = run_value(&simple_run("run-bad"));
    value["schema_version"] = serde_json::json!(999);
    let reply = post_json(&app, "/api/v1/runs", None, &value).await;
    assert_eq!(reply.status, StatusCode::UNPROCESSABLE_ENTITY);
}
