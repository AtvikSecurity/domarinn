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

/// The expected-failure statuses, end to end: a v4 run carrying them ingests
/// (the widened CHECK admits the rows), the run row promotes the counters,
/// `?status=xfail|xpass` filters cases, re-upload stays idempotent, and the
/// run-level `status` filter treats an xpass-only run as failing — strict
/// expect_fail is a property of the data, not just of the CLI's exit code.
#[tokio::test]
async fn expected_failure_statuses_ingest_filter_and_fail_the_run_level_filter() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-xf",
        Some("alpha"),
        Some("suiteA"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("openai", "t2", CaseStatus::XFail),
            CaseSpec::new("openai", "t3", CaseStatus::XPass),
        ],
    );
    let reply = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(reply.status, StatusCode::CREATED);
    // Idempotent re-upload: same content, same outcome, no 409.
    let again = post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.json());

    // The list row carries the promoted counters.
    let list = get(&app, "/api/v1/runs").await;
    let row = &list.json()["runs"][0];
    assert_eq!(row["xfail_count"], 1, "{row}");
    assert_eq!(row["xpass_count"], 1, "{row}");

    // Case-level status filters, for free via the widened FromStr/CHECK.
    let xfails = get(&app, "/api/v1/runs/r-xf/cases?status=xfail").await;
    let cases = xfails.json()["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0]["status"], "xfail");
    let xpasses = get(&app, "/api/v1/runs/r-xf/cases?status=xpass").await;
    assert_eq!(xpasses.json()["cases"].as_array().unwrap().len(), 1);

    // Run-level: an xpass makes the run failing, never passing — even though
    // its fail_count is 0.
    assert_eq!(row["fail_count"], 0, "{row}");
    let failing = get(&app, "/api/v1/runs?status=fail").await;
    assert_eq!(run_ids(&failing.json()), vec!["r-xf"]);
    let passing = get(&app, "/api/v1/runs?status=pass").await;
    assert!(run_ids(&passing.json()).is_empty());
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

/// `?error_class=unknown` is the UI's bucket for every case the breakdown
/// could not classify: NULL (stored before the column existed, or an error
/// with no class) and the empty string (`ErrorClass` is unvalidated, and an
/// exec child can emit `""`). The chip counts both, so the filter must match
/// both — it used to match only NULL, and clicking a `unknown × 1` chip backed
/// by `""` returned zero cases.
#[tokio::test]
async fn the_unknown_filter_matches_null_and_empty_classes() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-unknown",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Error)
                .output(None)
                .error("errored before classes existed"),
            CaseSpec::new("openai", "t2", CaseStatus::Error)
                .output(None)
                .error("child sent an empty class")
                .error_class(""),
            CaseSpec::new("openai", "t3", CaseStatus::Error)
                .output(None)
                .error("classified")
                .error_class("provider_timeout"),
            CaseSpec::new("openai", "t4", CaseStatus::Pass),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let unknown = get(&app, "/api/v1/runs/r-unknown/cases?error_class=unknown").await;
    let cases = unknown.json()["cases"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 2, "NULL and \"\" are the same absence");
    for c in &cases {
        assert_eq!(c["status"], "error");
    }
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

/// Empty outputs are aggregatable and filterable the same way errors are: a run
/// reporting "14 cases came back empty" has to be able to say that twelve were
/// refusals. The reason set is open — no `unknown` bucket, unlike `error_class`
/// — so an unlisted value must store and filter verbatim.
#[tokio::test]
async fn cases_carry_and_filter_by_empty_reason() {
    let (app, _dir) = test_app(Settings::default()).await;
    let run = make_run(
        "r-empty",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("refusal"),
            CaseSpec::new("openai", "t2", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("refusal"),
            CaseSpec::new("openai", "t3", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("tool_use_only"),
            // An unlisted reason — from a newer client, or an `exec` child this
            // build does not know about — round-trips rather than failing.
            CaseSpec::new("openai", "t4", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("weird_new_reason"),
            CaseSpec::new("openai", "t5", CaseStatus::Pass),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&run)).await;

    let all = get(&app, "/api/v1/runs/r-empty/cases").await.json();
    let reasons: Vec<Option<&str>> = all["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["empty_reason"].as_str())
        .collect();
    assert_eq!(
        reasons,
        vec![
            Some("refusal"),
            Some("refusal"),
            Some("tool_use_only"),
            Some("weird_new_reason"),
            // A case that produced real output is not empty, and must not be
            // given a reason.
            None,
        ]
    );

    let refusals = get(&app, "/api/v1/runs/r-empty/cases?empty_reason=refusal").await;
    assert_eq!(refusals.json()["cases"].as_array().unwrap().len(), 2);

    let weird = get(
        &app,
        "/api/v1/runs/r-empty/cases?empty_reason=weird_new_reason",
    )
    .await;
    assert_eq!(weird.json()["cases"].as_array().unwrap().len(), 1);

    // A blank filter value must return nothing, not the complement: `''` is the
    // storage sentinel for "known: not empty", and it never appears on the wire,
    // so it must not be selectable through the wire either.
    let blank = get(&app, "/api/v1/runs/r-empty/cases?empty_reason=").await;
    assert!(
        blank.json()["cases"].as_array().unwrap().is_empty(),
        "a blank empty_reason must not select the not-empty sentinel rows: {:?}",
        blank.json()
    );
}

/// The run-level empty tally has to reach the wire in two shapes: one number on
/// the list row (the `runs.empty_count` column) and the per-reason map on the
/// detail (the stored `summary.empty_counts`). Both are omitted, never zeroed,
/// when there is nothing to report — a dashboard must not invent "0 empty" for
/// a legacy row whose tally was never recorded.
#[tokio::test]
async fn runs_report_empty_count_and_empty_counts() {
    let (app, dir) = test_app(Settings::default()).await;

    let empties = make_run(
        "r-tally",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("refusal"),
            CaseSpec::new("openai", "t2", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("refusal"),
            CaseSpec::new("openai", "t3", CaseStatus::Pass),
        ],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&empties)).await;

    let clean = make_run(
        "r-noempty",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        10,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    post_json(&app, "/api/v1/runs", None, &run_value(&clean)).await;

    let listed = get(&app, "/api/v1/runs").await.json();
    let row = |id: &str| -> serde_json::Value {
        listed["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap_or_else(|| panic!("no row for {id} in {listed}"))
            .clone()
    };
    assert_eq!(row("r-tally")["empty_count"], serde_json::json!(2));
    assert!(
        row("r-noempty").get("empty_count").is_none(),
        "a run with no empty cases omits the key: {}",
        row("r-noempty")
    );

    let detail = get(&app, "/api/v1/runs/r-tally").await.json();
    assert_eq!(
        detail["empty_counts"],
        serde_json::json!({ "refusal": 2 }),
        "detail tallies by reason: {detail}"
    );
    let clean_detail = get(&app, "/api/v1/runs/r-noempty").await.json();
    assert!(
        clean_detail.get("empty_counts").is_none(),
        "a run with no empty cases omits the map: {clean_detail}"
    );

    // Unknown is not zero. A legacy row (NULL) and an undecodable blob (the -1
    // sentinel) must both render as absent rather than as a number. This tail
    // seeds those rows via rusqlite; a fresh Postgres deployment can never
    // contain that sqlite-legacy state, so only the tally half runs there.
    if common::pg::backend_is_postgres() {
        eprintln!("skipping legacy tail on postgres: exercises sqlite-legacy database state");
        return;
    }
    let db = rusqlite::Connection::open(dir.path().join("domarinn.db")).unwrap();
    db.execute(
        "UPDATE runs SET empty_count = NULL WHERE id = 'r-tally'",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE runs SET empty_count = -1 WHERE id = 'r-noempty'",
        [],
    )
    .unwrap();
    drop(db);

    let listed = get(&app, "/api/v1/runs").await.json();
    for id in ["r-tally", "r-noempty"] {
        let row = listed["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap();
        assert!(
            row.get("empty_count").is_none(),
            "unknown must not render as a number for {id}: {row}"
        );
    }
}

/// `empty_reason` is an open string, so nothing stops a hand-authored document
/// from sending `""` — and `""` is exactly the storage sentinel for "known: not
/// empty". Storage flattens it to that sentinel, which means the detail
/// `GROUP BY`, the case grid and the wire filter all drop the case; the list
/// count is the one place that could disagree, and must not. A present-but-blank
/// reason is not a reason.
#[tokio::test]
async fn a_blank_empty_reason_counts_as_not_empty_everywhere() {
    let (app, _dir) = test_app(Settings::default()).await;

    let run = make_run(
        "r-blank",
        Some("p"),
        Some("s"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Skip)
                .output(Some(""))
                .empty_reason("refusal"),
            CaseSpec::new("openai", "t2", CaseStatus::Skip).output(Some("")),
            CaseSpec::new("openai", "t3", CaseStatus::Pass),
        ],
    );
    let mut value = run_value(&run);
    // The hand-authored part: a reason field that is present and blank.
    value["cases"][1]["empty_reason"] = serde_json::json!("");
    let reply = post_json(&app, "/api/v1/runs", None, &value).await;
    assert_eq!(reply.status, StatusCode::CREATED);

    let listed = get(&app, "/api/v1/runs").await.json();
    let row = listed["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "r-blank")
        .unwrap_or_else(|| panic!("no row for r-blank in {listed}"));
    assert_eq!(
        row["empty_count"],
        serde_json::json!(1),
        "the blank reason must not be counted: {row}"
    );

    // The other three readers, which the count has to agree with.
    let detail = get(&app, "/api/v1/runs/r-blank").await.json();
    assert_eq!(
        detail["empty_counts"],
        serde_json::json!({ "refusal": 1 }),
        "detail must omit the blank reason: {detail}"
    );

    let cases = get(&app, "/api/v1/runs/r-blank/cases").await.json();
    let reasons: Vec<Option<&str>> = cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["empty_reason"].as_str())
        .collect();
    assert_eq!(
        reasons,
        vec![Some("refusal"), None, None],
        "a blank reason reads back as absent, never as \"\": {cases}"
    );

    let blank = get(&app, "/api/v1/runs/r-blank/cases?empty_reason=").await;
    assert!(
        blank.json()["cases"].as_array().unwrap().is_empty(),
        "the blank reason must not be selectable: {:?}",
        blank.json()
    );
}
