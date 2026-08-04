//! The `cached` run/case filters: fully-cached passing runs can be excluded
//! from `GET /runs` (CI noise), cache counts are exposed on run rows/detail,
//! and `GET /runs/{id}/cases` can filter to fresh (non-cached) responses.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::{build_app, AuthMode, ServerConfig, Settings};

/// Four runs spanning the cache spectrum (newest first: partial, cached-fail,
/// cached-pass, fresh):
/// - `r-fresh` — no cache hits, passing
/// - `r-cached-pass` — every provider call cached, passing (the CI-noise case)
/// - `r-cached-fail` — every provider call cached, but a failing verdict
/// - `r-partial` — one hit + one miss, passing
async fn seed_cache_mix(app: &axum::Router) {
    let runs = [
        make_run(
            "r-fresh",
            Some("alpha"),
            Some("suiteA"),
            vec![],
            Some("main"),
            0,
            &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
        ),
        make_run(
            "r-cached-pass",
            Some("alpha"),
            Some("suiteA"),
            vec![],
            Some("main"),
            10,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass).cached(true),
                CaseSpec::new("openai", "t2", CaseStatus::Pass).cached(true),
            ],
        ),
        make_run(
            "r-cached-fail",
            Some("alpha"),
            Some("suiteA"),
            vec![],
            Some("main"),
            20,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass).cached(true),
                CaseSpec::new("openai", "t2", CaseStatus::Fail).cached(true),
            ],
        ),
        make_run(
            "r-partial",
            Some("alpha"),
            Some("suiteA"),
            vec![],
            Some("main"),
            30,
            &[
                CaseSpec::new("openai", "t1", CaseStatus::Pass).cached(true),
                CaseSpec::new("openai", "t2", CaseStatus::Pass),
            ],
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

#[tokio::test]
async fn run_list_and_detail_expose_cache_counts() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let all = get(&app, "/api/v1/runs").await;
    let body = all.json();
    let runs = body["runs"].as_array().unwrap();
    let partial = runs
        .iter()
        .find(|r| r["id"] == "r-partial")
        .expect("r-partial listed");
    assert_eq!(partial["cache_hits"].as_i64(), Some(1));
    assert_eq!(partial["cache_misses"].as_i64(), Some(1));
    let fresh = runs
        .iter()
        .find(|r| r["id"] == "r-fresh")
        .expect("r-fresh listed");
    assert_eq!(fresh["cache_hits"].as_i64(), Some(0));
    assert_eq!(fresh["cache_misses"].as_i64(), Some(1));

    let detail = get(&app, "/api/v1/runs/r-cached-pass").await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.json()["cache_hits"].as_i64(), Some(2));
    assert_eq!(detail.json()["cache_misses"].as_i64(), Some(0));
}

/// The migration-12 columns: promoted so the run header can render them without
/// decompressing a blob, which is the only reason they were promoted at all.
#[tokio::test]
async fn run_detail_exposes_cache_tokens_and_the_second_cost_figure() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let detail = get(&app, "/api/v1/runs/r-cached-pass").await.json();
    assert_eq!(detail["cache_read_tokens"].as_i64(), Some(1_040));
    assert_eq!(detail["cache_write_tokens"].as_i64(), Some(82));
    assert_eq!(detail["cache_savings_usd"].as_f64(), Some(0.0011));
    // Grading cost is its own number, never folded into `cost_usd` — that one
    // is what the systems under test cost, and it is what `cost:` budgets.
    assert_eq!(detail["grader_cost_usd"].as_f64(), Some(0.0009));
    assert_ne!(detail["cost_usd"].as_f64(), Some(0.0009));
}

/// A run stored before migration 12 reads back as *unknown*, not as zero. Zero
/// would assert the run had no cache activity, which we have no way to know.
#[tokio::test]
async fn a_run_predating_the_columns_reports_them_as_null_not_zero() {
    let (app, dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let db = rusqlite::Connection::open(dir.path().join("domarinn.db")).unwrap();
    db.execute(
        "UPDATE runs SET cache_read_tokens = NULL, cache_write_tokens = NULL,
                         cache_savings_microusd = NULL, grader_cost_microusd = NULL
         WHERE id = 'r-cached-pass'",
        [],
    )
    .unwrap();
    drop(db);

    let detail = get(&app, "/api/v1/runs/r-cached-pass").await.json();
    assert!(detail["cache_read_tokens"].is_null());
    assert!(detail["cache_write_tokens"].is_null());
    assert!(detail["cache_savings_usd"].is_null());
    assert!(detail["grader_cost_usd"].is_null());
}

#[tokio::test]
async fn cached_exclude_hides_every_fully_cached_run_whatever_its_verdict() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    // No filter: everything is listed, and no hidden count is computed.
    let all = get(&app, "/api/v1/runs").await;
    let all_body = all.json();
    assert_eq!(run_ids(&all_body).len(), 4);
    assert!(all_body["cached_hidden"].is_null());

    // Both fully-cached runs go, the failing one included. The rule used to
    // spare it, reasoning that grader verdicts are not cached so a replay could
    // carry a fresh regression — a claim 0.5.0 had already retired by moving
    // graders into the same cache and key space as provider calls. What the
    // guard did do was fire on every run of any suite whose failures are not
    // rare, so the filter hid nothing at all. That is the bug this pins.
    //
    // `r-partial` is the boundary that still matters: one cached call and one
    // live one is not a replay, and it stays.
    let filtered = get(&app, "/api/v1/runs?cached=exclude").await;
    assert_eq!(filtered.status, StatusCode::OK);
    let body = filtered.json();
    assert_eq!(
        run_ids(&body),
        vec!["r-partial", "r-fresh"],
        "every fully-cached run must be hidden; partially-cached and fresh stay"
    );
    assert_eq!(body["cached_hidden"].as_i64(), Some(2));
}

#[tokio::test]
async fn cached_only_returns_fully_cached_runs_regardless_of_verdict() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let only = get(&app, "/api/v1/runs?cached=only").await;
    assert_eq!(only.status, StatusCode::OK);
    let body = only.json();
    assert_eq!(run_ids(&body), vec!["r-cached-fail", "r-cached-pass"]);
    assert!(body["cached_hidden"].is_null());
}

#[tokio::test]
async fn cached_all_is_explicit_no_op() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let all = get(&app, "/api/v1/runs?cached=all").await;
    assert_eq!(all.status, StatusCode::OK);
    assert_eq!(run_ids(&all.json()).len(), 4);
}

#[tokio::test]
async fn invalid_cached_query_is_400_with_json_error() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let bad = get(&app, "/api/v1/runs?cached=bogus").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    assert!(bad.json()["error"].is_string(), "body: {:?}", bad.json());
}

#[tokio::test]
async fn case_list_exposes_cached_and_filters_by_it() {
    let (app, _dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let all = get(&app, "/api/v1/runs/r-partial/cases").await;
    let all_body = all.json();
    let cases = all_body["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    let cached_flags: Vec<Option<bool>> = cases.iter().map(|c| c["cached"].as_bool()).collect();
    assert!(
        cached_flags.contains(&Some(true)) && cached_flags.contains(&Some(false)),
        "expected one cached and one fresh case, got {cached_flags:?}"
    );

    // Fresh only.
    let fresh = get(&app, "/api/v1/runs/r-partial/cases?cached=false").await;
    assert_eq!(fresh.status, StatusCode::OK);
    let fresh_body = fresh.json();
    let fresh_cases = fresh_body["cases"].as_array().unwrap();
    assert_eq!(fresh_cases.len(), 1);
    assert_eq!(fresh_cases[0]["cached"].as_bool(), Some(false));

    // Cached only.
    let cached = get(&app, "/api/v1/runs/r-partial/cases?cached=true").await;
    let cached_body = cached.json();
    let cached_cases = cached_body["cases"].as_array().unwrap();
    assert_eq!(cached_cases.len(), 1);
    assert_eq!(cached_cases[0]["cached"].as_bool(), Some(true));
}

/// Pre-migration rows have NULL cache columns. NULL means "unknown", and
/// unknown must NEVER be hidden — a NULL-unsafe predicate (`NOT (x = 0 AND …)`)
/// would silently drop every legacy row.
#[tokio::test]
async fn legacy_null_cache_columns_are_never_hidden() {
    let (app, dir) = test_app(Settings::default()).await;
    seed_cache_mix(&app).await;

    let db = rusqlite::Connection::open(dir.path().join("domarinn.db")).unwrap();
    db.execute(
        "UPDATE runs SET cache_hits = NULL, cache_misses = NULL WHERE id = 'r-cached-pass'",
        [],
    )
    .unwrap();
    db.execute(
        "UPDATE cases SET cached = NULL WHERE run_id = 'r-partial'",
        [],
    )
    .unwrap();
    drop(db);

    let filtered = get(&app, "/api/v1/runs?cached=exclude").await;
    let body = filtered.json();
    assert!(
        run_ids(&body).contains(&"r-cached-pass".to_string()),
        "legacy (NULL) run must stay visible under cached=exclude"
    );
    // One, not zero: `r-cached-pass` is the row that was NULLed and is
    // therefore unclassifiable, but `r-cached-fail` is still a replay and is
    // now hidden on that alone. The point of this test is the row above.
    assert_eq!(body["cached_hidden"].as_i64(), Some(1));

    // Legacy cases count as fresh (never hide what we can't classify) and
    // expose `cached: null` on the wire.
    let fresh = get(&app, "/api/v1/runs/r-partial/cases?cached=false").await;
    let fresh_body = fresh.json();
    let fresh_cases = fresh_body["cases"].as_array().unwrap();
    assert_eq!(fresh_cases.len(), 2);
    assert!(fresh_cases.iter().all(|c| c["cached"].is_null()));

    let cached = get(&app, "/api/v1/runs/r-partial/cases?cached=true").await;
    assert_eq!(cached.json()["cases"].as_array().unwrap().len(), 0);
}

/// Reopening the server against a database whose cache columns are NULL (a
/// pre-migration-6 database) repopulates them from the stored blobs, exactly
/// like the migration-3 column backfill.
#[tokio::test]
async fn backfill_populates_cache_columns_from_blobs() {
    let dir = tempfile::TempDir::new().unwrap();
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: AuthMode::Open,
    };

    {
        let (app, _state) = build_app(&config, Settings::default()).await.unwrap();
        seed_cache_mix(&app).await;
    }

    // Simulate a pre-migration-6 database: NULL out every promoted cache column.
    {
        let db = rusqlite::Connection::open(dir.path().join("domarinn.db")).unwrap();
        db.execute("UPDATE runs SET cache_hits = NULL, cache_misses = NULL", [])
            .unwrap();
        db.execute("UPDATE cases SET cached = NULL", []).unwrap();
    }

    // Reopen: the on-open backfill must restore the columns from the blobs.
    let (app, _state) = build_app(&config, Settings::default()).await.unwrap();
    let filtered = get(&app, "/api/v1/runs?cached=exclude").await;
    let body = filtered.json();
    assert_eq!(
        run_ids(&body),
        vec!["r-partial", "r-fresh"],
        "backfilled fully-cached runs must be hidden again"
    );
    assert_eq!(body["cached_hidden"].as_i64(), Some(2));

    let cached = get(&app, "/api/v1/runs/r-partial/cases?cached=true").await;
    assert_eq!(cached.json()["cases"].as_array().unwrap().len(), 1);
}
