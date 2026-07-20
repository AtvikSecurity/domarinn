mod common;

use axum::http::StatusCode;
use common::*;
use measurellm_server::{AuthMode, Settings};

const TOKENS: &str = "read:mllm_view,write:mllm_ci,admin:mllm_ops";

fn protect_writes() -> Settings {
    Settings {
        tokens: Some(TOKENS.to_string()),
        ..Default::default()
    }
}

fn closed() -> Settings {
    Settings {
        tokens: Some(TOKENS.to_string()),
        auth_mode: Some(AuthMode::Closed),
        ..Default::default()
    }
}

#[tokio::test]
async fn open_mode_allows_everything_anonymously() {
    let (app, _dir) = test_app(Settings::default()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "open");

    // Anonymous write + admin both succeed in open mode.
    let created = post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("o1"))).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let deleted = send(&app, "DELETE", "/api/v1/runs/o1", None, None, Vec::new()).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn protect_writes_reads_open_writes_gated() {
    let (app, _dir) = test_app(protect_writes()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "protect-writes");

    // Reads are open without a token.
    assert_eq!(get(&app, "/api/v1/runs").await.status, StatusCode::OK);

    // Write with no token -> 401.
    let anon = post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("p1"))).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    // Write with a read-only token -> 403.
    let reader = post_json(
        &app,
        "/api/v1/runs",
        Some("mllm_view"),
        &run_value(&simple_run("p1")),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN);

    // Write with a write token -> ok.
    let writer = post_json(
        &app,
        "/api/v1/runs",
        Some("mllm_ci"),
        &run_value(&simple_run("p1")),
    )
    .await;
    assert_eq!(writer.status, StatusCode::CREATED);

    // Admin route (delete) with a write token -> 403.
    let write_delete = send(
        &app,
        "DELETE",
        "/api/v1/runs/p1",
        Some("mllm_ci"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(write_delete.status, StatusCode::FORBIDDEN);

    // Admin route with an admin token -> ok.
    let admin_delete = send(
        &app,
        "DELETE",
        "/api/v1/runs/p1",
        Some("mllm_ops"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(admin_delete.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn closed_mode_gates_reads_too() {
    let (app, _dir) = test_app(closed()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "closed");

    // Open meta/health still work (they are not scoped).
    assert_eq!(get(&app, "/api/v1/health").await.status, StatusCode::OK);

    // Reads require a token.
    assert_eq!(
        get_auth(&app, "/api/v1/runs", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get_auth(&app, "/api/v1/runs", Some("mllm_view"))
            .await
            .status,
        StatusCode::OK
    );

    // Writes still require write scope.
    let reader_write = post_json(
        &app,
        "/api/v1/runs",
        Some("mllm_view"),
        &run_value(&simple_run("c1")),
    )
    .await;
    assert_eq!(reader_write.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_token_is_treated_as_anonymous() {
    let (app, _dir) = test_app(protect_writes()).await;
    let bad = post_json(
        &app,
        "/api/v1/runs",
        Some("not-a-real-token"),
        &run_value(&simple_run("x1")),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
}
