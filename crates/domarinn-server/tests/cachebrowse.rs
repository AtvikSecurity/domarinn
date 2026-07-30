//! The cache browse API: listing, filtering, searching, and inspecting
//! individual entries.
//!
//! A separate suite from `cache.rs`, which covers the client-facing get/put/
//! prune surface and is already near the per-file ratchet.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::cache::{CacheEntry, CacheKey, EntryKind};
use domarinn_core::types::{Output, TokenUsage};
use domarinn_server::{AuthMode, Settings};
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
        verdict: None,
        reasoning: None,
        empty_reason: None,
        tool_calls: Vec::new(),
        domarinn_version: "0.5.0".into(),
    }
}

async fn store(app: &axum::Router, seed: &str, e: &CacheEntry) -> String {
    store_as(app, seed, e, None).await
}

/// Seeding needs a write token once the app is not in open mode.
async fn store_as(app: &axum::Router, seed: &str, e: &CacheEntry, token: Option<&str>) -> String {
    let key = key_for(seed);
    let reply = put_bytes(
        app,
        &format!("/api/v1/cache/{key}"),
        token,
        serde_json::to_vec(e).unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "seeding {seed}");
    key
}

fn entries(body: &Value) -> &Vec<Value> {
    body["entries"].as_array().expect("entries array")
}

// ---------------------------------------------------------------------------
// The invariant that must never regress
// ---------------------------------------------------------------------------

/// Browsing is not a lookup, and it must not pretend to be one.
///
/// Two counters, two separate reasons. `hits`/`misses` back the UI's lookup
/// hit-rate tile, so paging a list through the counting read would make that
/// tile lie — the same argument HEAD already carries, one step further. And
/// `last_access_at` is what `cache_prune` evicts on, so a browse that touched
/// it would mean *looking at the cache changes what the cache evicts*: an admin
/// skimming a page would silently rescue the coldest entries in the store from
/// the next retention pass.
#[tokio::test]
async fn browsing_moves_neither_the_hit_counters_nor_the_eviction_clock() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = store(&app, "counters", &entry()).await;

    let before: Value = get(&app, "/api/v1/cache/stats").await.json();
    let access_before: Value = get(&app, "/api/v1/cache/entries").await.json();
    let last_access_before = entries(&access_before)[0]["last_access_at"].clone();

    // Page the list and open the entry — the whole browse path.
    for _ in 0..3 {
        assert_eq!(
            get(&app, "/api/v1/cache/entries?limit=50").await.status,
            StatusCode::OK
        );
        assert_eq!(
            get(&app, &format!("/api/v1/cache/entries/{key}"))
                .await
                .status,
            StatusCode::OK
        );
    }

    let after: Value = get(&app, "/api/v1/cache/stats").await.json();
    assert_eq!(
        after["hits"], before["hits"],
        "browsing moved the hit count"
    );
    assert_eq!(
        after["misses"], before["misses"],
        "browsing moved the miss count"
    );

    let access_after: Value = get(&app, "/api/v1/cache/entries").await.json();
    assert_eq!(
        entries(&access_after)[0]["last_access_at"],
        last_access_before,
        "browsing moved the eviction clock"
    );
}

/// The counting read still counts. Without this, the test above could pass by
/// the counters being broken outright.
#[tokio::test]
async fn the_client_read_path_still_counts() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = store(&app, "counts", &entry()).await;

    let before: Value = get(&app, "/api/v1/cache/stats").await.json();
    get(&app, &format!("/api/v1/cache/{key}")).await;
    let after: Value = get(&app, "/api/v1/cache/stats").await.json();

    assert_eq!(
        after["hits"].as_i64().unwrap(),
        before["hits"].as_i64().unwrap() + 1
    );
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

/// `/cache/entries` and `/cache/{key}` coexist because a legal key is
/// `sha256:<64 hex>` and can never be the literal `entries`. `/cache/stats`
/// already proves the precedent, but a route test is cheap and the failure
/// mode otherwise is a confusing 400 from key validation.
#[tokio::test]
async fn the_entries_route_does_not_shadow_the_key_route() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = store(&app, "shadow", &entry()).await;

    assert_eq!(
        get(&app, "/api/v1/cache/entries").await.status,
        StatusCode::OK
    );
    assert_eq!(
        get(&app, &format!("/api/v1/cache/{key}")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_invalid_key_is_rejected_and_a_missing_one_is_not_found() {
    let (app, _dir) = test_app(Settings::default()).await;

    assert_eq!(
        get(&app, "/api/v1/cache/entries/not-a-key").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        get(
            &app,
            &format!("/api/v1/cache/entries/{}", key_for("absent"))
        )
        .await
        .status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_unknown_query_parameter_is_a_400() {
    let (app, _dir) = test_app(Settings::default()).await;
    assert_eq!(
        get(&app, "/api/v1/cache/entries?kidn=provider")
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn an_unknown_tier_is_rejected_and_an_unmounted_one_is_not_found() {
    let (app, _dir) = test_app(Settings::default()).await;
    assert_eq!(
        get(&app, "/api/v1/cache/entries?tier=nonsense")
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    // `local` is a real tier that this instance has not mounted: the resource
    // does not exist here, which is a 404, not a malformed request.
    assert_eq!(
        get(&app, "/api/v1/cache/entries?tier=local").await.status,
        StatusCode::NOT_FOUND
    );
}

// ---------------------------------------------------------------------------
// Auth: gate enumeration, not access
// ---------------------------------------------------------------------------

/// Reading one entry has always required knowing a 256-bit content hash you
/// could only compute by already possessing the exact request — capability
/// access by accident, and unchanged here. *Enumerating* is the new power, and
/// `read` is the anonymous scope in protect-writes mode, so listing at `read`
/// would publish the whole prompt corpus to unauthenticated callers.
#[tokio::test]
async fn listing_requires_admin_while_reading_one_entry_does_not() {
    let settings = Settings {
        tokens: Some("admin:domarinn_ops,write:domarinn_ci,read:domarinn_ro".to_string()),
        auth_mode: Some(AuthMode::ProtectWrites),
        ..Default::default()
    };
    let (app, _dir) = test_app_with_mode(settings, AuthMode::ProtectWrites).await;
    let key = store_as(&app, "scoped", &entry(), Some("domarinn_ci")).await;

    for (token, expected) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some("domarinn_ro"), StatusCode::FORBIDDEN),
        (Some("domarinn_ci"), StatusCode::FORBIDDEN),
        (Some("domarinn_ops"), StatusCode::OK),
    ] {
        assert_eq!(
            get_auth(&app, "/api/v1/cache/entries", token).await.status,
            expected,
            "listing with {token:?}"
        );
        assert_eq!(
            get_auth(&app, "/api/v1/cache/facets", token).await.status,
            expected,
            "facets with {token:?}"
        );
    }

    // Detail keeps today's posture: anyone who can read, and who already knows
    // the key, may open it.
    assert_eq!(
        get_auth(&app, &format!("/api/v1/cache/entries/{key}"), None)
            .await
            .status,
        StatusCode::OK
    );
}

// ---------------------------------------------------------------------------
// Listing, sorting, pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_list_paginates_by_cursor_without_repeating_or_dropping() {
    let (app, _dir) = test_app(Settings::default()).await;
    for i in 0..5 {
        store(&app, &format!("page{i}"), &entry()).await;
    }

    let mut seen: Vec<String> = Vec::new();
    let mut uri = "/api/v1/cache/entries?limit=2".to_string();
    loop {
        let body: Value = get(&app, &uri).await.json();
        for e in entries(&body) {
            seen.push(e["key"].as_str().unwrap().to_string());
        }
        match body["next_cursor"].as_str() {
            Some(cursor) => uri = format!("/api/v1/cache/entries?limit=2&cursor={cursor}"),
            None => break,
        }
    }

    assert_eq!(
        seen.len(),
        5,
        "every entry must appear exactly once: {seen:?}"
    );
    let mut deduped = seen.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(deduped.len(), 5, "an entry was served twice: {seen:?}");
}

/// Ordering by an unknown value is meaningless, and the NULL tail would also
/// stop keyset pagination dead: `cost < ?` is NULL for every remaining row, so
/// the page would silently come back empty forever. Documented as an exception
/// to "never hide what we cannot classify" — filtering on an unknown is a
/// different thing from ordering by one.
#[tokio::test]
async fn sorting_by_cost_lists_only_entries_whose_cost_is_known() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut priced = entry();
    priced.cost_usd = Some(0.01);
    store(&app, "priced", &priced).await;
    store(&app, "unpriced", &entry()).await;

    let all: Value = get(&app, "/api/v1/cache/entries").await.json();
    assert_eq!(entries(&all).len(), 2);

    let by_cost: Value = get(&app, "/api/v1/cache/entries?sort=cost").await.json();
    assert_eq!(entries(&by_cost).len(), 1);
    assert_eq!(entries(&by_cost)[0]["cost_usd"], json!(0.01));
}

#[tokio::test]
async fn entries_sort_newest_first_by_default_and_can_be_reversed() {
    let (app, _dir) = test_app(Settings::default()).await;
    let small = entry();
    let mut big = entry();
    big.output = Output::Text("x".repeat(4096));
    store(&app, "small", &small).await;
    store(&app, "big", &big).await;

    let by_size: Value = get(&app, "/api/v1/cache/entries?sort=size").await.json();
    let sizes: Vec<i64> = entries(&by_size)
        .iter()
        .map(|e| e["size"].as_i64().unwrap())
        .collect();
    assert!(
        sizes[0] > sizes[1],
        "default order is descending: {sizes:?}"
    );

    let asc: Value = get(&app, "/api/v1/cache/entries?sort=size&order=asc")
        .await
        .json();
    let sizes: Vec<i64> = entries(&asc)
        .iter()
        .map(|e| e["size"].as_i64().unwrap())
        .collect();
    assert!(sizes[0] < sizes[1], "{sizes:?}");
}

// ---------------------------------------------------------------------------
// Filtering and search
// ---------------------------------------------------------------------------

#[tokio::test]
async fn entries_filter_by_kind_and_model() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut judge = entry();
    judge.kind = Some(EntryKind::new(EntryKind::JUDGE));
    judge.model = Some("gpt-4o".into());
    store(&app, "judge", &judge).await;
    store(&app, "provider", &entry()).await;

    let by_kind: Value = get(&app, "/api/v1/cache/entries?kind=judge").await.json();
    assert_eq!(entries(&by_kind).len(), 1);
    assert_eq!(entries(&by_kind)[0]["kind"], json!("judge"));

    let by_model: Value = get(&app, "/api/v1/cache/entries?model=gpt-4o").await.json();
    assert_eq!(entries(&by_model).len(), 1);
}

#[tokio::test]
async fn full_text_search_finds_an_entry_by_its_output() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut refund = entry();
    refund.output = Output::Text("our refund window is thirty days".into());
    store(&app, "refund", &refund).await;
    store(&app, "other", &entry()).await;

    let hits: Value = get(&app, "/api/v1/cache/entries?q=refund").await.json();
    assert_eq!(entries(&hits).len(), 1);
    assert_eq!(entries(&hits)[0]["key"], json!(key_for("refund")));
}

/// A search term that means something to fts5 must not become a 500. Users type
/// quotes and stars; the engine's syntax errors are the server's problem.
#[tokio::test]
async fn a_search_term_with_fts_syntax_does_not_error() {
    let (app, _dir) = test_app(Settings::default()).await;
    store(&app, "syntax", &entry()).await;

    // Percent-encoded, because these have to be legal URIs before they can be
    // hostile fts5 input. The point is what reaches the query engine.
    for q in [
        "%22unbalanced",
        "NEAR%28",
        "*",
        "a%20OR",
        "%2A%2A",
        "a%20AND%20%28b",
    ] {
        let reply = get(&app, &format!("/api/v1/cache/entries?q={q}")).await;
        assert!(
            reply.status.is_success(),
            "q={q:?} produced {}",
            reply.status
        );
    }
}

#[tokio::test]
async fn an_unparseable_entry_is_listed_but_claims_nothing() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for("opaque");
    put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"not an entry".to_vec(),
    )
    .await;

    let body: Value = get(&app, "/api/v1/cache/entries").await.json();
    let row = &entries(&body)[0];
    assert_eq!(row["key"], json!(key));
    assert_eq!(row["indexed"], json!(true));
    assert_eq!(row["parseable"], json!(false));
    assert!(row["model"].is_null());
    assert!(row["kind"].is_null());
}

// ---------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detail_returns_the_parsed_entry() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut e = entry();
    e.request = Some(json!({"transport": "http", "method": "POST", "url": "https://api/x"}));
    e.usage = Some(TokenUsage {
        input_tokens: 120,
        output_tokens: 45,
        ..Default::default()
    });
    e.reasoning = Some("thinking".into());
    let key = store(&app, "detail", &e).await;

    let body: Value = get(&app, &format!("/api/v1/cache/entries/{key}"))
        .await
        .json();
    assert_eq!(body["key"], json!(key));
    assert_eq!(body["model"], json!("claude-opus-5"));
    assert_eq!(body["input_tokens"], json!(120));
    assert_eq!(body["output_tokens"], json!(45));
    assert_eq!(body["output"], json!("hello"));
    assert_eq!(body["reasoning"], json!("thinking"));
    assert_eq!(body["request"]["url"], json!("https://api/x"));
    assert_eq!(body["parseable"], json!(true));
}

/// `raw` is the largest member and the one least often wanted; sending it on
/// every drawer open would make opening an entry cost the whole payload twice.
#[tokio::test]
async fn detail_withholds_raw_provider_metadata_unless_asked() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut e = entry();
    e.raw = Some(json!({"id": "msg_123", "role": "assistant"}));
    let key = store(&app, "raw", &e).await;

    let lean: Value = get(&app, &format!("/api/v1/cache/entries/{key}"))
        .await
        .json();
    assert!(lean["raw"].is_null(), "raw must be opt-in");

    let full: Value = get(&app, &format!("/api/v1/cache/entries/{key}?raw=true"))
        .await
        .json();
    assert_eq!(full["raw"]["id"], json!("msg_123"));
}

#[tokio::test]
async fn detail_of_an_unparseable_body_says_so_rather_than_failing() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for("badbody");
    put_bytes(&app, &format!("/api/v1/cache/{key}"), None, b"{{{".to_vec()).await;

    let reply = get(&app, &format!("/api/v1/cache/entries/{key}")).await;
    assert_eq!(reply.status, StatusCode::OK);
    let body: Value = reply.json();
    assert_eq!(body["parseable"], json!(false));
    assert!(body["output"].is_null());
    assert_eq!(body["size"], json!(3));
}

// ---------------------------------------------------------------------------
// Facets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn facets_count_the_values_a_filter_can_take() {
    let (app, _dir) = test_app(Settings::default()).await;
    let mut judge = entry();
    judge.kind = Some(EntryKind::new(EntryKind::JUDGE));
    judge.model = Some("gpt-4o".into());
    store(&app, "f1", &judge).await;
    store(&app, "f2", &entry()).await;
    store(&app, "f3", &entry()).await;

    let body: Value = get(&app, "/api/v1/cache/facets").await.json();
    assert_eq!(body["total"], json!(3));
    assert_eq!(body["unindexed"], json!(0));

    let kinds = body["kinds"].as_array().unwrap();
    let provider = kinds
        .iter()
        .find(|k| k["value"] == json!("provider"))
        .expect("provider facet");
    assert_eq!(provider["count"], json!(2));

    let models = body["models"].as_array().unwrap();
    assert!(models.iter().any(|m| m["value"] == json!("gpt-4o")));
}
