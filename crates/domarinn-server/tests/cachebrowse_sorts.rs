//! The cache browse API's text/expression sort columns (`kind`, `model`,
//! `tokens`, `key`), added after the original four integer sorts.
//!
//! A separate suite from `cachebrowse.rs`, which is near the per-file line
//! ratchet. The interesting machinery under test is the cursor: text sorts
//! carry a `t:{hex}:{key}` keyset cursor where the integer sorts carry the
//! historical `{i64}:{key}`, and the token sort paginates over an SQL
//! expression rather than a column.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::cache::{CacheEntry, CacheKey, EntryKind};
use domarinn_core::types::{Output, TokenUsage};
use domarinn_server::Settings;
use serde_json::{json, Value};

fn key_for(seed: &str) -> String {
    CacheKey::compute(&json!({ "seed": seed })).0
}

fn entry() -> CacheEntry {
    CacheEntry {
        created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        kind: Some(EntryKind::new(EntryKind::PROVIDER)),
        provider_fingerprint: None,
        request: None,
        output: Output::Text("hello".into()),
        usage: None,
        cost_usd: None,
        stop_reason: None,
        raw: None,
        attempts: None,
        provider_latency_ms: None,
        model: Some("claude-opus-5".into()),
        program_digest: None,
        address: None,
        verdict: None,
        reasoning: None,
        empty_reason: None,
        tool_calls: Vec::new(),
        domarinn_version: "0.5.0".into(),
    }
}

async fn store(app: &axum::Router, seed: &str, e: &CacheEntry) -> String {
    let key = key_for(seed);
    let reply = put_bytes(
        app,
        &format!("/api/v1/cache/{key}"),
        None,
        serde_json::to_vec(e).unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "seeding {seed}");
    key
}

fn entries(body: &Value) -> &Vec<Value> {
    body["entries"].as_array().expect("entries array")
}

/// Walk a sorted listing to exhaustion in pages of two and assert every seeded
/// entry appears exactly once — the invariant a broken keyset cursor loses
/// first, by repeating or dropping rows at a page boundary inside a tie.
async fn paginates_exhaustively(app: &axum::Router, sort: &str, expected: usize) {
    let mut seen: Vec<String> = Vec::new();
    let mut uri = format!("/api/v1/cache/entries?limit=2&sort={sort}");
    loop {
        let body: Value = get(app, &uri).await.json();
        for e in entries(&body) {
            seen.push(e["key"].as_str().unwrap().to_string());
        }
        match body["next_cursor"].as_str() {
            Some(cursor) => {
                uri = format!("/api/v1/cache/entries?limit=2&sort={sort}&cursor={cursor}")
            }
            None => break,
        }
    }
    assert_eq!(seen.len(), expected, "sort={sort}: {seen:?}");
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        deduped.len(),
        expected,
        "sort={sort} repeated a row: {seen:?}"
    );
}

/// The text cursor: five entries across two kinds, so the page boundary lands
/// inside a run of equal sort values and only the key tie-break separates them.
#[tokio::test]
async fn a_kind_sort_paginates_without_repeating_or_dropping() {
    let (app, _dir) = test_app(Settings::default()).await;
    for i in 0..3 {
        store(&app, &format!("provider{i}"), &entry()).await;
    }
    let mut judge = entry();
    judge.kind = Some(EntryKind::new(EntryKind::JUDGE));
    for i in 0..2 {
        store(&app, &format!("judge{i}"), &judge).await;
    }
    paginates_exhaustively(&app, "kind", 5).await;
}

/// The expression cursor: `tokens` orders by `(input_tokens + output_tokens)`,
/// and two entries share a sum so the tie-break is exercised there too.
#[tokio::test]
async fn a_token_sum_sort_paginates_without_repeating_or_dropping() {
    let (app, _dir) = test_app(Settings::default()).await;
    for (i, (input, output)) in [(100, 20), (60, 60), (90, 30), (10, 5), (200, 1)]
        .into_iter()
        .enumerate()
    {
        let mut e = entry();
        e.usage = Some(TokenUsage {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        });
        store(&app, &format!("tokens{i}"), &e).await;
    }
    paginates_exhaustively(&app, "tokens", 5).await;
}

/// Same stance as the cost sort: an entry nothing has been established about
/// (unparseable here, so `kind` is NULL) is excluded from an ordering by that
/// value rather than ranked by a guess — and the NULL tail would also stop the
/// keyset pagination dead.
#[tokio::test]
async fn sorting_by_kind_lists_only_indexed_entries() {
    let (app, _dir) = test_app(Settings::default()).await;
    store(&app, "wellformed", &entry()).await;
    let key = key_for("opaque");
    put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"not an entry".to_vec(),
    )
    .await;

    let all: Value = get(&app, "/api/v1/cache/entries").await.json();
    assert_eq!(entries(&all).len(), 2);

    let by_kind: Value = get(&app, "/api/v1/cache/entries?sort=kind").await.json();
    assert_eq!(entries(&by_kind).len(), 1);
    assert_eq!(entries(&by_kind)[0]["kind"], json!("provider"));
}

#[tokio::test]
async fn a_model_sort_orders_lexically_in_both_directions() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut zulu = entry();
    zulu.model = Some("zulu-1".into());
    store(&app, "zulu", &zulu).await;
    store(&app, "claude", &entry()).await;

    let asc: Value = get(&app, "/api/v1/cache/entries?sort=model&order=asc")
        .await
        .json();
    assert_eq!(entries(&asc)[0]["model"], json!("claude-opus-5"));

    // The grid asks for A→Z on first click, but the endpoint's own default
    // order remains desc.
    let desc: Value = get(&app, "/api/v1/cache/entries?sort=model").await.json();
    assert_eq!(entries(&desc)[0]["model"], json!("zulu-1"));
}

/// The local tier consumes the same `CacheSort` and must honour the new
/// variants with its in-memory comparator.
#[tokio::test]
async fn the_local_tier_sorts_by_model() {
    use domarinn_cache::LocalDiskCache;
    use domarinn_core::cache::CacheBackend;

    let local_dir = tempfile::TempDir::new().expect("tempdir");
    let disk = LocalDiskCache::new(local_dir.path());
    let mut judge = entry();
    judge.kind = Some(EntryKind::new(EntryKind::JUDGE));
    judge.model = Some("gpt-4o".into());
    disk.put(&CacheKey(key_for("localA")), &entry())
        .await
        .unwrap();
    disk.put(&CacheKey(key_for("localB")), &judge)
        .await
        .unwrap();

    let settings = Settings {
        local_cache_dir: Some(local_dir.path().to_path_buf()),
        ..Default::default()
    };
    let (app, _data) = test_app(settings).await;

    let asc: Value = get(
        &app,
        "/api/v1/cache/entries?tier=local&sort=model&order=asc",
    )
    .await
    .json();
    let models: Vec<&str> = entries(&asc)
        .iter()
        .map(|e| e["model"].as_str().unwrap())
        .collect();
    assert_eq!(models, vec!["claude-opus-5", "gpt-4o"]);
}
