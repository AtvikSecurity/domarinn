//! measurellm-server — the self-hostable results server + embedded web UI.
//!
//! Phase 0 stands up the app shell: liveness, the `/api/v1/meta` capability
//! endpoint the UI reads on boot, and an SPA fallback. Storage, run ingest,
//! auth, cache endpoints, and the real UI land in later phases.

use axum::{
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
    Json, Router,
};
use serde_json::json;

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub data_dir: std::path::PathBuf,
    /// Auth mode reported to the UI. Phase 0 is always open.
    pub auth_mode: AuthMode,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            port: 8321,
            data_dir: std::path::PathBuf::from("/data"),
            auth_mode: AuthMode::Open,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Open,
    ProtectWrites,
    Closed,
}

impl AuthMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthMode::Open => "open",
            AuthMode::ProtectWrites => "protect-writes",
            AuthMode::Closed => "closed",
        }
    }
}

/// Build the axum application. Separated from [`serve`] so integration tests can
/// drive it via `oneshot` without binding a socket.
pub fn app(config: ServerConfig) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/health", get(health))
        .route("/api/v1/meta", get(meta))
        .fallback(spa_fallback)
        .with_state(config)
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status": "ok"})))
}

async fn meta(
    axum::extract::State(config): axum::extract::State<ServerConfig>,
) -> impl IntoResponse {
    Json(json!({
        "name": "measurellm",
        "version": measurellm_core::VERSION,
        "auth_mode": config.auth_mode.as_str(),
        "supported_schema_versions": [measurellm_core::RESULT_SCHEMA_VERSION],
    }))
}

async fn spa_fallback() -> impl IntoResponse {
    // Placeholder until the embedded UI ships. The real build embeds web/dist.
    Html(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>measurellm</title></head>\
         <body style=\"font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem\">\
         <h1>measurellm</h1><p>The results UI is not built into this binary yet. \
         The JSON API is available under <code>/api/v1</code>.</p></body></html>",
    )
}

/// Bind and serve until shutdown (Ctrl-C).
pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let port = config.port;
    let app = app(config);
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("measurellm server listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn body_json(router: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let resp = router
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    #[tokio::test]
    async fn health_is_ok() {
        let (status, body) = body_json(app(ServerConfig::default()), "/api/v1/health").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ok");
    }

    #[tokio::test]
    async fn meta_reports_version_and_mode() {
        let (status, body) = body_json(app(ServerConfig::default()), "/api/v1/meta").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["auth_mode"], "open");
        assert_eq!(body["supported_schema_versions"][0], 1);
    }

    #[tokio::test]
    async fn unknown_path_serves_spa() {
        let resp = app(ServerConfig::default())
            .oneshot(
                Request::builder()
                    .uri("/runs/whatever")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
