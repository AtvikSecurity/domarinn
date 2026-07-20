mod common;

use axum::http::StatusCode;
use common::*;
use measurellm_core::asserts::AssertName;
use measurellm_core::result::{AssertStatus, CaseStatus};
use measurellm_server::Settings;

async fn seed(app: &axum::Router) {
    // Three runs across two projects/branches with distinct timestamps.
    let runs = [
        make_run(
            "r-alpha-1",
            Some("alpha"),
            Some("suiteA"),
            vec!["nightly"],
            Some("main"),
            0,
            &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
        ),
        make_run(
            "r-alpha-2",
            Some("alpha"),
            Some("suiteA"),
            vec!["release"],
            Some("feature"),
            10,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Fail),
            ],
        ),
        make_run(
            "r-beta-1",
            Some("beta"),
            Some("suiteB"),
            vec!["nightly"],
            Some("main"),
            20,
            &[CaseSpec::new("anthropic", "t1", CaseStatus::Error)],
        ),
    ];
    for run in &runs {
        let reply = post_json(app, "/api/v1/runs", None, &run_value(run)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "seed {}", run.run_id);
    }
}

#[tokio::test]
async fn list_runs_orders_newest_first_and_filters_by_project() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let all = get(&app, "/api/v1/runs").await;
    let ids: Vec<String> = all.json()["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, vec!["r-beta-1", "r-alpha-2", "r-alpha-1"]);

    let alpha = get(&app, "/api/v1/runs?project=alpha").await;
    assert_eq!(alpha.json()["runs"].as_array().unwrap().len(), 2);

    let branch = get(&app, "/api/v1/runs?branch=feature").await;
    let branch_body = branch.json();
    let branch_ids: Vec<&str> = branch_body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(branch_ids, vec!["r-alpha-2"]);

    let tag = get(&app, "/api/v1/runs?tag=release").await;
    assert_eq!(tag.json()["runs"].as_array().unwrap().len(), 1);

    // status=fail returns runs with any failing case.
    let failing = get(&app, "/api/v1/runs?status=fail").await;
    let failing_body = failing.json();
    let fail_ids: Vec<&str> = failing_body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(fail_ids, vec!["r-alpha-2"]);
}

#[tokio::test]
async fn invalid_status_query_is_400_with_json_error() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let bad = get(&app, "/api/v1/runs?status=bogus").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json()["error"].is_string(), "body: {:?}", bad.json());
}

#[tokio::test]
async fn list_runs_paginates_by_cursor() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let page1 = get(&app, "/api/v1/runs?limit=2").await;
    let runs1 = page1.json()["runs"].as_array().unwrap().clone();
    assert_eq!(runs1.len(), 2);
    let cursor = page1.json()["next_cursor"].as_str().unwrap().to_string();

    let page2 = get(&app, &format!("/api/v1/runs?limit=2&cursor={cursor}")).await;
    let runs2 = page2.json()["runs"].as_array().unwrap().clone();
    assert_eq!(runs2.len(), 1);
    assert_eq!(runs2[0]["id"], "r-alpha-1");
    assert!(page2.json()["next_cursor"].is_null());
}

#[tokio::test]
async fn run_detail_reports_assert_labels() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-labels",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass).asserts(vec![
                (AssertName::Contains, AssertStatus::Pass),
                (AssertName::Regex, AssertStatus::Pass),
            ]),
            CaseSpec::new("openai", "t2", CaseStatus::Fail)
                .asserts(vec![(AssertName::LlmRubric, AssertStatus::Fail)]),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let detail = get(&app, "/api/v1/runs/r-labels").await;
    assert_eq!(detail.status, StatusCode::OK);
    let labels: Vec<String> = detail.json()["assert_labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(labels, vec!["contains", "llm-rubric", "regex"]);
}

#[tokio::test]
async fn cases_are_lean_but_detail_is_full() {
    let (app, _dir) = test_app(Settings::default()).await;
    let long_output = "x".repeat(500);
    // Build a run whose single case has a long output so we can verify preview truncation.
    let mut run = simple_run("r-cases");
    run.cases[0].output = Some(measurellm_core::types::Output::Text(long_output.clone()));
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let case_key = run.cases[0].case_key.clone();

    let lean = get(&app, "/api/v1/runs/r-cases/cases").await;
    assert_eq!(lean.status, StatusCode::OK);
    let cases = lean.json()["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 1);
    let preview = cases[0]["output_preview"].as_str().unwrap();
    assert_eq!(preview.chars().count(), 300);
    // Lean rows carry asserts json but not the detail/full output.
    assert!(cases[0]["asserts"].is_array());
    assert!(cases[0].get("output").is_none());

    let full = get(&app, &format!("/api/v1/runs/r-cases/cases/{case_key}")).await;
    assert_eq!(full.status, StatusCode::OK);
    // The full detail decompresses the original CaseResult (untagged Output::Text).
    assert_eq!(full.json()["output"], serde_json::json!(long_output));
    assert_eq!(full.json()["case_key"], case_key);
}

#[tokio::test]
async fn cases_filter_by_status() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-mix",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("openai", "t2", CaseStatus::Fail),
            CaseSpec::new("openai", "t3", CaseStatus::Fail),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let failing = get(&app, "/api/v1/runs/r-mix/cases?status=fail").await;
    assert_eq!(failing.json()["cases"].as_array().unwrap().len(), 2);

    // An invalid status value is a 400, not a silently-ignored filter.
    let bad = get(&app, "/api/v1/runs/r-mix/cases?status=bogus").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json()["error"].is_string(), "body: {:?}", bad.json());
}

#[tokio::test]
async fn export_returns_original_document() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = simple_run("r-export");
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let export = get(&app, "/api/v1/runs/r-export/export").await;
    assert_eq!(export.status, StatusCode::OK);
    // Round-trips back into a RunResult identical to what we sent.
    let restored: measurellm_core::result::RunResult =
        serde_json::from_slice(&export.body).unwrap();
    assert_eq!(restored.run_id, "r-export");
    assert_eq!(restored.summary.total, 1);
    assert_eq!(restored.cases.len(), 1);
}

#[tokio::test]
async fn missing_run_is_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    assert_eq!(
        get(&app, "/api/v1/runs/nope").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/v1/runs/nope/cases").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, "/api/v1/runs/nope/export").await.status,
        StatusCode::NOT_FOUND
    );
}
