mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::cache::{CacheEntry, CacheKey, EntryKind};
use domarinn_core::empty::EmptyReason;
use domarinn_core::types::Output;
use domarinn_server::Settings;
use serde_json::json;

fn key_for(seed: i64) -> String {
    CacheKey::compute(&json!({ "seed": seed })).0
}

#[tokio::test]
async fn cache_put_is_immutable_first_write_wins() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for(1);

    let first = put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"original".to_vec(),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);

    // Second write with different bytes is a no-op returning 200.
    let second = put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"replacement".to_vec(),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK);

    // Stored bytes are still the original.
    let got = get(&app, &format!("/api/v1/cache/{key}")).await;
    assert_eq!(got.status, StatusCode::OK);
    assert_eq!(got.body, b"original");
}

#[tokio::test]
async fn cache_get_head_and_miss() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for(2);

    // HEAD/GET before a write -> 404.
    let head_miss = send(
        &app,
        "HEAD",
        &format!("/api/v1/cache/{key}"),
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(head_miss.status, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, &format!("/api/v1/cache/{key}")).await.status,
        StatusCode::NOT_FOUND
    );

    put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"bytes".to_vec(),
    )
    .await;

    let head_hit = send(
        &app,
        "HEAD",
        &format!("/api/v1/cache/{key}"),
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(head_hit.status, StatusCode::OK);
}

/// The domarinn CLI only ever `GET`s the cache; `HEAD` is an existence probe
/// from something external. The hits/misses counters back the server's headline
/// hit rate, so probes must leave them where they were.
#[tokio::test]
async fn head_probes_do_not_move_the_hit_and_miss_counters() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for(6);
    let uri = format!("/api/v1/cache/{key}");

    // Probe a missing key, store it, probe it again: one 404, one 200.
    let head_miss = send(&app, "HEAD", &uri, None, None, Vec::new()).await;
    assert_eq!(head_miss.status, StatusCode::NOT_FOUND);
    put_bytes(&app, &uri, None, b"bytes".to_vec()).await;
    let head_hit = send(&app, "HEAD", &uri, None, None, Vec::new()).await;
    assert_eq!(head_hit.status, StatusCode::OK);

    let after_probes = get(&app, "/api/v1/cache/stats").await.json();
    assert_eq!(after_probes["hits"], 0, "stats: {after_probes:?}");
    assert_eq!(after_probes["misses"], 0, "stats: {after_probes:?}");

    // GET accounting is unchanged.
    get(&app, &uri).await;
    get(&app, &format!("/api/v1/cache/{}", key_for(998))).await;
    let after_gets = get(&app, "/api/v1/cache/stats").await.json();
    assert_eq!(after_gets["hits"], 1, "stats: {after_gets:?}");
    assert_eq!(after_gets["misses"], 1, "stats: {after_gets:?}");
}

#[tokio::test]
async fn cache_rejects_invalid_key() {
    let (app, _dir) = test_app(Settings::default()).await;
    let bad = get(&app, "/api/v1/cache/not-a-valid-key").await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    let bad_put = put_bytes(&app, "/api/v1/cache/md5:abc", None, b"x".to_vec()).await;
    assert_eq!(bad_put.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn cache_enforces_entry_size_limit() {
    let settings = Settings {
        cache_max_entry_bytes: Some(16),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;
    let key = key_for(3);

    let too_big = put_bytes(&app, &format!("/api/v1/cache/{key}"), None, vec![0u8; 32]).await;
    assert_eq!(too_big.status, StatusCode::PAYLOAD_TOO_LARGE);

    let ok = put_bytes(&app, &format!("/api/v1/cache/{key}"), None, vec![0u8; 8]).await;
    assert_eq!(ok.status, StatusCode::CREATED);
}

#[tokio::test]
async fn cache_stats_track_hits_misses_and_size() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for(4);

    put_bytes(
        &app,
        &format!("/api/v1/cache/{key}"),
        None,
        b"hello".to_vec(),
    )
    .await;
    // one hit
    get(&app, &format!("/api/v1/cache/{key}")).await;
    // one miss
    get(&app, &format!("/api/v1/cache/{}", key_for(999))).await;

    let stats = get(&app, "/api/v1/cache/stats").await;
    assert_eq!(stats.status, StatusCode::OK);
    let body = stats.json();
    assert_eq!(body["entries"], 1);
    assert_eq!(body["total_bytes"], 5);
    assert_eq!(body["hits"], 1);
    assert_eq!(body["misses"], 1);
    assert!(body["oldest_entry_at"].is_string());
}

#[tokio::test]
async fn cache_concurrent_put_same_key_one_wins() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = key_for(5);
    let uri = format!("/api/v1/cache/{key}");

    let a = put_bytes(&app, &uri, None, b"AAAA".to_vec());
    let b = put_bytes(&app, &uri, None, b"BBBB".to_vec());
    let (ra, rb) = tokio::join!(a, b);

    let statuses = [ra.status, rb.status];
    let created = statuses
        .iter()
        .filter(|s| **s == StatusCode::CREATED)
        .count();
    let existing = statuses.iter().filter(|s| **s == StatusCode::OK).count();
    assert_eq!(created, 1, "exactly one PUT should create");
    assert_eq!(existing, 1, "the other should no-op");

    // Whatever won is what is stored and is stable.
    let got = get(&app, &uri).await;
    assert!(got.body == b"AAAA" || got.body == b"BBBB");
}

#[tokio::test]
async fn cache_prune_requires_admin_in_protected_mode() {
    let settings = Settings {
        tokens: Some("admin:domarinn_ops,write:domarinn_ci".to_string()),
        auth_mode: Some(domarinn_server::AuthMode::ProtectWrites),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;

    // No token -> unauthorized.
    let anon = send(&app, "POST", "/api/v1/cache/prune", None, None, Vec::new()).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    // Write token is insufficient.
    let writer = send(
        &app,
        "POST",
        "/api/v1/cache/prune",
        Some("domarinn_ci"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(writer.status, StatusCode::FORBIDDEN);

    // Admin token works.
    let admin = send(
        &app,
        "POST",
        "/api/v1/cache/prune?older_than_days=0",
        Some("domarinn_ops"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(admin.status, StatusCode::OK);
}

/// Regression: an unparameterized `POST /cache/prune` — exactly what the UI
/// "Prune cache" button and a plain admin POST send — must apply the server's
/// configured retention limits (the manual equivalent of the hourly task).
/// With a tiny `max_bytes`, a bare prune has to LRU-evict the entries that
/// overflow the budget. Before the fix a bare prune passed no bounds and
/// silently evicted nothing.
///
/// "Bare" means **no parameter of any kind**, not "neither of the two original
/// parameters". Widened deliberately when the filter set grew: keying the
/// fallback off `older_than_days`/`target_bytes` alone would make
/// `?empty_reason=refusal` apply the age and size limits too, so a request for
/// a scalpel would quietly swing the blunt instrument. The companion test below
/// pins the other half.
#[tokio::test]
async fn a_bare_prune_still_applies_the_configured_retention_limits() {
    let settings = Settings {
        cache_max_bytes: Some(64),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;

    // Three 40-byte entries (~120 bytes) overflow the 64-byte budget.
    for seed in 0..3 {
        let key = key_for(seed);
        let r = put_bytes(&app, &format!("/api/v1/cache/{key}"), None, vec![b'x'; 40]).await;
        assert!(
            matches!(r.status, StatusCode::CREATED | StatusCode::OK),
            "put status: {:?}",
            r.status
        );
    }

    // A bare prune (no query params) evicts down to the configured budget.
    let pruned = send(&app, "POST", "/api/v1/cache/prune", None, None, Vec::new()).await;
    assert_eq!(pruned.status, StatusCode::OK);
    assert!(
        pruned.json()["pruned"].as_u64().unwrap() >= 1,
        "bare prune should evict entries to reach the budget; got {:?}",
        pruned.json()
    );

    // The cache is now within the configured budget.
    let stats = get(&app, "/api/v1/cache/stats").await;
    assert!(
        stats.json()["total_bytes"].as_i64().unwrap() <= 64,
        "cache should be within max_bytes after a bare prune; stats: {:?}",
        stats.json()
    );
}

/// An entry whose output came back empty for `reason`.
fn empty_entry(reason: &str) -> CacheEntry {
    CacheEntry {
        created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        kind: Some(EntryKind::new(EntryKind::PROVIDER)),
        provider_fingerprint: None,
        request: None,
        output: Output::Text(String::new()),
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
        empty_reason: Some(EmptyReason::new(reason)),
        tool_calls: Vec::new(),
        domarinn_version: "0.9.0".into(),
    }
}

async fn seed(app: &axum::Router, seed: i64, entry: &CacheEntry) -> String {
    let key = key_for(seed);
    let reply = put_bytes(
        app,
        &format!("/api/v1/cache/{key}"),
        None,
        serde_json::to_vec(entry).unwrap(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "seeding {seed}");
    key
}

/// The other half of the bare-prune rule: a prune that names a predicate
/// applies **only** that predicate.
///
/// Without this, asking to evict refusals would also silently apply
/// `max_age_days` and `max_bytes` — so a targeted eviction, the whole point of
/// the filter, would throw away the warm cache it was meant to spare.
#[tokio::test]
async fn a_prune_naming_only_an_empty_reason_does_not_also_apply_retention() {
    let settings = Settings {
        // Limits a bare prune would visibly enforce: one byte of budget and a
        // zero-day window. Neither may fire here.
        cache_max_bytes: Some(1),
        cache_max_age_days: Some(0),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;

    let refused = seed(&app, 1, &empty_entry(EmptyReason::REFUSAL)).await;
    let mut good = empty_entry(EmptyReason::REFUSAL);
    good.output = Output::Text("a real answer".into());
    good.empty_reason = None;
    let kept = seed(&app, 2, &good).await;

    let reply = send(
        &app,
        "POST",
        "/api/v1/cache/prune?empty_reason=refusal",
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["pruned"], json!(1));

    assert_eq!(
        get(&app, &format!("/api/v1/cache/{refused}")).await.status,
        StatusCode::NOT_FOUND,
        "the refusal should be gone"
    );
    assert_eq!(
        get(&app, &format!("/api/v1/cache/{kept}")).await.status,
        StatusCode::OK,
        "a prune naming only empty_reason must not apply max_bytes or max_age_days"
    );
}

/// `?empty_reasons=refusal` — the plural typo — must be a `400`.
///
/// `deny_unknown_fields` on the query struct is what makes it one, and dropping
/// it would be worse than a missing feature: an unknown key would be ignored,
/// the request would name no predicate, and the fallback would then apply the
/// server's **full retention limits**. A typo would evict on age and size
/// instead of doing nothing.
#[tokio::test]
async fn a_misspelled_prune_filter_is_rejected_rather_than_ignored() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = seed(&app, 3, &empty_entry(EmptyReason::REFUSAL)).await;

    let reply = send(
        &app,
        "POST",
        "/api/v1/cache/prune?empty_reasons=refusal",
        None,
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        get(&app, &format!("/api/v1/cache/{key}")).await.status,
        StatusCode::OK,
        "a rejected prune must not have evicted anything"
    );
}

/// A negative window puts the cutoff in the *future*, so `older_than_days=-1`
/// matches every entry ever stored. That is live behaviour today, and the wider
/// filter set gives it a second parameter to arrive through.
#[tokio::test]
async fn a_negative_day_count_is_rejected_rather_than_deleting_everything() {
    let (app, _dir) = test_app(Settings::default()).await;
    let key = seed(&app, 4, &empty_entry(EmptyReason::BLANK)).await;

    for query in ["older_than_days=-1", "newer_than_days=-7"] {
        let reply = send(
            &app,
            "POST",
            &format!("/api/v1/cache/prune?{query}"),
            None,
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "for {query}");
    }
    assert_eq!(
        get(&app, &format!("/api/v1/cache/{key}")).await.status,
        StatusCode::OK
    );
}

/// A script doing `curl -X POST ".../cache/prune?empty_reason=$REASON"` with
/// `REASON` unset sends `?empty_reason=`, which splits to no reasons at all.
/// Reading that as "no parameter given" substituted the full configured
/// retention, so a caller asking to remove *nothing* evicted by age and size
/// instead. The presence of the parameter is what decides, not what it parsed
/// into.
#[tokio::test]
async fn an_empty_empty_reason_parameter_prunes_nothing_rather_than_everything() {
    let settings = Settings {
        // Limits a bare prune would visibly enforce.
        cache_max_bytes: Some(1),
        cache_max_age_days: Some(0),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;

    let mut good = empty_entry(EmptyReason::REFUSAL);
    good.output = Output::Text("a real answer".into());
    good.empty_reason = None;
    let kept = seed(&app, 1, &good).await;

    for uri in [
        "/api/v1/cache/prune?empty_reason=",
        "/api/v1/cache/prune?empty_reason=,,",
    ] {
        let reply = send(&app, "POST", uri, None, None, Vec::new()).await;
        assert_eq!(reply.status, StatusCode::OK, "{uri}");
        assert_eq!(reply.json()["pruned"], json!(0), "{uri}");
        assert_eq!(
            get(&app, &format!("/api/v1/cache/{kept}")).await.status,
            StatusCode::OK,
            "{uri} must not fall through to the configured retention limits"
        );
    }
}
