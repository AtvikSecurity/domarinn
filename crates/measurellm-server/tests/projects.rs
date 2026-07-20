mod common;

use axum::http::StatusCode;
use common::*;
use measurellm_core::result::CaseStatus;
use measurellm_server::Settings;
use serde_json::json;

async fn seed(app: &axum::Router) {
    let runs = [
        make_run(
            "p-1",
            Some("proj"),
            Some("suite"),
            vec![],
            Some("main"),
            0,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Fail),
            ],
        ),
        make_run(
            "p-2",
            Some("proj"),
            Some("suite"),
            vec![],
            Some("main"),
            10,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Pass),
            ],
        ),
    ];
    for run in &runs {
        post_json(app, "/api/v1/runs", None, &run_value(run)).await;
    }
}

#[tokio::test]
async fn projects_and_suites_are_listed_with_series() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    let projects = get(&app, "/api/v1/projects").await;
    assert_eq!(projects.status, StatusCode::OK);
    let list = projects.json()["projects"].as_array().unwrap().clone();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["project"], "proj");
    assert_eq!(list[0]["run_count"], 2);
    assert_eq!(list[0]["suite_count"], 1);

    let suites = get(&app, "/api/v1/projects/proj/suites").await;
    assert_eq!(suites.status, StatusCode::OK);
    let suite_list = suites.json()["suites"].as_array().unwrap().clone();
    assert_eq!(suite_list.len(), 1);
    assert_eq!(suite_list[0]["suite"], "suite");
    let series = suite_list[0]["series"].as_array().unwrap();
    assert_eq!(series.len(), 2);
    // Series is newest-first; p-2 (100% pass) then p-1 (50% pass).
    assert_eq!(series[0]["run_id"], "p-2");
    assert_eq!(series[0]["pass_rate"], 1.0);
    assert_eq!(series[1]["run_id"], "p-1");
    assert_eq!(series[1]["pass_rate"], 0.5);
}

#[tokio::test]
async fn baseline_set_get_and_delete() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;

    // Set baseline to p-1.
    let set = send(
        &app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&json!({ "run_id": "p-1" })).unwrap(),
    )
    .await;
    assert_eq!(set.status, StatusCode::OK);
    assert_eq!(set.json()["run_id"], "p-1");

    // It surfaces in the suites listing.
    let suites = get(&app, "/api/v1/projects/proj/suites").await;
    assert_eq!(suites.json()["suites"][0]["baseline_run_id"], "p-1");

    // Delete it.
    let del = send(
        &app,
        "DELETE",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(del.status, StatusCode::NO_CONTENT);

    let after = get(&app, "/api/v1/projects/proj/suites").await;
    assert!(after.json()["suites"][0]["baseline_run_id"].is_null());
}

#[tokio::test]
async fn baseline_for_missing_run_is_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let set = send(
        &app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&json!({ "run_id": "ghost" })).unwrap(),
    )
    .await;
    assert_eq!(set.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_run_removes_it() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed(&app).await;
    let del = send(&app, "DELETE", "/api/v1/runs/p-1", None, None, Vec::new()).await;
    assert_eq!(del.status, StatusCode::NO_CONTENT);
    assert_eq!(
        get(&app, "/api/v1/runs/p-1").await.status,
        StatusCode::NOT_FOUND
    );
    // Deleting again -> 404.
    let again = send(&app, "DELETE", "/api/v1/runs/p-1", None, None, Vec::new()).await;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
}
