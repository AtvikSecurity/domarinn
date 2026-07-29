//! JSON-RPC method routing and per-method authorization.
//!
//! Authorization is decided **per method**, not per route. The [`Scoped`]
//! extractor cannot be used here: it rejects before the handler body runs, so
//! it would 401 the whole endpoint — including `server/discover`, which is
//! precisely what a client sends first to work out what it is talking to. A
//! 401 there reads as a broken server rather than one that needs a token.
//!
//! The check itself delegates to [`Identity::check`], so `open` /
//! `protect-writes` / `closed` behave exactly as they do everywhere else. A
//! second copy of that matrix would be a security bug waiting to happen.
//!
//! [`Scoped`]: crate::auth::Scoped

use axum::http::StatusCode;
use serde_json::{json, Value};

use super::proto::{CacheHint, Rejection};
use super::ratelimit::RateLimiter;
use super::{budget, catalog, jsonrpc, prompts, tools};
use crate::auth::{Access, Identity, Scope};
use crate::AppState;

/// A method's answer, before era decoration.
pub struct Outcome {
    pub payload: Value,
    /// Present only for the results the spec marks cacheable.
    pub cache: Option<CacheHint>,
}

impl Outcome {
    fn plain(payload: Value) -> Outcome {
        Outcome {
            payload,
            cache: None,
        }
    }

    fn cacheable(payload: Value, cache: CacheHint) -> Outcome {
        Outcome {
            payload,
            cache: Some(cache),
        }
    }
}

/// Route one request to its handler.
///
/// Era-blind by design: every payload here is shaped once by
/// [`super::proto::decorate`] on the way out, so nothing in this module or
/// below it needs to know which revision the caller speaks.
pub async fn dispatch(
    state: &AppState,
    limiter: &RateLimiter,
    identity: &Identity,
    incoming: &jsonrpc::Incoming,
) -> Result<Outcome, Rejection> {
    match incoming.method.as_str() {
        // -- Unauthenticated: discovery and liveness ------------------------
        //
        // These leak nothing — the catalogs are compiled in and identical for
        // every caller — and gating them would break client bootstrap for no
        // security gain.
        "server/discover" => Ok(Outcome::cacheable(
            catalog::discover(),
            catalog::discover_cache(),
        )),
        "initialize" => Ok(Outcome::plain(catalog::initialize_result(
            negotiate_version(incoming),
        ))),
        "ping" => Ok(Outcome::plain(json!({}))),
        "tools/list" => Ok(Outcome::cacheable(
            catalog::tools_list(),
            catalog::tools_cache(),
        )),
        "prompts/list" => Ok(Outcome::cacheable(
            catalog::prompts_list(),
            catalog::prompts_cache(),
        )),

        // Unauthenticated because it reads no storage: a prompt renders
        // instructions naming tools and ids the caller already supplied. The
        // moment a prompt reads run data, this must move below the line.
        "prompts/get" => {
            let name = incoming.name_field().unwrap_or_default().to_string();
            let arguments = incoming
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            prompts::get(&name, &arguments)
                .map(Outcome::plain)
                .map_err(|e| Rejection::new(StatusCode::OK, jsonrpc::INVALID_PARAMS, e.message()))
        }

        // -- Authenticated: everything that reads stored eval data ----------
        "tools/call" => {
            authorize(identity, Scope::Read)?;
            if let Err(retry_after) = limiter.check(identity) {
                return Err(Rejection::new(
                    StatusCode::TOO_MANY_REQUESTS,
                    jsonrpc::RATE_LIMITED,
                    format!(
                        "rate limit exceeded; retry in {} seconds",
                        retry_after.as_secs()
                    ),
                )
                .with_data(json!({ "retryAfterSeconds": retry_after.as_secs() })));
            }
            call_tool(state, incoming).await
        }

        _ => Err(Rejection::new(
            StatusCode::NOT_FOUND,
            jsonrpc::METHOD_NOT_FOUND,
            format!("Method not found: {}", incoming.method),
        )),
    }
}

/// Run a tool and wrap it in a `CallToolResult`.
///
/// Takes no era: the payload is built era-blind and shaped once by
/// [`super::proto::decorate`] on the way out.
async fn call_tool(state: &AppState, incoming: &jsonrpc::Incoming) -> Result<Outcome, Rejection> {
    let name = incoming.name_field().unwrap_or_default().to_string();
    if name.is_empty() {
        return Err(Rejection::new(
            StatusCode::OK,
            jsonrpc::INVALID_PARAMS,
            "tools/call requires params.name",
        ));
    }

    let result = tools::call(state, &name, incoming.arguments())
        .await
        .map_err(|_| {
            // An unknown tool is a *protocol* error, not a tool execution
            // error: the model cannot fix it by adjusting arguments.
            Rejection::new(
                StatusCode::OK,
                jsonrpc::METHOD_NOT_FOUND,
                format!("Unknown tool: {name}"),
            )
        })?;

    let mut payload = json!({
        "content": [ { "type": "text", "text": result.text } ],
        "structuredContent": result.structured,
        "isError": result.is_error,
    });

    // Last line of defense: a tool that somehow produced an oversized payload
    // must not be allowed to blow the caller's context window. Every tool
    // already checks this, so reaching here means one of them has a bug.
    if !budget::fits(&payload) {
        tracing::warn!(tool = %name, "mcp tool result exceeded the response budget");
        let message = "result exceeded the response budget. Narrow the filters or lower `limit`.";
        payload = json!({
            "content": [ { "type": "text", "text": message } ],
            "structuredContent": { "error": message },
            "isError": true,
        });
    }

    Ok(Outcome::plain(payload))
}

/// Which legacy revision to reply with.
///
/// Echo the client's request when we speak it, otherwise name our preferred
/// legacy version — a legacy client has no fall-forward mechanism, so the
/// reply is the only chance to tell it something useful.
fn negotiate_version(incoming: &jsonrpc::Incoming) -> &'static str {
    let requested = incoming
        .params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    jsonrpc::LEGACY_VERSIONS
        .into_iter()
        .find(|v| *v == requested)
        .unwrap_or(jsonrpc::LEGACY_VERSIONS[0])
}

/// Enforce a scope using the crate's single authorization matrix.
fn authorize(identity: &Identity, required: Scope) -> Result<(), Rejection> {
    match identity.check(required) {
        Access::Granted => Ok(()),
        Access::Unauthenticated => Err(Rejection::new(
            StatusCode::UNAUTHORIZED,
            jsonrpc::AUTH_REQUIRED,
            "authentication required: present a domarinn API key or token as \
             `Authorization: Bearer <token>`",
        )),
        Access::Forbidden => Err(Rejection::new(
            StatusCode::FORBIDDEN,
            jsonrpc::AUTH_REQUIRED,
            "insufficient scope: this endpoint requires at least `read`",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthMode;

    fn incoming(method: &str, params: Value) -> jsonrpc::Incoming {
        jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params
        }))
        .unwrap()
    }

    fn identity(mode: AuthMode, scope: Option<Scope>) -> Identity {
        let mut identity = Identity::anonymous(mode);
        identity.scope = scope;
        identity
    }

    #[test]
    fn legacy_version_negotiation_echoes_what_we_speak() {
        for version in jsonrpc::LEGACY_VERSIONS {
            let request = incoming("initialize", json!({ "protocolVersion": version }));
            assert_eq!(negotiate_version(&request), version);
        }
    }

    #[test]
    fn an_unknown_requested_version_falls_back_to_our_preferred_legacy() {
        let request = incoming("initialize", json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(negotiate_version(&request), jsonrpc::LEGACY_VERSIONS[0]);
        let bare = incoming("initialize", json!({}));
        assert_eq!(negotiate_version(&bare), jsonrpc::LEGACY_VERSIONS[0]);
    }

    #[test]
    fn closed_mode_requires_a_credential_for_reads() {
        let anon = identity(AuthMode::Closed, None);
        let rejection = authorize(&anon, Scope::Read).unwrap_err();
        assert_eq!(rejection.status, StatusCode::UNAUTHORIZED);
        assert_eq!(rejection.code, jsonrpc::AUTH_REQUIRED);

        let reader = identity(AuthMode::Closed, Some(Scope::Read));
        assert!(authorize(&reader, Scope::Read).is_ok());
    }

    #[test]
    fn the_permissive_modes_waive_reads() {
        for mode in [AuthMode::Open, AuthMode::ProtectWrites] {
            let anon = identity(mode, None);
            assert!(
                authorize(&anon, Scope::Read).is_ok(),
                "{mode:?} should allow anonymous reads"
            );
        }
    }

    #[test]
    fn a_credential_below_the_required_scope_is_forbidden_not_unauthorized() {
        let reader = identity(AuthMode::Closed, Some(Scope::Read));
        let rejection = authorize(&reader, Scope::Admin).unwrap_err();
        assert_eq!(rejection.status, StatusCode::FORBIDDEN);
    }
}
