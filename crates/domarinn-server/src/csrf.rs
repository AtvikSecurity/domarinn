//! CSRF defense-in-depth for cookie-authed mutations; see [`csrf_middleware`].

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::{auth, AppState};

/// CSRF defense-in-depth for cookie-authed mutations. The session cookie is
/// `SameSite=Lax`, which already withholds it from cross-site POSTs in every
/// current browser; this middleware additionally rejects unsafe-method
/// requests whose identity was resolved *from the cookie* when they present
/// an `Origin` (or `Referer`) that matches neither `DOMARINN_PUBLIC_URL` nor
/// the request's own `Host`. Header-credential traffic (CLI, API keys, static
/// tokens) is structurally exempt — a cross-site attacker cannot set headers.
pub(crate) async fn csrf_middleware(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    use axum::http::Method;
    let safe_method = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let via_cookie = req
        .extensions()
        .get::<auth::Identity>()
        .is_some_and(|identity| identity.via_cookie);
    if !safe_method && via_cookie {
        match presented_origin(req.headers()) {
            // A parseable Origin/Referer: it must match our origin.
            PresentedOrigin::Host(presented) => {
                if !origin_allowed(&presented, state.public_url.as_deref(), req.headers()) {
                    tracing::warn!(origin = %presented, "rejected cross-origin cookie-authed request");
                    return cross_origin_rejected();
                }
            }
            // A header is present but does not parse to a host (e.g. the
            // literal `Origin: null` a sandboxed iframe sends). Fail closed —
            // we cannot prove it is same-origin.
            PresentedOrigin::Unparseable => {
                tracing::warn!("rejected cookie-authed request with an unparseable Origin/Referer");
                return cross_origin_rejected();
            }
            // Neither header present: allow. Browsers always send Origin on
            // cross-site POSTs, so this is non-browser traffic that chose to
            // authenticate with a cookie.
            PresentedOrigin::Absent => {}
        }
    }
    next.run(req).await
}

fn cross_origin_rejected() -> Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        axum::Json(serde_json::json!({ "error": "cross-origin request rejected" })),
    )
        .into_response()
}

pub(crate) enum PresentedOrigin {
    Host(String),
    Unparseable,
    Absent,
}

/// Classify the request's `Origin` (falling back to `Referer`): parsed to a
/// `host[:port]`, present-but-unparseable, or absent.
pub(crate) fn presented_origin(headers: &axum::http::HeaderMap) -> PresentedOrigin {
    let Some(value) = headers
        .get(axum::http::header::ORIGIN)
        .or_else(|| headers.get(axum::http::header::REFERER))
    else {
        return PresentedOrigin::Absent;
    };
    match value.to_str().ok().and_then(url_host) {
        Some(host) => PresentedOrigin::Host(host),
        None => PresentedOrigin::Unparseable,
    }
}

/// Extract `host[:port]` (lowercased) from an absolute URL. Returns `None`
/// for anything that does not look like `scheme://host...` — including the
/// literal `Origin: null`.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn origin_allowed(
    presented: &str,
    public_url: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> bool {
    if let Some(public_host) = public_url.and_then(url_host) {
        if presented == public_host {
            return true;
        }
    }
    if let Some(host) = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        if presented == host.to_ascii_lowercase() {
            return true;
        }
    }
    false
}
