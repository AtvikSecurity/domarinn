//! The Model Context Protocol endpoint: `POST /api/v1/mcp`.
//!
//! One stateless route, mounted only when `DOMARINN_MCP_ENABLED` is set, that
//! exposes domarinn's eval history to MCP clients as read-only tools and
//! prompts. It sits inside the main router, so it inherits the request-id and
//! tracing layers, request decompression, and — critically — the
//! [`auth_middleware`](crate::auth_middleware) that resolves an
//! [`Identity`](crate::auth::Identity) for every request.
//!
//! ## Why this is small
//!
//! Protocol revision `2026-07-28` retired the `initialize` handshake,
//! `Mcp-Session-Id`, the standalone GET stream, and `Last-Event-ID`
//! resumption. The core is now stateless request/response JSON-RPC, which maps
//! onto a single axum route with no session manager and nothing held between
//! requests. Handshake-era clients are still served (see [`proto::Era`]),
//! because no shipping client speaks the modern revision yet.
//!
//! ## Invariants
//!
//! These are load-bearing. Changing one means changing this doc first.
//!
//! 1. **Never emits `text/event-stream`.** The spec lets a server answer with
//!    JSON *or* SSE, per request. Choosing JSON-only deletes stream framing,
//!    keep-alives, `subscriptions/listen`, MRTR streaming, and
//!    cancellation-by-disconnect. Every tool here answers in milliseconds.
//! 2. **Never mints or echoes `Mcp-Session-Id`, and ignores `Last-Event-ID`.**
//!    The server is stateless in both eras; the session id was always optional.
//! 3. **Recognizes no `Mcp-Param-*` headers**, because no tool parameter is
//!    marked with `x-mcp-header`. Unrecognized fields are ignored per RFC 9110.
//! 4. **Tools read [`Storage`](crate::storage::Storage) directly**, never the
//!    handlers in [`crate::routes`]. Sharing the storage layer is the point;
//!    re-parsing a serialized HTTP response would not be.
//! 5. **Era branching happens once**, in [`proto::decorate`]. An
//!    `if era == Modern` inside a tool or prompt is a bug.
//! 6. **No OAuth is advertised.** The 401 carries a bare
//!    `WWW-Authenticate: Bearer realm="domarinn"` with no `resource_metadata`
//!    parameter — RFC 6750's minimum, deliberately *not* an RFC 9728
//!    advertisement. Some clients discard a configured static `Authorization`
//!    header the moment a server advertises OAuth, which would break the only
//!    authentication path this endpoint has.

pub mod budget;
mod catalog;
mod dispatch;
mod headers;
mod jsonrpc;
mod origin;
mod prompts;
mod proto;
mod ratelimit;
mod text;
mod tools;

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde_json::Value;

use self::origin::OriginPolicy;
use self::proto::{Era, Rejection};
use self::ratelimit::RateLimiter;
use crate::auth::Identity;
use crate::AppState;

/// The MCP endpoint path. Under `/api/` so that a miss falls through to the
/// JSON 404 branch of `spa_fallback` rather than serving the SPA shell.
pub const MCP_PATH: &str = "/api/v1/mcp";

/// Body ceiling for a JSON-RPC message. The global 64 MiB limit exists for run
/// ingest; an MCP request is a few kilobytes at most.
///
/// `DefaultBodyLimit` works by inserting an extension that the body extractor
/// consumes, and inserting *replaces* — so layering it on this method router
/// overrides the global limit for this route alone. Because it sits outside
/// `RequestDecompressionLayer` in the stack, the cap applies to the
/// *decompressed* stream, which is also what defuses a gzip bomb.
const MAX_BODY: usize = 256 * 1024;

/// Everything the endpoint needs beyond [`AppState`]. Its presence on
/// `AppState` *is* the enabled flag.
pub struct McpState {
    origins: OriginPolicy,
    limiter: RateLimiter,
}

impl McpState {
    pub fn new(public_url: Option<&str>, allowed_origins: Option<&str>) -> McpState {
        McpState {
            origins: OriginPolicy::new(public_url, allowed_origins),
            limiter: RateLimiter::default(),
        }
    }
}

/// The MCP routes, to be merged into the main router before its layers are
/// applied so they inherit auth, tracing, and the request id.
pub(crate) fn routes(mcp: &McpState) -> Router<AppState> {
    // GET/DELETE on this path fall through to the method router's own 405 with
    // `Allow: POST`, which is exactly what the spec asks a modern-only
    // endpoint to return for the retired verbs.
    //
    // Both layers are applied to a router holding *only* this route, then the
    // well-known routes are merged on afterwards, so neither the tighter body
    // limit nor CORS leaks onto anything else.
    let endpoint = Router::new()
        .route(MCP_PATH, post(endpoint))
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(mcp.origins.cors_layer());

    // Turn OAuth discovery into a definitive "no". Without these,
    // `spa_fallback` answers any non-`/api/` path with the SPA shell at HTTP
    // 200, so a client probing for protected-resource metadata gets an
    // ambiguous HTML page instead of a clean 404.
    let well_known = Router::new()
        .route("/.well-known/oauth-protected-resource", get(no_oauth))
        .route(
            "/.well-known/oauth-protected-resource/{*rest}",
            get(no_oauth),
        )
        .route("/.well-known/oauth-authorization-server", get(no_oauth))
        .route(
            "/.well-known/oauth-authorization-server/{*rest}",
            get(no_oauth),
        );

    endpoint.merge(well_known)
}

async fn no_oauth() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({
            "error": "this server does not use OAuth; authenticate with \
                      `Authorization: Bearer <domarinn api key or token>`"
        })),
    )
        .into_response()
}

/// The single MCP handler.
///
/// Takes [`Extension<Identity>`] rather than
/// [`Scoped`](crate::auth::Scoped) on purpose: `Scoped` rejects in the
/// extractor, before the body is even read, which would 401 the discovery
/// methods a client must be able to call before it has credentials.
/// Authorization is decided per JSON-RPC method in [`dispatch`].
async fn endpoint(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(mcp) = state.mcp.clone() else {
        // Unreachable while the route is only mounted with state present.
        return error_response(
            &Rejection::new(
                StatusCode::NOT_FOUND,
                jsonrpc::METHOD_NOT_FOUND,
                "the MCP endpoint is not enabled",
            ),
            None,
        );
    };

    // DNS-rebinding defense, before anything else looks at the body.
    if !mcp.origins.permits(&headers) {
        tracing::warn!("rejected mcp request from a disallowed origin");
        return error_response(
            &Rejection::new(
                StatusCode::FORBIDDEN,
                jsonrpc::INVALID_REQUEST,
                "origin not allowed",
            ),
            // No id: the body has deliberately not been parsed yet.
            None,
        );
    }

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(e) => {
            return error_response(
                &Rejection::new(
                    StatusCode::BAD_REQUEST,
                    jsonrpc::PARSE_ERROR,
                    format!("invalid JSON: {e}"),
                ),
                None,
            )
        }
    };

    let incoming = match jsonrpc::parse(&parsed) {
        Ok(incoming) => incoming,
        Err((code, message)) => {
            return error_response(
                &Rejection::new(StatusCode::BAD_REQUEST, code, message),
                None,
            )
        }
    };

    let era = match proto::detect(&headers, &incoming.method) {
        Ok(era) => era,
        Err(rejection) => return error_response(&rejection, incoming.id.as_ref()),
    };

    // A notification is acknowledged and dropped. The spec does not define
    // header requirements for a notification POST, so validation is skipped.
    if incoming.is_notification() {
        tracing::debug!(method = %incoming.method, "mcp notification accepted");
        return StatusCode::ACCEPTED.into_response();
    }

    if era == Era::Modern {
        if let Err(rejection) = proto::validate_meta(&incoming) {
            return error_response(&rejection, incoming.id.as_ref());
        }
        if let Err(rejection) = headers::validate(&headers, &incoming) {
            return error_response(&rejection, incoming.id.as_ref());
        }
    }

    let id = incoming.id.clone().unwrap_or(Value::Null);
    match dispatch::dispatch(&state, &mcp.limiter, &identity, era, &incoming).await {
        Ok(outcome) => {
            let mut payload = outcome.payload;
            proto::decorate(era, &mut payload, outcome.cache);
            let is_error = payload
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            tracing::info!(
                mcp_method = %incoming.method,
                mcp_name = incoming.name_field().unwrap_or("-"),
                era = ?era,
                is_error,
                "mcp request"
            );
            (StatusCode::OK, Json(jsonrpc::success(&id, payload))).into_response()
        }
        Err(rejection) => {
            tracing::info!(
                mcp_method = %incoming.method,
                code = rejection.code,
                status = rejection.status.as_u16(),
                "mcp request rejected"
            );
            error_response(&rejection, incoming.id.as_ref())
        }
    }
}

/// Render a [`Rejection`] as an HTTP response carrying a JSON-RPC error.
///
/// The body shape deliberately diverges from the rest of the API's
/// `{"error": …}`: an MCP client parses JSON-RPC, not domarinn's `ApiError`.
fn error_response(rejection: &Rejection, id: Option<&Value>) -> Response {
    let body = jsonrpc::failure_data(
        id,
        rejection.code,
        rejection.message.clone(),
        rejection.data.clone(),
    );

    let mut response = (rejection.status, Json(body)).into_response();

    if rejection.status == StatusCode::UNAUTHORIZED {
        // RFC 6750's minimum. Note the absent `resource_metadata` parameter —
        // see invariant 6 in the module docs.
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Bearer realm=\"domarinn\""),
        );
    }

    if rejection.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(seconds) = rejection
            .data
            .as_ref()
            .and_then(|d| d.get("retryAfterSeconds"))
            .and_then(Value::as_u64)
        {
            if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
        }
    }

    response
}

/// Build the shared state when MCP is enabled.
pub(crate) fn state_from_settings(
    enabled: bool,
    public_url: Option<&str>,
    allowed_origins: Option<&str>,
) -> Option<Arc<McpState>> {
    enabled.then(|| Arc::new(McpState::new(public_url, allowed_origins)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_is_only_built_when_enabled() {
        assert!(state_from_settings(false, None, None).is_none());
        assert!(state_from_settings(true, None, None).is_some());
    }

    #[test]
    fn an_unauthorized_rejection_carries_www_authenticate_without_oauth_metadata() {
        let rejection = Rejection::new(
            StatusCode::UNAUTHORIZED,
            jsonrpc::AUTH_REQUIRED,
            "authentication required",
        );
        let response = error_response(&rejection, None);
        let header = response
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(header, "Bearer realm=\"domarinn\"");
        assert!(
            !header.contains("resource_metadata"),
            "advertising OAuth breaks static-header auth in real clients"
        );
    }

    #[test]
    fn a_rate_limit_rejection_carries_retry_after() {
        let rejection = Rejection::new(
            StatusCode::TOO_MANY_REQUESTS,
            jsonrpc::RATE_LIMITED,
            "slow down",
        )
        .with_data(serde_json::json!({ "retryAfterSeconds": 7 }));
        let response = error_response(&rejection, None);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "7");
    }

    #[test]
    fn ordinary_rejections_carry_neither_header() {
        let rejection = Rejection::new(StatusCode::NOT_FOUND, jsonrpc::METHOD_NOT_FOUND, "nope");
        let response = error_response(&rejection, None);
        assert!(response.headers().get(header::WWW_AUTHENTICATE).is_none());
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
    }
}
