mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::Settings;

#[tokio::test]
async fn compare_classifies_case_transitions() {
    let (app, _dir) = test_app(Settings::default()).await;

    let base = make_run(
        "base",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("p", "t1", CaseStatus::Pass).output(Some("A")),
            CaseSpec::new("p", "t2", CaseStatus::Fail).output(Some("x")),
            CaseSpec::new("p", "t3", CaseStatus::Pass).output(Some("same")),
            CaseSpec::new("p", "t4", CaseStatus::Pass).output(Some("gone")),
        ],
    );
    let head = make_run(
        "head",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        5,
        &[
            CaseSpec::new("p", "t1", CaseStatus::Fail).output(Some("A")), // newly_failing, output same
            CaseSpec::new("p", "t2", CaseStatus::Pass).output(Some("x")), // newly_passing
            CaseSpec::new("p", "t3", CaseStatus::Pass).output(Some("different")), // still_passing + output_changed
            CaseSpec::new("p", "t5", CaseStatus::Pass).output(Some("new")),       // added
                                                                                  // t4 removed
        ],
    );

    post_json(&app, "/api/v1/runs", None, &run_value(&base)).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&head)).await;

    let cmp = get(&app, "/api/v1/runs/base/compare/head").await;
    assert_eq!(cmp.status, StatusCode::OK);
    let body = cmp.json();
    assert_eq!(body["base"], "base");
    assert_eq!(body["head"], "head");

    let summary = &body["summary"];
    assert_eq!(summary["newly_failing"], 1, "summary={summary}");
    assert_eq!(summary["newly_passing"], 1);
    assert_eq!(summary["still_failing"], 0);
    assert_eq!(summary["output_changed"], 1);
    assert_eq!(summary["added"], 1);
    assert_eq!(summary["removed"], 1);

    // Spot-check per-case deltas by case_key.
    let key = |provider: &str, test: &str| {
        let cell = domarinn_core::result::CellKey {
            provider_id: provider.to_string(),
            prompt_id: None,
            test_id: test.to_string(),
            repeat: 0,
        };
        cell.case_key()
    };
    let cases = body["cases"].as_array().unwrap();
    let find = |ck: &str| cases.iter().find(|c| c["case_key"] == ck).unwrap().clone();

    let t1 = find(key("p", "t1").as_str());
    assert_eq!(t1["delta"], "newly_failing");
    assert_eq!(t1["base_status"], "pass");
    assert_eq!(t1["head_status"], "fail");
    assert_eq!(t1["output_changed"], false);

    let t3 = find(key("p", "t3").as_str());
    assert_eq!(t3["delta"], "still_passing");
    assert_eq!(t3["output_changed"], true);

    let t4 = find(key("p", "t4").as_str());
    assert_eq!(t4["delta"], "removed");
    assert!(t4["head_status"].is_null());

    let t5 = find(key("p", "t5").as_str());
    assert_eq!(t5["delta"], "added");
    assert!(t5["base_status"].is_null());
}

#[tokio::test]
async fn compare_missing_run_is_404() {
    let (app, _dir) = test_app(Settings::default()).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("only"))).await;
    let cmp = get(&app, "/api/v1/runs/only/compare/ghost").await;
    assert_eq!(cmp.status, StatusCode::NOT_FOUND);
}
