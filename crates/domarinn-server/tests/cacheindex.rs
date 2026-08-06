//! Storage-level tests for the cache's derived columns: what migrations 2 and 3
//! promote out of the opaque `body` blob, the kind-inference ladder, the FTS
//! index, the two background passes that populate rows written before each
//! migration, and what a prune is allowed to delete.
//!
//! These reach past the HTTP surface and assert directly against `cache.db`
//! with a raw `rusqlite` connection, in `backfill.rs`'s style. Rows standing in
//! for a pre-migration database are inserted through that raw connection
//! precisely so they bypass the indexing `cache_put` now does — otherwise there
//! would be nothing left for the backfill to find.
//!
//! The prune tests live here rather than beside the route in `cache.rs` on
//! purpose: the route's "a bare prune applies the configured retention limits"
//! fallback would mask a storage layer that deleted everything when handed a
//! filter naming nothing.

mod common;

use std::path::Path;

use domarinn_core::cache::{CacheEntry, CacheKey, EntryKind, GradedVerdict};
use domarinn_core::types::Output;
use domarinn_server::storage::{CachePruneFilter, Storage};
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

/// Insert a row the way a **migration-2** database holds one: indexed and
/// searchable, but by a build that had never heard of `empty_reason`, so
/// `reindexed_at` is still NULL and the promoted reason is missing.
///
/// The fts row is planted too, and that is the whole point of the fixture: the
/// re-index pass meets an existing row for this rowid, and `cache_entries_fts`
/// is plain fts5 with an explicit rowid, so a bare INSERT would raise
/// `SQLITE_CONSTRAINT`.
fn plant_pre_migration_3(conn: &Connection, key: &CacheKey, body: &[u8]) {
    conn.execute(
        "INSERT INTO cache_entries
             (key, body, size, created_at, last_access_at, indexed_at, index_ok, model)
         VALUES (?1, ?2, ?3, 0, 0, 1700000000000, 1, 'claude-opus-5')",
        params![key.0, body, body.len() as i64],
    )
    .expect("plant row");
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO cache_entries_fts (rowid, request, output) VALUES (?1, '', 'stale')",
        params![rowid],
    )
    .expect("plant fts row");
}

fn fts_rows(conn: &Connection, key: &CacheKey) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM cache_entries_fts
          WHERE rowid = (SELECT rowid FROM cache_entries WHERE key = ?1)",
        params![key.0],
        |row| row.get(0),
    )
    .expect("count fts rows")
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
    storage
        .cache_prune(CachePruneFilter {
            target_bytes: Some(0),
            ..CachePruneFilter::default()
        })
        .await
        .unwrap();

    let conn = raw(dir.path());
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM cache_entries_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "a pruned entry must not stay searchable");
}

// ---------------------------------------------------------------------------
// The migration-3 re-index
// ---------------------------------------------------------------------------

/// The test that would have caught the wedge.
///
/// `cache_entries_fts` is plain fts5 and `insert_fts` supplies an explicit
/// rowid, so a second INSERT for a rowid that already has a row raises
/// `SQLITE_CONSTRAINT` — the `?` propagates out of the batch writer *before*
/// its `commit`, rolling back every `reindexed_at` stamp with it. The driver
/// then re-reads the identical batch forever, and the `empty_reason` this whole
/// feature exists to filter on is never populated for a single pre-existing row.
///
/// So this asserts three separate things, and the first two are why a test that
/// only checked "no duplicate fts row" would have passed against the broken
/// code: the batch **committed** (`reindexed_at IS NOT NULL`), the reason
/// landed, and the search index holds exactly one row for the entry.
#[tokio::test]
async fn re_indexing_commits_and_populates_empty_reason() {
    let (storage, dir) = storage().await;
    let key = key_for("poisoned");
    let mut e = entry();
    e.output = Output::Text(String::new());
    e.empty_reason = Some(domarinn_core::empty::EmptyReason::new(
        domarinn_core::empty::EmptyReason::REFUSAL,
    ));
    plant_pre_migration_3(&raw(dir.path()), &key, &serde_json::to_vec(&e).unwrap());

    assert_eq!(storage.cache_reindex_batch(100).await.unwrap(), 1);

    let conn = raw(dir.path());
    assert!(
        column::<Option<i64>>(&conn, &key, "reindexed_at").is_some(),
        "the batch must have committed; a rolled-back batch leaves this NULL"
    );
    assert_eq!(column::<String>(&conn, &key, "empty_reason"), "refusal");
    assert_eq!(
        fts_rows(&conn, &key),
        1,
        "the entry must be searchable exactly once"
    );

    assert_eq!(
        storage.cache_reindex_batch(100).await.unwrap(),
        0,
        "a re-indexed row must not be picked up again"
    );
}

/// A row already recorded as unparseable is stamped without its body ever being
/// read. Re-parsing what already failed cannot succeed, and leaving it pending
/// would make every future pass scan it — the same argument
/// `an_unparseable_row_is_stamped_once_and_never_rescanned` makes for the first
/// pass, one migration later.
#[tokio::test]
async fn the_re_index_leaves_unparseable_rows_alone_but_still_drains() {
    let (storage, dir) = storage().await;
    let key = key_for("junk3");
    {
        let conn = raw(dir.path());
        conn.execute(
            "INSERT INTO cache_entries
                 (key, body, size, created_at, last_access_at, indexed_at, index_ok)
             VALUES (?1, ?2, 10, 0, 0, 1700000000000, 0)",
            params![key.0, b"{ not json".to_vec()],
        )
        .unwrap();
    }

    assert_eq!(storage.cache_reindex_batch(100).await.unwrap(), 1);
    assert_eq!(
        storage.cache_reindex_batch(100).await.unwrap(),
        0,
        "the pass must drain rather than revisit rows it declined to read"
    );

    let conn = raw(dir.path());
    assert_eq!(
        column::<i64>(&conn, &key, "index_ok"),
        0,
        "its verdict is unchanged"
    );
    assert_eq!(
        fts_rows(&conn, &key),
        0,
        "nothing was parsed, so nothing may become searchable"
    );
}

/// A row written by *this* build is already current, so the catch-up pass has
/// nothing to do with it. Leaving `reindexed_at` NULL on every new write would
/// queue the whole live cache for a second body read it does not need.
#[tokio::test]
async fn a_fresh_put_is_never_queued_for_the_catch_up_pass() {
    let (storage, _dir) = storage().await;
    let mut e = entry();
    e.empty_reason = Some(domarinn_core::empty::EmptyReason::new(
        domarinn_core::empty::EmptyReason::BLANK,
    ));
    storage
        .cache_put(key_for("fresh").0, serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();

    assert_eq!(storage.cache_reindex_batch(100).await.unwrap(), 0);
    assert_eq!(storage.cache_backfill_remaining().await.unwrap().total(), 0);
}

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

/// The rule that keeps an unattended caller from wiping a shared cache.
///
/// The list query builds from `WHERE 1=1` and appends ` AND …` per filter,
/// which is fine for a SELECT and fatal for a DELETE: a filter that named
/// nothing would match every row. Two callers can present one — including the
/// **hourly** retention task, if its configured reasons parse to nothing — so
/// the guard is asserted here, at the storage level, and not only at the route
/// where the retention-limits fallback would mask it.
#[tokio::test]
async fn a_prune_with_no_predicate_deletes_nothing() {
    let (storage, _dir) = storage().await;
    for i in 0..3 {
        storage
            .cache_put(
                key_for(&format!("keep{i}")).0,
                serde_json::to_vec(&entry()).unwrap(),
            )
            .await
            .unwrap();
    }

    assert_eq!(
        storage
            .cache_prune(CachePruneFilter::default())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        storage.cache_stats().await.unwrap().entries,
        3,
        "a filter naming nothing must delete nothing at all"
    );

    // An `empty_reason` list that collapsed to empty is the same case: it is
    // "no such predicate", never "every entry".
    assert_eq!(
        storage
            .cache_prune(CachePruneFilter {
                empty_reason: Vec::new(),
                ..CachePruneFilter::default()
            })
            .await
            .unwrap(),
        0
    );
    assert_eq!(storage.cache_stats().await.unwrap().entries, 3);
}

#[tokio::test]
async fn a_prune_by_empty_reason_removes_only_those_entries() {
    let (storage, _dir) = storage().await;
    let mut refused = entry();
    refused.empty_reason = Some(domarinn_core::empty::EmptyReason::new(
        domarinn_core::empty::EmptyReason::REFUSAL,
    ));
    let mut blank = entry();
    blank.empty_reason = Some(domarinn_core::empty::EmptyReason::new(
        domarinn_core::empty::EmptyReason::BLANK,
    ));
    for (seed, e) in [("refused", &refused), ("blank", &blank), ("fine", &entry())] {
        storage
            .cache_put(key_for(seed).0, serde_json::to_vec(e).unwrap())
            .await
            .unwrap();
    }

    let pruned = storage
        .cache_prune(CachePruneFilter {
            empty_reason: vec!["refusal".into(), "blank".into()],
            ..CachePruneFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(pruned, 2);
    assert!(
        storage
            .cache_get(key_for("fine").0)
            .await
            .unwrap()
            .is_some(),
        "an entry with no recorded reason is never matched by a reason predicate"
    );
}

/// Deleting one entry has to take its search row with it, or the index keeps
/// answering for something that is gone. The `cache_entries_delete_fts` trigger
/// is what makes that true, and this is the test that says so.
#[tokio::test]
async fn a_deleted_entry_leaves_no_fts_row() {
    let (storage, dir) = storage().await;
    let key = key_for("doomed");
    let mut e = entry();
    e.output = Output::Text("ephemeral".into());
    storage
        .cache_put(key.0.clone(), serde_json::to_vec(&e).unwrap())
        .await
        .unwrap();
    assert_eq!(fts_rows(&raw(dir.path()), &key), 1);

    assert!(storage.cache_delete_entry(key.0.clone()).await.unwrap());
    assert!(
        !storage.cache_delete_entry(key.0.clone()).await.unwrap(),
        "a second delete removes nothing, which is how a caller tells the two apart"
    );

    let conn = raw(dir.path());
    let searchable: i64 = conn
        .query_row("SELECT COUNT(*) FROM cache_entries_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(searchable, 0);
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
