//! Request-id correlation: every response carries an `x-request-id` header,
//! generated as a ULID when absent and echoed verbatim when supplied.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::test_app;
use measurellm_server::Settings;
use tower::ServiceExt;

const REQUEST_ID: &str = "x-request-id";

/// A request without an `x-request-id` gets a freshly minted ULID on the
/// response.
#[tokio::test]
async fn response_carries_a_ulid_request_id() {
    let (app, _dir) = test_app(Settings::default()).await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/health")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let id = resp
        .headers()
        .get(REQUEST_ID)
        .expect("x-request-id header present")
        .to_str()
        .expect("x-request-id is valid UTF-8");
    assert!(!id.is_empty(), "x-request-id must not be empty");
    ulid::Ulid::from_string(id).expect("x-request-id parses as a ULID");
}

/// A supplied `x-request-id` is respected and round-trips to the response
/// verbatim rather than being replaced.
#[tokio::test]
async fn incoming_request_id_is_preserved() {
    let (app, _dir) = test_app(Settings::default()).await;
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/health")
        .header(REQUEST_ID, "test-id-123")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let id = resp
        .headers()
        .get(REQUEST_ID)
        .expect("x-request-id header present")
        .to_str()
        .expect("x-request-id is valid UTF-8");
    assert_eq!(id, "test-id-123");
}
