//! HTTP-surface tests for `GET /runs/{id}/matrix` — the per-run prompt ×
//! provider aggregate that powers the UI matrix view.
//!
//! The fixture is a run shaped 2 providers × 2 prompts × 3 tests × repeat 2,
//! plus a fourth test that ran on only one provider (a `None`-hole row) and a
//! cell whose two repeats disagree on pass/fail and output (the flakiness
//! signal). Repeats of one cell are ingested out of `idx` order to prove
//! `case_keys` sort by `repeat_idx`, not insertion order.

mod common;

use std::path::Path;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::{CaseStatus, CellKey, RunResult};
use domarinn_server::Settings;
use rusqlite::Connection;

/// Open a plain read/write connection to the runs database so a test can force
/// the pre-backfill / failed-backfill states the public API can't produce.
fn raw(dir: &Path) -> Connection {
    let conn = Connection::open(dir.join("domarinn.db")).expect("open raw runs db");
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    conn
}

/// Expected `case_key` for a `(provider, prompt, test, repeat)` cell.
fn key(provider: &str, prompt: &str, test: &str, repeat: u32) -> String {
    CellKey {
        provider_id: provider.to_string(),
        prompt_id: Some(prompt.to_string()),
        test_id: test.to_string(),
        repeat,
    }
    .case_key()
    .as_str()
    .to_string()
}

/// The matrix fixture. Insertion order fixes `idx`, which drives first-seen
/// column/row order.
fn matrix_run(id: &str) -> RunResult {
    let mut specs: Vec<CaseSpec> = Vec::new();

    // Special cell t1 / openai / p-a. Repeats ingested OUT of order (repeat 1
    // before repeat 0) so the case-key ordering can't accidentally pass by
    // matching idx order. Mixed pass/fail + distinct outputs + hand-computable
    // cost/latency.
    specs.push(
        CaseSpec::new("openai", "t1", CaseStatus::Fail)
            .prompt("p-a")
            .repeat(1)
            .output(Some("B"))
            .latency(30)
            .cost(Some(0.003)),
    );
    specs.push(
        CaseSpec::new("openai", "t1", CaseStatus::Pass)
            .prompt("p-a")
            .repeat(0)
            .output(Some("A"))
            .latency(10)
            .cost(Some(0.001)),
    );

    // Rest of the t1 row: the other three columns, both repeats, all passing
    // with identical output within a cell (so distinct_outputs == 1).
    for (provider, prompt) in [
        ("openai", "p-b"),
        ("anthropic", "p-a"),
        ("anthropic", "p-b"),
    ] {
        for r in [0u32, 1] {
            specs.push(
                CaseSpec::new(provider, "t1", CaseStatus::Pass)
                    .prompt(prompt)
                    .repeat(r)
                    .output(Some("same")),
            );
        }
    }

    // t2 and t3: the full 2×2 grid, both repeats, all passing.
    for test in ["t2", "t3"] {
        for provider in ["openai", "anthropic"] {
            for prompt in ["p-a", "p-b"] {
                for r in [0u32, 1] {
                    specs.push(
                        CaseSpec::new(provider, test, CaseStatus::Pass)
                            .prompt(prompt)
                            .repeat(r)
                            .output(Some("same")),
                    );
                }
            }
        }
    }

    // t4 runs on openai only -> the anthropic columns are None holes.
    for prompt in ["p-a", "p-b"] {
        for r in [0u32, 1] {
            specs.push(
                CaseSpec::new("openai", "t4", CaseStatus::Pass)
                    .prompt(prompt)
                    .repeat(r)
                    .output(Some("same")),
            );
        }
    }

    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &specs,
    )
}

async fn ingest(app: &axum::Router, run: &RunResult) {
    let reply = post_json(app, "/api/v1/runs", None, &run_value(run)).await;
    assert_eq!(reply.status, StatusCode::CREATED);
}

#[tokio::test]
async fn matrix_columns_are_complete_first_seen_ordered_and_rows_aligned() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-mtx")).await;

    let reply = get(&app, "/api/v1/runs/r-mtx/matrix").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["run_id"], "r-mtx");

    // Columns: complete set of (provider, prompt) pairs in first-seen order.
    let columns = body["columns"].as_array().unwrap();
    let cols: Vec<(&str, Option<&str>)> = columns
        .iter()
        .map(|c| (c["provider_id"].as_str().unwrap(), c["prompt_id"].as_str()))
        .collect();
    assert_eq!(
        cols,
        vec![
            ("openai", Some("p-a")),
            ("openai", Some("p-b")),
            ("anthropic", Some("p-a")),
            ("anthropic", Some("p-b")),
        ]
    );

    // Rows: one per test, first-seen order.
    let rows = body["rows"].as_array().unwrap();
    let test_ids: Vec<&str> = rows
        .iter()
        .map(|r| r["test_id"].as_str().unwrap())
        .collect();
    assert_eq!(test_ids, vec!["t1", "t2", "t3", "t4"]);

    // Name is the first non-null case name seen for the test.
    assert_eq!(rows[0]["name"], "openai::t1");

    // Every full-grid row has a cell in all four columns.
    for row in &rows[..3] {
        let cells = row["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 4);
        assert!(cells.iter().all(|c| !c.is_null()), "no holes in {row}");
    }

    // t4 ran on openai only: anthropic columns (2, 3) are None holes.
    let t4 = &rows[3];
    let t4_cells = t4["cells"].as_array().unwrap();
    assert!(!t4_cells[0].is_null());
    assert!(!t4_cells[1].is_null());
    assert!(t4_cells[2].is_null(), "expected None hole at column 2");
    assert!(t4_cells[3].is_null(), "expected None hole at column 3");
}

#[tokio::test]
async fn matrix_collapses_repeats_with_flakiness_signals() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-mtx")).await;

    let body = get(&app, "/api/v1/runs/r-mtx/matrix").await.json();
    // t1 / openai / p-a is row 0, column 0.
    let cell = &body["rows"][0]["cells"][0];

    assert_eq!(cell["total"], 2);
    assert_eq!(cell["passed"], 1);
    assert_eq!(cell["failed"], 1);
    assert_eq!(cell["errored"], 0);
    assert_eq!(cell["skipped"], 0);
    // Mixed pass/fail pair -> pass_fraction 0.5.
    assert_eq!(cell["pass_fraction"], 0.5);
    // The two repeats produced different outputs.
    assert_eq!(cell["distinct_outputs"], 2);
    // case_keys ordered by repeat_idx (0 then 1), despite repeat 1 being
    // ingested first.
    assert_eq!(
        cell["case_keys"],
        serde_json::json!([key("openai", "p-a", "t1", 0), key("openai", "p-a", "t1", 1),])
    );

    // A uniform cell (t2 / openai / p-a): both repeats pass with identical
    // output -> one distinct output, full pass fraction.
    let uniform = &body["rows"][1]["cells"][0];
    assert_eq!(uniform["total"], 2);
    assert_eq!(uniform["passed"], 2);
    assert_eq!(uniform["pass_fraction"], 1.0);
    assert_eq!(uniform["distinct_outputs"], 1);
}

#[tokio::test]
async fn matrix_score_cost_and_latency_aggregates_are_correct() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-mtx")).await;

    let body = get(&app, "/api/v1/runs/r-mtx/matrix").await.json();
    // Hand-computed cell t1 / openai / p-a:
    //   scores  1.0 (pass) + 0.0 (fail) -> mean 0.5
    //   latency 10 + 30              -> mean 20.0
    //   cost    0.001 + 0.003        -> sum  0.004
    let cell = &body["rows"][0]["cells"][0];
    assert_eq!(cell["score_mean"], 0.5);
    assert_eq!(cell["latency_ms_mean"], 20.0);
    assert_eq!(cell["cost_usd"], 0.004);

    // A default cell sums the default 0.0025 cost across both repeats.
    let uniform = &body["rows"][1]["cells"][0];
    assert_eq!(uniform["score_mean"], 1.0);
    assert_eq!(uniform["latency_ms_mean"], 42.0);
    assert_eq!(uniform["cost_usd"], 0.005);
}

#[tokio::test]
async fn matrix_paginates_rows_while_columns_stay_complete() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-mtx")).await;

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let uri = match &cursor {
            Some(c) => format!("/api/v1/runs/r-mtx/matrix?limit=1&cursor={c}"),
            None => "/api/v1/runs/r-mtx/matrix?limit=1".to_string(),
        };
        let body = get(&app, &uri).await.json();

        // Exactly one row per page; columns never paginate.
        let rows = body["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(body["columns"].as_array().unwrap().len(), 4);
        seen.push(rows[0]["test_id"].as_str().unwrap().to_string());

        match body["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
        assert!(seen.len() <= 4, "pagination did not terminate: {seen:?}");
    }

    // Every row visited exactly once, in first-seen order.
    assert_eq!(seen, vec!["t1", "t2", "t3", "t4"]);
}

/// A three-case run that exercises answerer attribution: `openai/p-a/t1` runs
/// twice, and its second repeat was answered by `reserve` — a provider that
/// forms no column of its own, because nothing was ever configured to it.
fn fallback_run(id: &str) -> RunResult {
    let specs = vec![
        CaseSpec::new("openai", "t1", CaseStatus::Pass)
            .prompt("p-a")
            .repeat(0)
            .cost(Some(0.001)),
        CaseSpec::new("openai", "t1", CaseStatus::Pass)
            .prompt("p-a")
            .repeat(1)
            .cost(Some(0.002))
            .answered_by("reserve"),
        CaseSpec::new("anthropic", "t1", CaseStatus::Pass)
            .prompt("p-a")
            .repeat(0)
            .cost(Some(0.004)),
    ];
    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &specs,
    )
}

/// `(provider_id, cases, cost_usd)` from a matrix body, in wire order.
fn provider_costs(body: &serde_json::Value) -> Vec<(String, i64, Option<f64>)> {
    body["provider_costs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            (
                p["provider_id"].as_str().unwrap().to_string(),
                p["cases"].as_i64().unwrap(),
                p["cost_usd"].as_f64(),
            )
        })
        .collect()
}

#[tokio::test]
async fn matrix_counts_fallback_answers_and_bills_the_answering_provider() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &fallback_run("r-fb")).await;

    let body = get(&app, "/api/v1/runs/r-fb/matrix").await.json();

    // Columns stay keyed on the CONFIGURED provider: `reserve` answered a case
    // but was never configured for one, so it forms no column.
    let cols: Vec<(&str, Option<&str>)> = body["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["provider_id"].as_str().unwrap(), c["prompt_id"].as_str()))
        .collect();
    assert_eq!(
        cols,
        vec![("openai", Some("p-a")), ("anthropic", Some("p-a"))]
    );

    // The openai cell keeps both repeats and reports that one was handed off.
    let cell = &body["rows"][0]["cells"][0];
    assert_eq!(cell["total"], 2);
    assert_eq!(cell["fallback_answered"], 1);
    assert_eq!(cell["cost_usd"], 0.003);
    // The anthropic cell answered itself.
    assert_eq!(body["rows"][0]["cells"][1]["fallback_answered"], 0);

    // Cost follows whoever made the call, in first-seen order — including
    // `reserve`, which appears here and nowhere else in the matrix.
    assert_eq!(
        provider_costs(&body),
        vec![
            ("openai".to_string(), 1, Some(0.001)),
            ("reserve".to_string(), 1, Some(0.002)),
            ("anthropic".to_string(), 1, Some(0.004)),
        ],
        "billing the primary for the fallback's tokens is the bug this rollup \
         exists to prevent"
    );
}

/// The rollup describes the run, not the page: it is accumulated in the full
/// scan that precedes row pagination, so asking for one row at a time must not
/// shrink it.
#[tokio::test]
async fn matrix_provider_costs_do_not_move_with_row_pagination() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-mtx")).await;

    let full = get(&app, "/api/v1/runs/r-mtx/matrix").await.json();
    let paged = get(&app, "/api/v1/runs/r-mtx/matrix?limit=1").await.json();

    assert_eq!(paged["rows"].as_array().unwrap().len(), 1);
    assert_eq!(full["rows"].as_array().unwrap().len(), 4);
    assert_eq!(provider_costs(&paged), provider_costs(&full));
}

/// The same shape as [`fallback_run`], except the fallback's answer was
/// *skipped*: `reserve` replied, the reply matched the suite's
/// `skip_on_empty_reason`, and nothing was graded. The tokens were still spent.
fn skipped_fallback_run(id: &str) -> RunResult {
    let specs = vec![
        CaseSpec::new("openai", "t1", CaseStatus::Pass)
            .prompt("p-a")
            .repeat(0)
            .cost(Some(0.001)),
        CaseSpec::new("openai", "t1", CaseStatus::Skip)
            .prompt("p-a")
            .repeat(1)
            .cost(Some(0.002))
            .output(Some(""))
            .empty_reason("refusal")
            .answered_by("reserve"),
    ];
    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &specs,
    )
}

/// The two rollups draw the "a fallback answered" line in different places on
/// purpose, and this is the one row shape where they differ.
///
/// `fallback_answered` is a grading view: the CLI's `RunSummary.fallback_cases`
/// counts non-`Skip` cases only, and the popover renders this number next to
/// the cell's verdicts, so counting a skip here is exactly how web and CLI came
/// to report different totals for the same run. `provider_costs` is a spend
/// view and must keep the skipped row: the call was made and billed.
#[tokio::test]
async fn a_skipped_fallback_answer_is_billed_but_not_counted_as_a_handoff() {
    let (app, _dir) = test_app(Settings::default()).await;
    ingest(&app, &skipped_fallback_run("r-fb-skip")).await;

    let body = get(&app, "/api/v1/runs/r-fb-skip/matrix").await.json();

    let cell = &body["rows"][0]["cells"][0];
    assert_eq!(cell["total"], 2);
    assert_eq!(cell["skipped"], 1);
    assert_eq!(
        cell["fallback_answered"], 0,
        "the only handoff in this cell was skipped, so no graded repeat was \
         answered by a fallback — claiming one contradicts the CLI"
    );

    assert_eq!(
        provider_costs(&body),
        vec![
            ("openai".to_string(), 1, Some(0.001)),
            ("reserve".to_string(), 1, Some(0.002)),
        ],
        "spend attribution keeps the skipped row: the tokens were paid for \
         whether or not a verdict came out of them"
    );
}

/// A row stored before answerer attribution existed carries NULL, which is not
/// evidence of a handoff — it degrades to billing the configured provider
/// rather than inventing an answerer or dropping the cost entirely.
#[tokio::test]
async fn matrix_attributes_a_legacy_null_row_to_its_configured_provider() {
    // Simulates a pre-attribution row via rusqlite; a fresh Postgres
    // deployment can never contain that sqlite-legacy state.
    if common::pg::backend_is_postgres() {
        eprintln!("skipping on postgres: exercises sqlite-legacy database state");
        return;
    }
    let (app, dir) = test_app(Settings::default()).await;
    ingest(&app, &fallback_run("r-legacy")).await;

    {
        let conn = raw(dir.path());
        conn.execute(
            "UPDATE cases SET answered_by_provider_id = NULL WHERE run_id='r-legacy'",
            [],
        )
        .unwrap();
    }

    let body = get(&app, "/api/v1/runs/r-legacy/matrix").await.json();
    assert_eq!(
        provider_costs(&body),
        vec![
            ("openai".to_string(), 2, Some(0.003)),
            ("anthropic".to_string(), 1, Some(0.004)),
        ],
        "with no answerer recorded, both openai repeats bill openai"
    );
    assert_eq!(
        body["rows"][0]["cells"][0]["fallback_answered"], 0,
        "a NULL is not a handoff"
    );
}

#[tokio::test]
async fn matrix_404s_for_unknown_run() {
    let (app, _dir) = test_app(Settings::default()).await;
    let reply = get(&app, "/api/v1/runs/nope/matrix").await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert!(reply.json()["error"].is_string());
}

#[tokio::test]
async fn matrix_is_empty_for_a_run_without_cell_columns() {
    // Simulates a pre-backfill/failed-backfill run via rusqlite; a fresh
    // Postgres deployment can never contain that sqlite-legacy state.
    if common::pg::backend_is_postgres() {
        eprintln!("skipping on postgres: exercises sqlite-legacy database state");
        return;
    }
    let (app, dir) = test_app(Settings::default()).await;
    ingest(&app, &matrix_run("r-empty")).await;

    // Simulate a pre-backfill / failed-backfill run: clear the cell columns on
    // every case so none survive the matrix's provider filter.
    {
        let conn = raw(dir.path());
        conn.execute(
            "UPDATE cases SET provider_id='', prompt_id=NULL, test_id=NULL, repeat_idx=NULL
             WHERE run_id='r-empty'",
            [],
        )
        .unwrap();
    }

    let reply = get(&app, "/api/v1/runs/r-empty/matrix").await;
    // The run still exists, so this is a 200 with an empty matrix, not a 404.
    assert_eq!(reply.status, StatusCode::OK);
    let body = reply.json();
    assert_eq!(body["run_id"], "r-empty");
    assert_eq!(body["columns"].as_array().unwrap().len(), 0);
    assert_eq!(body["rows"].as_array().unwrap().len(), 0);
    assert!(body.get("next_cursor").is_some());
    assert!(body["next_cursor"].is_null());
}
