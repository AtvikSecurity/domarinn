//! The closed-by-default contract: with no explicit mode anywhere, every data
//! endpoint requires auth while the bootstrap surface (health, meta, setup,
//! login, the SPA shell) stays reachable so a fresh install can be claimed.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_server::{AuthMode, Settings};
use serde_json::json;

/// The router as a default deployment sees it: config mode `Closed` (what
/// `ServerConfig::default()` and the CLI now pass), nothing in the env.
async fn closed_app() -> (axum::Router, tempfile::TempDir) {
    test_app_with_mode(Settings::default(), AuthMode::Closed).await
}

#[tokio::test]
async fn closed_gates_data_but_leaves_bootstrap_open() {
    let (app, _dir) = closed_app().await;

    // Data endpoints are gated for anonymous callers.
    assert_eq!(
        get(&app, "/api/v1/runs").await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get(&app, "/api/v1/cache/stats").await.status,
        StatusCode::UNAUTHORIZED
    );

    // The bootstrap surface stays open.
    assert_eq!(get(&app, "/health").await.status, StatusCode::OK);
    assert_eq!(get(&app, "/api/v1/health").await.status, StatusCode::OK);

    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.json()["auth_mode"], "closed");
    assert_eq!(meta.json()["setup_required"], true);

    // `me` reports anonymity rather than rejecting.
    let me = get(&app, "/api/v1/auth/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["authenticated"], false);

    // The SPA shell must load so the login page can render.
    assert_eq!(get(&app, "/").await.status, StatusCode::OK);
    assert_eq!(get(&app, "/login").await.status, StatusCode::OK);
}

#[tokio::test]
async fn closed_first_run_setup_flow() {
    let (app, _dir) = closed_app().await;

    // Claim the instance while zero users exist.
    let created = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({"username": "root", "password": "hunter2hunter2"}),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let token = created.json()["token"].as_str().unwrap().to_string();
    assert_eq!(created.json()["user"]["role"], "admin");

    // The minted session unlocks the gated API.
    assert_eq!(
        get_auth(&app, "/api/v1/runs", Some(&token)).await.status,
        StatusCode::OK
    );

    // Setup is one-shot.
    let again = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({"username": "root2", "password": "hunter2hunter2"}),
    )
    .await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    // Login stays reachable anonymously — and enforces credentials.
    let bad = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({"username": "root", "password": "wrong-password"}),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
    let good = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({"username": "root", "password": "hunter2hunter2"}),
    )
    .await;
    assert_eq!(good.status, StatusCode::OK);
}
