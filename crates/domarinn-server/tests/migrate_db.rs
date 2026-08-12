//! SQLite → Postgres migration tool tests: opt-in via
//! `DOMARINN_TEST_DATABASE_URL` (the `pg_smoke.rs` pattern).
//!
//! The seed side is deliberately pinned to SQLite whatever
//! `DOMARINN_TEST_BACKEND` says — the tool's whole job is to read a SQLite
//! deployment — so these tests build their app directly instead of going
//! through `common::test_app_with_storage`, whose backend switch would put
//! the seed data in Postgres and leave the tool nothing to copy.

mod common;

use axum::http::StatusCode;
use axum::Router;
use domarinn_core::result::CaseStatus;
use domarinn_server::storage::{migratedb, Storage};
use domarinn_server::{build_app, AuthMode, ServerConfig, Settings};
use serde_json::json;
use tempfile::TempDir;

/// A SQLite-backed router + a second [`Storage`] handle onto the same files.
/// `database_url: None` explicitly, so the seed side stays SQLite even when
/// the environment configures the Postgres test backend.
async fn sqlite_app(dir: &TempDir) -> (Router, Storage) {
    let settings = Settings {
        database_url: None,
        ..Settings::default()
    };
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: AuthMode::Open,
    };
    let (app, _state) = build_app(&config, settings).await.expect("build_app");
    let storage = Storage::open(dir.path().to_path_buf())
        .await
        .expect("open sqlite storage");
    (app, storage)
}

/// A Postgres-backed router + storage over an already-migrated database.
async fn pg_app(url: &str, dir: &TempDir) -> (Router, Storage) {
    let settings = Settings {
        database_url: Some(url.to_string()),
        ..Settings::default()
    };
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: AuthMode::Open,
    };
    let (app, _state) = build_app(&config, settings).await.expect("build_app pg");
    let storage = Storage::open_postgres(url.to_string())
        .await
        .expect("open pg storage");
    (app, storage)
}

/// Seed the SQLite deployment through the same surfaces production writes
/// use: two runs over HTTP, a baseline over HTTP, cache traffic via Storage.
async fn seed(app: &Router, storage: &Storage) {
    let run1 = common::make_run(
        "run_mig_1",
        Some("proj"),
        Some("suite"),
        vec!["migseedterm"],
        Some("main"),
        0,
        &[
            common::CaseSpec::new("openai:gpt-5", "t1", CaseStatus::Pass)
                .tags(vec!["casetag"])
                .rendered_prompt("say migseedterm"),
            common::CaseSpec::new("openai:gpt-5", "t2", CaseStatus::Fail).error("boom"),
        ],
    );
    let run2 = common::make_run(
        "run_mig_2",
        Some("proj"),
        Some("suite"),
        vec!["nightly"],
        Some("main"),
        1,
        &[common::CaseSpec::new("openai:gpt-5", "t1", CaseStatus::Pass).cached(true)],
    );
    for run in [&run1, &run2] {
        let reply = common::post_json(app, "/api/v1/runs", None, &common::run_value(run)).await;
        assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply.json());
    }

    let reply = common::send(
        app,
        "PUT",
        "/api/v1/projects/proj/suites/suite/baseline",
        None,
        None,
        serde_json::to_vec(&json!({ "run_id": "run_mig_1" })).unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());

    // Two entries, one hit, one miss — so the migrated stats are asymmetric
    // and a dropped counter cannot pass by coincidence.
    storage
        .cache_put("sha256:mig_a".into(), b"{\"v\":\"aa\"}".to_vec())
        .await
        .expect("cache put a");
    storage
        .cache_put("sha256:mig_b".into(), b"{\"v\":\"bb\"}".to_vec())
        .await
        .expect("cache put b");
    let body = storage
        .cache_get("sha256:mig_a".into())
        .await
        .expect("cache get hit");
    assert!(body.is_some());
    let body = storage
        .cache_get("sha256:absent".into())
        .await
        .expect("cache get miss");
    assert!(body.is_none());
}

#[tokio::test]
async fn migrate_db_copies_a_sqlite_deployment() {
    let Ok(_) = std::env::var("DOMARINN_TEST_DATABASE_URL") else {
        eprintln!("DOMARINN_TEST_DATABASE_URL unset; skipping migrate-db test");
        return;
    };

    let dir = TempDir::new().expect("tempdir");
    let (app, storage) = sqlite_app(&dir).await;
    seed(&app, &storage).await;

    let url = tokio::task::spawn_blocking(common::pg::fresh_database_url)
        .await
        .expect("create database task");
    let report = migratedb::migrate_to_postgres(dir.path().to_path_buf(), url.clone())
        .await
        .expect("migrate");

    let rows = |table: &str| {
        report
            .tables
            .iter()
            .find(|(t, _)| t == table)
            .unwrap_or_else(|| panic!("{table} missing from report"))
            .1
    };
    assert_eq!(rows("runs"), 2);
    assert_eq!(rows("cases"), 3);
    assert_eq!(rows("baselines"), 1);
    assert_eq!(rows("cache_entries"), 2);

    // Read everything back through a Postgres-backed router over the target.
    let pg_dir = TempDir::new().expect("pg tempdir");
    let (pg_app, pg_storage) = pg_app(&url, &pg_dir).await;

    // Run list: both ids, newest first.
    let reply = common::get(&pg_app, "/api/v1/runs?cached=all").await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
    let ids: Vec<String> = reply.json()["runs"]
        .as_array()
        .expect("runs array")
        .iter()
        .map(|r| r["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids, vec!["run_mig_2", "run_mig_1"]);

    // Run detail fields survived the copy.
    let reply = common::get(&pg_app, "/api/v1/runs/run_mig_1").await;
    assert_eq!(reply.status, StatusCode::OK);
    let detail = reply.json();
    assert_eq!(detail["project"], "proj");
    assert_eq!(detail["suite"], "suite");
    assert_eq!(detail["git_branch"], "main");
    assert_eq!(detail["pass_count"], 1);
    assert_eq!(detail["fail_count"], 1);

    // Case list.
    let reply = common::get(&pg_app, "/api/v1/runs/run_mig_1/cases").await;
    assert_eq!(reply.status, StatusCode::OK);
    let cases = reply.json()["cases"].as_array().expect("cases").clone();
    assert_eq!(cases.len(), 2);

    // Search finds the seeded term through the tsvector mirrors.
    let reply = common::get(&pg_app, "/api/v1/search?q=migseedterm").await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
    let hits = reply.json();
    let runs = hits["runs"].as_array().expect("runs hits");
    assert_eq!(runs.len(), 1, "search should hit the run: {hits:?}");
    assert_eq!(runs[0]["id"], "run_mig_1");

    // Baseline intact.
    let reply = common::get(&pg_app, "/api/v1/projects/proj/suites").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["suites"][0]["baseline_run_id"], "run_mig_1");

    // Counters carried over — read stats before any Postgres-side cache
    // traffic can bump them.
    let stats = pg_storage.cache_stats().await.expect("cache stats");
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.misses, 1);

    // Cache bodies round-trip.
    let body = pg_storage
        .cache_get("sha256:mig_a".into())
        .await
        .expect("pg cache get");
    assert_eq!(body.as_deref(), Some(b"{\"v\":\"aa\"}".as_slice()));
}

#[tokio::test]
async fn migrate_db_refuses_a_non_empty_target() {
    let Ok(_) = std::env::var("DOMARINN_TEST_DATABASE_URL") else {
        eprintln!("DOMARINN_TEST_DATABASE_URL unset; skipping migrate-db refusal test");
        return;
    };

    let dir = TempDir::new().expect("tempdir");
    let (app, storage) = sqlite_app(&dir).await;
    seed(&app, &storage).await;

    let url = tokio::task::spawn_blocking(common::pg::fresh_database_url)
        .await
        .expect("create database task");
    migratedb::migrate_to_postgres(dir.path().to_path_buf(), url.clone())
        .await
        .expect("first migrate");
    let err = migratedb::migrate_to_postgres(dir.path().to_path_buf(), url)
        .await
        .expect_err("second migrate must refuse the populated target");
    assert!(
        format!("{err:#}").contains("empty"),
        "error should tell the operator to use an empty database: {err:#}"
    );
}
