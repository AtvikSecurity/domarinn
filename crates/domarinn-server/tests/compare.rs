mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::asserts::AssertName;
use domarinn_core::result::{AssertStatus, CaseStatus};
use domarinn_server::Settings;
use serde_json::{json, Value};

/// Resolve a `CellKey` case_key the way the fixtures do (provider/test, no
/// prompt, repeat 0).
fn case_key(provider: &str, test: &str) -> String {
    domarinn_core::result::CellKey {
        provider_id: provider.to_string(),
        prompt_id: None,
        test_id: test.to_string(),
        repeat: 0,
    }
    .case_key()
    .as_str()
    .to_string()
}

/// Serialize a run and override its `config_digest` field on the wire (ingest
/// stores the digest verbatim), so tests can exercise config-drift scenarios.
fn run_value_with_digest(run: &domarinn_core::result::RunResult, digest: &str) -> Value {
    let mut v = run_value(run);
    v["config_digest"] = json!(digest);
    v
}

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

#[tokio::test]
async fn compare_enriches_scores_stats_and_totals() {
    let (app, _dir) = test_app(Settings::default()).await;

    // 1 regression (t1), 2 fixes (t2, t3), 1 still-passing (t4), plus a removed
    // (t5) and an added (t6) case.
    let base = make_run(
        "b1",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("p", "t1", CaseStatus::Pass),
            CaseSpec::new("p", "t2", CaseStatus::Fail),
            CaseSpec::new("p", "t3", CaseStatus::Fail),
            CaseSpec::new("p", "t4", CaseStatus::Pass),
            CaseSpec::new("p", "t5", CaseStatus::Pass),
        ],
    );
    let head = make_run(
        "h1",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        5,
        &[
            CaseSpec::new("p", "t1", CaseStatus::Fail),
            CaseSpec::new("p", "t2", CaseStatus::Pass),
            CaseSpec::new("p", "t3", CaseStatus::Pass),
            CaseSpec::new("p", "t4", CaseStatus::Pass),
            CaseSpec::new("p", "t6", CaseStatus::Pass),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&base)).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&head)).await;

    let cmp = get(&app, "/api/v1/runs/b1/compare/h1").await;
    assert_eq!(cmp.status, StatusCode::OK);
    let body = cmp.json();

    let cases = body["cases"].as_array().unwrap();
    let find = |ck: String| {
        cases
            .iter()
            .find(|c| c["case_key"] == ck)
            .unwrap_or_else(|| panic!("case {ck} not found"))
            .clone()
    };

    // Score deltas: head - base, only when both present.
    let t1 = find(case_key("p", "t1"));
    assert_eq!(t1["base_score"], 1.0);
    assert_eq!(t1["head_score"], 0.0);
    assert_eq!(t1["score_delta"], -1.0);

    let t2 = find(case_key("p", "t2"));
    assert_eq!(t2["base_score"], 0.0);
    assert_eq!(t2["head_score"], 1.0);
    assert_eq!(t2["score_delta"], 1.0);

    // Removed case: no head side → score_delta is null.
    let t5 = find(case_key("p", "t5"));
    assert_eq!(t5["delta"], "removed");
    assert_eq!(t5["base_score"], 1.0);
    assert!(t5["head_score"].is_null());
    assert!(t5["score_delta"].is_null());

    // Added case: no base side → score_delta is null.
    let t6 = find(case_key("p", "t6"));
    assert_eq!(t6["delta"], "added");
    assert!(t6["base_score"].is_null());
    assert_eq!(t6["head_score"], 1.0);
    assert!(t6["score_delta"].is_null());

    // McNemar is fed (regressions=1, fixes=2).
    let mcnemar = &body["stats"]["mcnemar"];
    assert_eq!(mcnemar["regressions"], 1, "stats={}", body["stats"]);
    assert_eq!(mcnemar["fixes"], 2);

    // Wilson rates match the runs' pass/case counts (base 3/5, head 4/5).
    let base_pr = &body["stats"]["base_pass_rate"];
    assert_eq!(base_pr["passed"], 3);
    assert_eq!(base_pr["total"], 5);
    assert!((base_pr["rate"].as_f64().unwrap() - 0.6).abs() < 1e-9);
    let head_pr = &body["stats"]["head_pass_rate"];
    assert_eq!(head_pr["passed"], 4);
    assert_eq!(head_pr["total"], 5);
    assert!((head_pr["rate"].as_f64().unwrap() - 0.8).abs() < 1e-9);

    // Totals match the ingested aggregates (10 in / 20 out tokens per case,
    // $0.0025 per case, 5 cases, 30s duration).
    let base_totals = &body["totals"]["base"];
    assert_eq!(base_totals["prompt_tokens"], 50);
    assert_eq!(base_totals["completion_tokens"], 100);
    assert_eq!(base_totals["case_count"], 5);
    assert_eq!(base_totals["pass_count"], 3);
    assert_eq!(base_totals["duration_ms"], 30_000);
    assert!((base_totals["cost_usd"].as_f64().unwrap() - 0.0125).abs() < 1e-9);
    assert_eq!(body["totals"]["head"]["pass_count"], 4);
}

#[tokio::test]
async fn compare_reports_assert_flips_and_skips_unpairable() {
    let (app, _dir) = test_app(Settings::default()).await;

    // `cflip`: single Contains assert flips Pass→Fail (pairs positionally).
    // `unpair`: base has two Contains asserts, head has one → the kind occurs a
    // different number of times on each side, so it can't be paired and emits
    // no flip even though the case's overall status changed.
    let base = make_run(
        "ab",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("p", "cflip", CaseStatus::Pass)
                .asserts(vec![(AssertName::Contains, AssertStatus::Pass)]),
            CaseSpec::new("p", "unpair", CaseStatus::Fail).asserts(vec![
                (AssertName::Contains, AssertStatus::Fail),
                (AssertName::Contains, AssertStatus::Fail),
            ]),
        ],
    );
    let head = make_run(
        "ah",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        5,
        &[
            CaseSpec::new("p", "cflip", CaseStatus::Fail)
                .asserts(vec![(AssertName::Contains, AssertStatus::Fail)]),
            CaseSpec::new("p", "unpair", CaseStatus::Pass)
                .asserts(vec![(AssertName::Contains, AssertStatus::Pass)]),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&base)).await;
    post_json(&app, "/api/v1/runs", None, &run_value(&head)).await;

    let body = get(&app, "/api/v1/runs/ab/compare/ah").await.json();
    let cases = body["cases"].as_array().unwrap();
    let find = |ck: String| cases.iter().find(|c| c["case_key"] == ck).unwrap().clone();

    let cflip = find(case_key("p", "cflip"));
    let flips = cflip["assert_flips"].as_array().unwrap();
    assert_eq!(flips.len(), 1, "cflip flips={:?}", cflip["assert_flips"]);
    assert_eq!(flips[0]["kind"], "contains");
    assert_eq!(flips[0]["base_passed"], true);
    assert_eq!(flips[0]["head_passed"], false);
    assert_eq!(flips[0]["base_score"], 1.0);
    assert_eq!(flips[0]["head_score"], 0.0);

    let unpair = find(case_key("p", "unpair"));
    assert_eq!(
        unpair["assert_flips"],
        json!([]),
        "unpairable kind-sequences must emit no flips"
    );
}

#[tokio::test]
async fn compare_reports_config_drift() {
    let (app, _dir) = test_app(Settings::default()).await;

    let a = make_run("cfg_a", Some("p"), Some("s"), vec![], Some("main"), 0, &[]);
    let b = make_run("cfg_b", Some("p"), Some("s"), vec![], Some("main"), 1, &[]);
    let c = make_run("cfg_c", Some("p"), Some("s"), vec![], Some("main"), 2, &[]);
    let e = make_run("cfg_e", Some("p"), Some("s"), vec![], Some("main"), 3, &[]);

    post_json(
        &app,
        "/api/v1/runs",
        None,
        &run_value_with_digest(&a, "sha256:aaa"),
    )
    .await;
    post_json(
        &app,
        "/api/v1/runs",
        None,
        &run_value_with_digest(&b, "sha256:bbb"),
    )
    .await;
    post_json(
        &app,
        "/api/v1/runs",
        None,
        &run_value_with_digest(&c, "sha256:aaa"),
    )
    .await;
    // Empty digest ingests as the sentinel that reads back as `None`.
    post_json(&app, "/api/v1/runs", None, &run_value_with_digest(&e, "")).await;

    // Differing digests → changed: true.
    let drift = get(&app, "/api/v1/runs/cfg_a/compare/cfg_b").await.json();
    assert_eq!(drift["config"]["base_digest"], "sha256:aaa");
    assert_eq!(drift["config"]["head_digest"], "sha256:bbb");
    assert_eq!(drift["config"]["changed"], true);

    // Identical digests → changed: false.
    let same = get(&app, "/api/v1/runs/cfg_a/compare/cfg_c").await.json();
    assert_eq!(same["config"]["changed"], false);

    // One digest unknown → changed: null.
    let unknown = get(&app, "/api/v1/runs/cfg_a/compare/cfg_e").await.json();
    assert_eq!(unknown["config"]["base_digest"], "sha256:aaa");
    assert!(unknown["config"]["head_digest"].is_null());
    assert!(unknown["config"]["changed"].is_null());
}
