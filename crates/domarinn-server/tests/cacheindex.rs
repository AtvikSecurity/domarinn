//! Storage-level tests for cache migration 2: the columns promoted out of the
//! opaque `body` blob, the kind-inference ladder, the FTS index, and the
//! background backfill that populates rows written before the migration.
//!
//! These reach past the HTTP surface and assert directly against `cache.db`
//! with a raw `rusqlite` connection, in `backfill.rs`'s style. Rows standing in
//! for a pre-migration database are inserted through that raw connection
//! precisely so they bypass the indexing `cache_put` now does — otherwise there
//! would be nothing left for the backfill to find.

mod common;

use std::path::Path;

use domarinn_core::cache::{CacheEntry, CacheKey, EntryKind, GradedVerdict};
use domarinn_core::types::Output;
use domarinn_server::storage::Storage;
use rusqlite::{params, Connection};
use serde_json::json;
use tempfile::TempDir;

/// A connection straight to the cache database, for inspecting columns no DTO
/// exposes and for planting rows that look like they predate the migration.
fn raw(dir: &Path) -> Connection {
    Connection::open(dir.join("cache.db")).expect("open raw cache db")
}

async fn storage() -> (Storage, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let storage = Storage::open(dir.path().to_path_buf())
        .await
        .expect("open storage");
    (storage, dir)
}

/// A minimal entry. Every test that cares about a field sets it explicitly, so
/// the defaults here should stay boring.
fn entry() -> CacheEntry {
    CacheEntry {
        created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        kind: None,
        provider_fingerprint: None,
        request: None,
        output: Output::Text("hello".into()),
        usage: None,
        cost_usd: None,
        stop_reason: None,
        raw: None,
        attempts: None,
        provider_latency_ms: None,
        model: None,
        program_digest: None,
        address: None,
        verdict: None,
        reasoning: None,
        empty_reason: None,
        tool_calls: Vec::new(),
        domarinn_version: "0.5.0".into(),
    }
}

fn key_for(seed: &str) -> CacheKey {
    CacheKey::compute(&json!({ "seed": seed }))
}

/// Insert a row the way a pre-migration database holds one: a body, and every
/// promoted column still NULL.
fn plant_unindexed(conn: &Connection, key: &CacheKey, body: &[u8]) {
    conn.execute(
        "INSERT INTO cache_entries (key, body, size, created_at, last_access_at)
         VALUES (?1, ?2, ?3, 0, 0)",
        params![key.0, body, body.len() as i64],
    )
    .expect("plant row");
}

fn column<T: rusqlite::types::FromSql>(conn: &Connection, key: &CacheKey, col: &str) -> T {
    conn.query_row(
        &format!("SELECT {col} FROM cache_entries WHERE key = ?1"),
        params![key.0],
        |row| row.get(0),
    )
    .unwrap_or_else(|e| panic!("reading {col}: {e}"))
}

// ---------------------------------------------------------------------------
// Indexing on write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_put_indexes_its_metadata_without_a_backfill() {
    let (storage, dir) = storage().await;
    let key = key_for("put");
    let mut e = entry();
    e.kind = Some(EntryKind::new(EntryKind::PROVIDER));
    e.model = Some("claude-opus-5".into());
    e.cost_usd = Some(0.0125);

    storage
        .cache_put(key.0.clone(), serde_json::to_vec(&e).unwrap())
        .await
        .expect("put");

    let conn = raw(dir.path());
    assert_eq!(column::<String>(&conn, &key, "kind"), "provider");
    assert_eq!(column::<String>(&conn, &key, "model"), "claude-opus-5");
    assert_eq!(column::<i64>(&conn, &key, "cost_microusd"), 12_500);
    assert_eq!(column::<i64>(&conn, &key, "index_ok"), 1);
    assert!(column::<Option<i64>>(&conn, &key, "indexed_at").is_some());
}

/// The cache is a blob store by contract: it stores what it is handed and the
/// client owns the format. A body this server cannot parse must still be
/// retrievable byte-for-byte by a client that can.
#[tokio::test]
async fn a_body_this_server_cannot_parse_is_still_stored_and_served() {
    let (storage, dir) = storage().await;
    let key = key_for("garbage");
    let body = b"this is not a cache entry".to_vec();

    storage
        .cache_put(key.0.clone(), body.clone())
        .await
        .expect("an unparseable put must still succeed");

    assert_eq!(
        storage.cache_get(key.0.clone()).await.unwrap(),
        Some(body),
        "the stored bytes must come back unchanged"
    );

    let conn = raw(dir.path());
    assert_eq!(column::<i64>(&conn, &key, "index_ok"), 0);
    assert!(
        column::<Option<String>>(&conn, &key, "model").is_none(),
        "nothing was parsed, so nothing may be claimed"
    );
}

// ---------------------------------------------------------------------------
// The backfill
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_backfill_populates_a_row_written_before_the_migration() {
    let (storage, dir) = storage().await;
    let key = key_for("old");
    let mut e = entry();
    e.model = Some("gpt-4o".into());
    e.attempts = Some(1);
    plant_unindexed(&raw(dir.path()), &key, &serde_json::to_vec(&e).unwrap());

    assert_eq!(storage.cache_index_batch(100).await.unwrap(), 1);

    let conn = raw(dir.path());
    assert_eq!(column::<String>(&conn, &key, "model"), "gpt-4o");
}

/// The invariant that makes the backfill terminate. A row whose body cannot be
/// parsed is stamped as looked-at, so it drops out of the driving predicate
/// forever; leaving it NULL would make every future pass pick it up again.
#[tokio::test]
async fn an_unparseable_row_is_stamped_once_and_never_rescanned() {
    let (storage, dir) = storage().await;
    plant_unindexed(&raw(dir.path()), &key_for("junk"), b"{ not json");

    assert_eq!(storage.cache_index_batch(100).await.unwrap(), 1);
    assert_eq!(
        storage.cache_index_batch(100).await.unwrap(),
        0,
        "a stamped row must not be picked up again"
    );
}

#[tokio::test]
async fn the_backfill_drains_and_then_reports_nothing_left() {
    let (storage, dir) = storage().await;
    {
        let conn = raw(dir.path());
        for i in 0..5 {
            let e = entry();
            plant_unindexed(
                &conn,
                &key_for(&format!("e{i}")),
                &serde_json::to_vec(&e).unwrap(),
            );
        }
    }

    assert_eq!(storage.cache_index_batch(2).await.unwrap(), 2);
    assert_eq!(storage.cache_index_batch(2).await.unwrap(), 2);
    assert_eq!(storage.cache_index_batch(2).await.unwrap(), 1);
    assert_eq!(storage.cache_index_batch(2).await.unwrap(), 0);
    assert_eq!(storage.cache_stats().await.unwrap().unindexed, 0);
}

#[tokio::test]
async fn stats_report_how_much_is_still_unindexed() {
    let (storage, dir) = storage().await;
    {
        let conn = raw(dir.path());
        for i in 0..3 {
            plant_unindexed(
                &conn,
                &key_for(&format!("u{i}")),
                &serde_json::to_vec(&entry()).unwrap(),
            );
        }
    }
    assert_eq!(storage.cache_stats().await.unwrap().unindexed, 3);
    storage.cache_index_batch(100).await.unwrap();
    assert_eq!(storage.cache_stats().await.unwrap().unindexed, 0);
}

// ---------------------------------------------------------------------------
// The kind-inference ladder
// ---------------------------------------------------------------------------

async fn inferred_kind(entry: CacheEntry, seed: &str) -> Option<String> {
    let (storage, dir) = storage().await;
    let key = key_for(seed);
    plant_unindexed(&raw(dir.path()), &key, &serde_json::to_vec(&entry).unwrap());
    storage.cache_index_batch(10).await.unwrap();
    column::<Option<String>>(&raw(dir.path()), &key, "kind")
}

/// An explicit kind is a statement by the writer; everything below it in the
/// ladder is a guess by a reader. A guess must never overrule a statement —
/// including one this binary does not recognize.
#[tokio::test]
async fn an_explicit_kind_beats_every_inference() {
    let mut e = entry();
    e.kind = Some(EntryKind::new("distillation"));
    // Signals that would otherwise infer `provider`.
    e.attempts = Some(2);
    e.provider_latency_ms = Some(40);
    assert_eq!(
        inferred_kind(e, "explicit").await.as_deref(),
        Some("distillation")
    );
}

#[tokio::test]
async fn a_legacy_rubric_verdict_is_inferred_as_a_judge() {
    let mut e = entry();
    e.verdict = Some(GradedVerdict::Rubric {
        score: 0.9,
        pass: true,
        reasoning: "good".into(),
    });
    assert_eq!(inferred_kind(e, "rubric").await.as_deref(), Some("judge"));
}

#[tokio::test]
async fn an_embedding_is_inferred_from_its_output_shape() {
    let mut e = entry();
    e.output = Output::Json(json!({"dims": 1536}));
    assert_eq!(
        inferred_kind(e, "embed").await.as_deref(),
        Some("embedding")
    );
}

/// `response_to_entry` always records both; `fresh_entry` always records
/// neither, and says why. So either one present is a provider response.
#[tokio::test]
async fn a_pre_kind_entry_with_an_attempt_count_is_inferred_as_a_provider() {
    let mut e = entry();
    e.attempts = Some(1);
    assert_eq!(
        inferred_kind(e, "attempts").await.as_deref(),
        Some("provider")
    );
}

#[tokio::test]
async fn an_exec_request_is_inferred_from_the_kind_on_its_stdin_envelope() {
    let mut e = entry();
    e.request = Some(json!({
        "transport": "exec",
        "command": "./sut",
        "stdin": {"domarinn": {"protocol": 1, "kind": "assert"}},
    }));
    assert_eq!(
        inferred_kind(e, "execassert").await.as_deref(),
        Some("exec_assert")
    );
}

/// The honest bottom of the ladder. An old HTTP judge entry is indistinguishable
/// from an old HTTP provider entry, so it gets no kind rather than a wrong one.
#[tokio::test]
async fn an_entry_with_no_evidence_is_left_without_a_kind() {
    let mut e = entry();
    e.request = Some(json!({"transport": "http", "method": "POST", "url": "https://x/y"}));
    assert_eq!(inferred_kind(e, "unknowable").await, None);
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_text_search_finds_an_entry_by_its_output() {
    let (storage, dir) = storage().await;
    let key = key_for("fts");
    let mut e = entry();
    e.output = Output::Text("the refund policy is thirty days".into());
    storage
        .cache_put(key.0.clone(), serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();

    let conn = raw(dir.path());
    let hits: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cache_entries_fts WHERE cache_entries_fts MATCH 'refund'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 1);
}

/// The FTS table is standalone, not external-content, so a delete has to reach
/// it. Without this the index keeps answering for entries that are gone.
#[tokio::test]
async fn pruning_an_entry_drops_its_search_row() {
    let (storage, dir) = storage().await;
    let mut e = entry();
    e.output = Output::Text("ephemeral".into());
    storage
        .cache_put(key_for("prune").0, serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();

    // Evict by size target rather than by age. `older_than_days = 0` computes
    // a cutoff of *now* and deletes strictly-older rows, so an entry written in
    // the same millisecond survives — a real flake, not a hypothetical.
    storage.cache_prune(None, Some(0)).await.unwrap();

    let conn = raw(dir.path());
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM cache_entries_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "a pruned entry must not stay searchable");
}

// ---------------------------------------------------------------------------
// The request summary
// ---------------------------------------------------------------------------

/// A self-hosted gateway URL can carry a key in its query string. The summary
/// is the one promoted field rendered in a list, so it must not become the
/// place a credential leaks.
#[tokio::test]
async fn an_http_request_summary_drops_the_query_string() {
    let (storage, dir) = storage().await;
    let key = key_for("qs");
    let mut e = entry();
    e.request = Some(json!({
        "transport": "http",
        "method": "POST",
        "url": "https://gateway.internal/v1/messages?api_key=super-secret",
    }));
    storage
        .cache_put(key.0.clone(), serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();

    let summary = column::<String>(&raw(dir.path()), &key, "request_summary");
    assert!(
        !summary.contains("super-secret"),
        "summary leaked a query string: {summary}"
    );
    assert!(summary.contains("gateway.internal"), "{summary}");
}

#[tokio::test]
async fn an_exec_request_summary_names_the_command() {
    let (storage, dir) = storage().await;
    let key = key_for("execsummary");
    let mut e = entry();
    e.request = Some(json!({"transport": "exec", "command": "./bin/sut", "args": ["--fast"]}));
    storage
        .cache_put(key.0.clone(), serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();

    let summary = column::<String>(&raw(dir.path()), &key, "request_summary");
    assert!(summary.contains("./bin/sut"), "{summary}");
}
