mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::asserts::AssertName;
use domarinn_core::result::{AssertStatus, CaseStatus};
use domarinn_server::Settings;

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

fn run_ids(body: &serde_json::Value) -> Vec<String> {
    body["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// The facet that makes a shared board usable: CI runs and developer iteration
/// are separable, and a run can be traced to a person.
#[tokio::test]
async fn list_runs_filters_by_origin_and_actor() {
    use domarinn_core::result::{CiMeta, RunOrigin};

    let (app, _dir) = test_app(Settings::default()).await;

    let mut ci_run = make_run(
        "r-ci",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    ci_run.ci = Some(CiMeta {
        provider: Some("github".into()),
        run_url: Some("https://ci.example/1".into()),
    });
    ci_run.origin = Some(RunOrigin {
        actor: Some("alice".into()),
        host: Some("runner-01".into()),
        ..Default::default()
    });

    let mut local_run = make_run(
        "r-local",
        Some("p"),
        Some("s"),
        vec![],
        Some("feat/x"),
        10,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    // `make_run` attaches CI metadata to every fixture run, so a run that is
    // meant to read as developer-local has to say so explicitly.
    local_run.ci = None;
    local_run.origin = Some(RunOrigin {
        actor: Some("bob".into()),
        host: Some("bob-laptop".into()),
        ..Default::default()
    });

    // A run from a client that predates provenance: no origin, no CI. It must
    // read as `local` rather than vanishing from both sides of the facet.
    let mut legacy = make_run(
        "r-legacy",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        20,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    legacy.ci = None;

    for run in [&ci_run, &local_run, &legacy] {
        let reply = post_json(&app, "/api/v1/runs", None, &run_value(run)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "seed {}", run.run_id);
    }

    let ci = get(&app, "/api/v1/runs?origin=ci").await;
    assert_eq!(run_ids(&ci.json()), vec!["r-ci"]);

    let local = get(&app, "/api/v1/runs?origin=local").await;
    assert_eq!(run_ids(&local.json()), vec!["r-legacy", "r-local"]);

    let alice = get(&app, "/api/v1/runs?actor=alice").await;
    assert_eq!(run_ids(&alice.json()), vec!["r-ci"]);

    let nobody = get(&app, "/api/v1/runs?actor=nobody").await;
    assert!(run_ids(&nobody.json()).is_empty());

    // The two facets compose.
    let combined = get(&app, "/api/v1/runs?origin=local&actor=bob").await;
    assert_eq!(run_ids(&combined.json()), vec!["r-local"]);
}

/// Provenance reaches the list rows, so the UI can render who ran a run without
/// opening it. These are promoted columns, not blob reads.
#[tokio::test]
async fn list_runs_carries_provenance_fields() {
    use domarinn_core::result::RunOrigin;

    let (app, _dir) = test_app(Settings::default()).await;
    let mut run = make_run(
        "r-prov",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    run.ci = None;
    run.origin = Some(RunOrigin {
        actor: Some("dana".into()),
        host: Some("dana-laptop".into()),
        version: Some("0.2.0".into()),
        note: Some("checking the tokenizer fix".into()),
        ..Default::default()
    });
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let body = get(&app, "/api/v1/runs").await.json();
    let row = &body["runs"][0];
    assert_eq!(row["actor"], "dana");
    assert_eq!(row["host"], "dana-laptop");
    assert_eq!(row["domarinn_version"], "0.2.0");
    assert_eq!(row["note"], "checking the tokenizer fix");
    assert!(row["ci_provider"].is_null());

    let detail = get(&app, "/api/v1/runs/r-prov").await.json();
    assert_eq!(detail["actor"], "dana");
    assert_eq!(detail["note"], "checking the tokenizer fix");
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
async fn unknown_query_param_is_400() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    // An unknown/typo'd query key is a hard 400 (RunQuery denies unknown
    // fields), not a silently-ignored filter.
    let bad = get(&app, "/api/v1/runs?prject=alpha").await;
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
    run.cases[0].output = Some(domarinn_core::types::Output::Text(long_output.clone()));
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
    assert_eq!(full.json()["case_key"], case_key.as_str());
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
    let restored: domarinn_core::result::RunResult = serde_json::from_slice(&export.body).unwrap();
    assert_eq!(restored.run_id.as_str(), "r-export");
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

/// Errors are aggregatable and filterable, so a run reporting "14 errors" can
/// say that twelve were rate limits and none were about the model.
#[tokio::test]
async fn cases_carry_and_filter_by_error_class() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-errs",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Error)
                .output(None)
                .error("provider error: HTTP 429")
                .error_class("provider_rate_limit"),
            CaseSpec::new("openai", "t2", CaseStatus::Error)
                .output(None)
                .error("provider error: HTTP 429")
                .error_class("provider_rate_limit"),
            CaseSpec::new("openai", "t3", CaseStatus::Error)
                .output(None)
                .error("grader returned a truncated verdict")
                .error_class("grader_failed"),
            CaseSpec::new("openai", "t4", CaseStatus::Pass),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let all = get(&app, "/api/v1/runs/r-errs/cases").await.json();
    let classes: Vec<Option<&str>> = all["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["error_class"].as_str())
        .collect();
    assert_eq!(
        classes,
        vec![
            Some("provider_rate_limit"),
            Some("provider_rate_limit"),
            Some("grader_failed"),
            // A passing case has no class, and must not be given one.
            None,
        ]
    );

    let limited = get(
        &app,
        "/api/v1/runs/r-errs/cases?error_class=provider_rate_limit",
    )
    .await;
    assert_eq!(limited.json()["cases"].as_array().unwrap().len(), 2);

    let grader = get(&app, "/api/v1/runs/r-errs/cases?error_class=grader_failed").await;
    assert_eq!(grader.json()["cases"].as_array().unwrap().len(), 1);
}

/// An unrecognized class — from a newer client, or from an `exec` child
/// domarinn did not compile — must round-trip rather than failing ingest. This
/// is why the type is an open string newtype and not an enum.
#[tokio::test]
async fn an_unknown_error_class_is_stored_not_rejected() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-future",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Error)
            .output(None)
            .error("something new")
            .error_class("invented_by_a_newer_child")],
    );
    let reply = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(reply.status, StatusCode::CREATED);

    let body = get(&app, "/api/v1/runs/r-future/cases").await.json();
    assert_eq!(body["cases"][0]["error_class"], "invented_by_a_newer_child");
}
