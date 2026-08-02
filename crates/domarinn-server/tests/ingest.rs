mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::Settings;

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
    let reparsed: domarinn_core::result::RunResult = serde_json::from_str(&text).unwrap();
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

/// The lower half of the window is a promise, not an accident: a CLI one
/// release behind must still be able to upload (`docs/concepts/architecture.md`
/// and the `supported_schema_versions` the CLI preflights against both say so).
/// Without this, narrowing ingest to the current version only would still
/// satisfy the *status* assertions in the rejection test below — every version
/// it posts is out of window either way — while quietly breaking every CLI that
/// had not yet updated. (That test's `{min}..={current}` window string would
/// catch the narrowing; the statuses alone would not, and it is the statuses
/// that describe the promise.)
#[tokio::test]
async fn ingest_accepts_one_release_back() {
    let (app, _dir) = test_app(Settings::default()).await;
    let previous = u64::from(domarinn_core::RESULT_SCHEMA_VERSION) - 1;

    let mut value = run_value(&simple_run("run-one-back"));
    value["schema_version"] = serde_json::json!(previous);
    let reply = post_json(&app, "/api/v1/runs", None, &value).await;
    assert_eq!(
        reply.status,
        StatusCode::CREATED,
        "schema_version {previous} is one release back and must still ingest: {}",
        reply.json()
    );
    assert_eq!(reply.json()["id"], "run-one-back");

    // And it is a real stored run, not just an accepted body.
    let detail = get(&app, "/api/v1/runs/run-one-back").await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(
        detail.json()["schema_version"],
        previous,
        "the row keeps the version it was uploaded with"
    );
}

/// A rejected `schema_version` is a version-skew report, and skew has a
/// direction: below the window the *uploader* is behind, above it the *server*
/// is. The bare "supported: 2..=3" the endpoint used to return told an operator
/// which numbers were legal but not which binary to upgrade — the exact gap a
/// 0.6.2 server hit when 0.7.0 started emitting `ChatRole::Tool`. Both
/// boundaries are exercised, and both versions are derived from
/// `RESULT_SCHEMA_VERSION` so a future bump moves the test with the code.
#[tokio::test]
async fn unsupported_schema_version_names_the_remedy() {
    let (app, _dir) = test_app(Settings::default()).await;
    let current = u64::from(domarinn_core::RESULT_SCHEMA_VERSION);
    let min = current.saturating_sub(1);

    for offending in [min - 1, current + 1] {
        let mut value = run_value(&simple_run("run-skew"));
        value["schema_version"] = serde_json::json!(offending);
        let reply = post_json(&app, "/api/v1/runs", None, &value).await;
        assert_eq!(
            reply.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "schema_version {offending} is outside {min}..={current}"
        );

        let error = reply.json()["error"].as_str().unwrap().to_string();
        // The number actually sent, so an operator can tell which side they are
        // on without re-reading their own request. Matched with its label, not
        // as a bare digit — the window bounds are digits in the same string.
        assert!(
            error.contains(&format!("schema_version {offending}")),
            "the 422 must echo the offending version {offending}: {error}"
        );
        assert!(
            error.contains(&format!("{min}..={current}")),
            "the 422 must state the accepted window: {error}"
        );
        // Both remedies are spelled out, keyed to the boundary that was crossed.
        assert!(
            error.contains("upgrade the CLI"),
            "below the window the uploading CLI is the old one: {error}"
        );
        assert!(
            error.contains("upgrade the server"),
            "above the window the server is the old one: {error}"
        );
    }
}
