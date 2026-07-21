//! Integration tests for the embedded web UI: static asset serving and the
//! SPA fallback, plus isolation of the `/api/` namespace from the fallback.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use tower::ServiceExt;

use common::test_app;
use domarinn_server::Settings;

/// A response captured with the headers we care about.
struct Resp {
    status: StatusCode,
    content_type: Option<String>,
    cache_control: Option<String>,
    body: Vec<u8>,
}

impl Resp {
    fn body_lossy(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

async fn get(app: &Router, uri: &str) -> Resp {
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let content_type = header("content-type");
    let cache_control = header("cache-control");
    let body = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    Resp {
        status,
        content_type,
        cache_control,
        body,
    }
}

// ---------------------------------------------------------------------------
// dist fixtures (read straight off disk so tests do not depend on hashes)
// ---------------------------------------------------------------------------

fn dist_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/dist")
}

fn index_bytes() -> Vec<u8> {
    std::fs::read(dist_dir().join("index.html")).expect("web/dist/index.html should exist")
}

/// Return the (name, bytes) of the lexicographically-first file Vite emitted
/// under `web/dist/assets`, or `None` if the UI has not been built.
fn first_asset() -> Option<(String, Vec<u8>)> {
    let dir = dist_dir().join("assets");
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_file())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    let entry = entries.into_iter().next()?;
    let bytes = std::fs::read(entry.path()).ok()?;
    Some((entry.file_name().to_string_lossy().into_owned(), bytes))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spa_fallback_serves_index_html() {
    let (app, _dir) = test_app(Settings::default()).await;
    let r = get(&app, "/").await;
    assert_eq!(r.status, StatusCode::OK);
    let ct = r.content_type.clone().unwrap_or_default();
    assert!(ct.starts_with("text/html"), "content-type was {ct:?}");
    assert!(
        r.body_lossy().to_lowercase().contains("<!doctype html"),
        "body should be an HTML document: {}",
        r.body_lossy()
    );
    // The shell must not be cached long-term (it references hashed assets).
    assert_eq!(r.cache_control.as_deref(), Some("no-cache"));
}

#[tokio::test]
async fn spa_fallback_covers_client_side_routes() {
    let (app, _dir) = test_app(Settings::default()).await;
    // A deep client route that is not a real file must still return the shell.
    let r = get(&app, "/runs/some-run-id").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r
        .content_type
        .clone()
        .unwrap_or_default()
        .starts_with("text/html"));
    assert_eq!(r.body, index_bytes());
}

#[tokio::test]
async fn served_index_matches_embedded_source() {
    // Proves we serve the real embedded index.html, not a hardcoded placeholder.
    let (app, _dir) = test_app(Settings::default()).await;
    let r = get(&app, "/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, index_bytes());
}

#[tokio::test]
async fn unknown_api_route_is_json_404_not_spa() {
    let (app, _dir) = test_app(Settings::default()).await;
    let r = get(&app, "/api/v1/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // Critically: an unknown API route must NEVER be answered with index.html.
    assert!(
        !r.body_lossy().to_lowercase().contains("<!doctype"),
        "api 404 must not be the SPA shell: {}",
        r.body_lossy()
    );
    let ct = r.content_type.clone().unwrap_or_default();
    assert!(
        ct.starts_with("application/json"),
        "content-type was {ct:?}"
    );
    let value: serde_json::Value = serde_json::from_slice(&r.body).expect("json error body");
    assert!(
        value.get("error").is_some(),
        "expected an error field: {value}"
    );
}

#[tokio::test]
async fn embedded_asset_has_immutable_cache_and_content() {
    let (app, _dir) = test_app(Settings::default()).await;
    let Some((name, disk_bytes)) = first_asset() else {
        // UI not built (no web/dist/assets); the hashed-asset path cannot be
        // exercised. Other tests still cover fallback + api-404 isolation.
        eprintln!("skipping: web/dist/assets not present");
        return;
    };
    let r = get(&app, &format!("/assets/{name}")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.cache_control.as_deref(),
        Some("public, max-age=31536000, immutable"),
        "hashed assets must be immutably cacheable"
    );
    assert!(r.content_type.is_some(), "asset needs a content-type");
    assert_eq!(r.body, disk_bytes, "served asset bytes must match the file");
}

#[tokio::test]
async fn missing_asset_is_404_not_spa() {
    let (app, _dir) = test_app(Settings::default()).await;
    let r = get(&app, "/assets/definitely-not-real-1234567890.js").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // A missing hashed asset must 404, not silently serve the SPA shell.
    assert!(
        !r.body_lossy().to_lowercase().contains("<!doctype"),
        "missing asset must not fall through to the SPA shell"
    );
}
