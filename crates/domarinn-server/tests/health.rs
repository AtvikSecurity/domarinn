//! Liveness (`/health`) vs readiness (`/api/v1/ready`): the first answers while
//! the process serves HTTP, the second only while storage answers a query.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_server::{AuthMode, Settings};

#[tokio::test]
async fn ready_is_200_and_open_even_in_closed_mode() {
    // Closed is the default posture; a readiness probe carries no credentials,
    // so the route must sit on the bootstrap surface beside /health.
    let (app, _dir) = test_app_with_mode(Settings::default(), AuthMode::Closed).await;
    let reply = get(&app, "/api/v1/ready").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.json()["status"], "ready");
}

#[tokio::test]
async fn ready_is_503_when_storage_is_gone_while_health_stays_200() {
    // The SQLite failure mode: the data dir vanishes under a running server.
    // Postgres has no directory to pull away, so this case is SQLite-only.
    if pg::backend_is_postgres() {
        return;
    }
    let (app, dir) = test_app(Settings::default()).await;
    std::fs::remove_dir_all(dir.path()).expect("remove data dir");

    let reply = get(&app, "/api/v1/ready").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.json()["status"], "unavailable");
    // Unauthenticated route: the body names the component, never the cause
    // (which would leak filesystem paths or a database host).
    let body = String::from_utf8_lossy(&reply.body);
    assert!(
        !body.contains(&*dir.path().to_string_lossy()),
        "ready must not leak the data dir path: {body}"
    );

    // Liveness is deliberately independent of storage.
    assert_eq!(get(&app, "/api/v1/health").await.status, StatusCode::OK);
}
