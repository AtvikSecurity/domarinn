mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::cache::CacheKey;
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
#[tokio::test]
async fn bare_prune_applies_configured_limits() {
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
