//! Postgres backend smoke test: opt-in via `DOMARINN_TEST_DATABASE_URL`.
//!
//! The full suite runs against Postgres through the `tests/common` harness
//! (`DOMARINN_TEST_BACKEND=postgres`); this file is the fast first gate — it
//! proves migrations apply, ingest lands, and the cache round-trips, without
//! needing the whole harness. Each run uses a fresh database so reruns never
//! see stale state.

mod common;

use axum::http::StatusCode;
use domarinn_server::storage::Storage;
use domarinn_server::{build_app, AuthMode, ServerConfig, Settings};
use tempfile::TempDir;

/// `CREATE DATABASE` cannot run inside a transaction, so raw SQL via a
/// second connection is the portable way to get an isolated database.
fn fresh_database(admin_url: &str) -> String {
    let mut client = postgres::Client::connect(admin_url, postgres::NoTls).expect("admin connect");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = format!("smoke_{nanos}");
    client
        .batch_execute(&format!(
            "CREATE DATABASE {name} TEMPLATE template0 LOCALE 'C'"
        ))
        .expect("create database");
    // Swap the path segment of the URL for the new database's name.
    let (base, _) = admin_url.rsplit_once('/').expect("url has a path");
    format!("{base}/{name}")
}

#[tokio::test]
async fn postgres_backend_smoke() {
    let Ok(admin_url) = std::env::var("DOMARINN_TEST_DATABASE_URL") else {
        eprintln!("DOMARINN_TEST_DATABASE_URL unset; skipping postgres smoke test");
        return;
    };
    // The sync client embeds its own runtime, which must not start on this
    // tokio test thread (the production path always goes via spawn_blocking).
    let url = tokio::task::spawn_blocking(move || fresh_database(&admin_url))
        .await
        .expect("create database task");

    // Opening twice proves migrations are idempotent and the advisory lock
    // path works on an already-migrated database.
    Storage::open_postgres(url.clone())
        .await
        .expect("first open");
    let storage = Storage::open_postgres(url.clone()).await.expect("reopen");

    // Router over the postgres backend (data_dir still needed for the local
    // cache tier; it stays empty here).
    let dir = TempDir::new().expect("tempdir");
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: AuthMode::Open,
    };
    let settings = Settings {
        database_url: Some(url),
        ..Settings::default()
    };
    let (app, _state) = build_app(&config, settings).await.expect("build_app");

    // Ingest a run through the HTTP surface and read it back.
    let run = common::make_run(
        "run_pg_smoke_1",
        Some("proj"),
        Some("suite"),
        vec!["smoke"],
        Some("main"),
        0,
        &[common::CaseSpec::new(
            "openai:gpt-5",
            "t1",
            domarinn_core::result::CaseStatus::Pass,
        )],
    );
    let body = serde_json::to_value(&run).expect("serialize run");
    let reply = common::post_json(&app, "/api/v1/runs", None, &body).await;
    assert_eq!(reply.status, StatusCode::CREATED, "{:?}", reply.json());
    let reply = common::get(&app, "/api/v1/runs/run_pg_smoke_1").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["project"], "proj");
    let reply = common::get(&app, "/api/v1/runs/run_pg_smoke_1/cases").await;
    assert_eq!(reply.status, StatusCode::OK);

    // Idempotent re-upload.
    let reply = common::post_json(&app, "/api/v1/runs", None, &body).await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());

    // Full-text search runs through the tsvector mirror tables: a hit for
    // the run's tag, prefix matching, and marked-up snippets.
    let reply = common::get(&app, "/api/v1/search?q=smok").await;
    assert_eq!(reply.status, StatusCode::OK, "{:?}", reply.json());
    let hits = reply.json();
    let runs = hits["runs"].as_array().expect("runs array").clone();
    assert_eq!(runs.len(), 1, "prefix search should hit the run: {hits:?}");
    assert_eq!(runs[0]["id"], "run_pg_smoke_1");
    let snippet = runs[0]["snippet"].as_str().expect("snippet");
    assert!(
        snippet.contains('\u{e000}') && snippet.contains('\u{e001}'),
        "snippet should mark the match: {snippet:?}"
    );
    // A query with no indexable tokens is empty groups, not an error.
    let reply = common::get(&app, "/api/v1/search?q=%2A%2A%2A").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["runs"].as_array().map(Vec::len), Some(0));

    // Cache round-trip straight through Storage (put wins, second put loses,
    // get counts a hit).
    let outcome = storage
        .cache_put("sha256:smoke".into(), b"{\"v\":1}".to_vec())
        .await
        .expect("cache put");
    assert_eq!(outcome, domarinn_server::storage::CachePutOutcome::Created);
    let outcome = storage
        .cache_put("sha256:smoke".into(), b"{\"v\":1}".to_vec())
        .await
        .expect("cache re-put");
    assert_eq!(outcome, domarinn_server::storage::CachePutOutcome::Exists);
    let body = storage
        .cache_get("sha256:smoke".into())
        .await
        .expect("cache get");
    assert_eq!(body.as_deref(), Some(b"{\"v\":1}".as_slice()));
}
