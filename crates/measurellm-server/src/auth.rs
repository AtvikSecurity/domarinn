//! Token authentication and scope enforcement.
//!
//! Tokens are configured via `MEASURELLM_TOKENS="scope:token,scope:token"`.
//! The active [`AuthMode`](crate::AuthMode) is derived from whether any tokens
//! exist (see [`crate`] docs). A request passes through [`authenticate`], which
//! attaches an [`Identity`] extension; individual routes then demand a scope via
//! the [`Scoped`] extractor. The middleware itself never rejects — enforcement
//! lives entirely in the extractor so unauthenticated reads stay open in the
//! relevant modes.

use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::{request::Parts, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::AuthMode;

/// Access scopes, ordered so that a higher scope subsumes lower ones
/// (`Admin` ⊃ `Write` ⊃ `Read`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Read = 1,
    Write = 2,
    Admin = 3,
}

impl Scope {
    fn parse(s: &str) -> Option<Scope> {
        match s.trim() {
            "read" => Some(Scope::Read),
            "write" => Some(Scope::Write),
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }
}

/// A successful token match: which scope it grants and a human label.
#[derive(Debug, Clone)]
pub struct Grant {
    pub scope: Scope,
    pub label: String,
}

/// Authenticates a bearer token into a [`Grant`].
///
/// A future `OidcAuthenticator` would live alongside this: JWTs contain dots
/// (`header.payload.signature`) whereas the static `mllm_` tokens never do, so a
/// composite authenticator could branch on that and validate JWTs here.
pub trait Authenticator: Send + Sync {
    fn authenticate(&self, token: &str) -> Option<Grant>;
    /// Whether any tokens are configured at all (drives the default mode).
    fn has_tokens(&self) -> bool;
}

/// Constant-time comparison against a fixed set of tokens.
pub struct StaticTokenAuthenticator {
    tokens: Vec<(Scope, String)>,
}

impl StaticTokenAuthenticator {
    pub fn new(tokens: Vec<(Scope, String)>) -> Self {
        StaticTokenAuthenticator { tokens }
    }

    /// Parse a `MEASURELLM_TOKENS` value: comma-separated `scope:token` pairs.
    pub fn from_env_value(raw: &str) -> Self {
        let mut tokens = Vec::new();
        for entry in raw.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            if let Some((scope, token)) = entry.split_once(':') {
                if let Some(scope) = Scope::parse(scope) {
                    let token = token.trim();
                    if !token.is_empty() {
                        tokens.push((scope, token.to_string()));
                    }
                }
            }
        }
        StaticTokenAuthenticator::new(tokens)
    }
}

impl Authenticator for StaticTokenAuthenticator {
    fn authenticate(&self, token: &str) -> Option<Grant> {
        let presented = token.as_bytes();
        let mut best: Option<Scope> = None;
        // Compare against every configured token to avoid short-circuit timing.
        for (scope, configured) in &self.tokens {
            let matches: bool = configured.as_bytes().ct_eq(presented).into();
            if matches {
                best = Some(best.map_or(*scope, |b| b.max(*scope)));
            }
        }
        best.map(|scope| Grant {
            scope,
            label: scope.label().to_string(),
        })
    }

    fn has_tokens(&self) -> bool {
        !self.tokens.is_empty()
    }
}

/// The authenticated (or anonymous) identity attached to every request.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The scope granted by a valid token, or `None` when anonymous.
    pub scope: Option<Scope>,
    /// The active auth mode.
    pub mode: AuthMode,
    /// The token's scope label, used for `uploaded_by`.
    pub label: Option<String>,
}

/// The outcome of checking an identity against a route's required scope.
enum Access {
    Granted,
    Unauthenticated,
    Forbidden,
}

impl Identity {
    fn check(&self, route: Scope) -> Access {
        // Determine the scope actually required, given the mode.
        let required = match self.mode {
            AuthMode::Open => None,
            AuthMode::ProtectWrites => {
                if route == Scope::Read {
                    None
                } else {
                    Some(route)
                }
            }
            AuthMode::Closed => Some(route),
        };
        let Some(required) = required else {
            return Access::Granted;
        };
        match self.scope {
            Some(granted) if granted >= required => Access::Granted,
            Some(_) => Access::Forbidden,
            None => Access::Unauthenticated,
        }
    }
}

/// Build an [`Identity`] from the request headers.
pub fn authenticate(auth: &dyn Authenticator, mode: AuthMode, headers: &HeaderMap) -> Identity {
    let grant = bearer_token(headers).and_then(|token| auth.authenticate(&token));
    Identity {
        scope: grant.as_ref().map(|g| g.scope),
        mode,
        label: grant.map(|g| g.label),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("Bearer ") {
        Some(rest.trim().to_string())
    } else if let Some(rest) = trimmed.strip_prefix("bearer ") {
        Some(rest.trim().to_string())
    } else {
        Some(trimmed.to_string())
    }
}

// ---------------------------------------------------------------------------
// Scope extractor
// ---------------------------------------------------------------------------

/// Marker for the scope a route requires.
pub trait ScopeMarker {
    const REQUIRED: Scope;
}

/// Read scope (list/get endpoints).
pub struct Read;
/// Write scope (ingest, baseline, cache PUT).
pub struct Write;
/// Admin scope (delete, prune).
pub struct Admin;

impl ScopeMarker for Read {
    const REQUIRED: Scope = Scope::Read;
}
impl ScopeMarker for Write {
    const REQUIRED: Scope = Scope::Write;
}
impl ScopeMarker for Admin {
    const REQUIRED: Scope = Scope::Admin;
}

/// Extractor that enforces the marker's scope and yields the [`Identity`].
pub struct Scoped<M> {
    pub identity: Identity,
    _marker: PhantomData<M>,
}

impl<S, M> FromRequestParts<S> for Scoped<M>
where
    S: Send + Sync,
    M: ScopeMarker,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let identity = parts.extensions.get::<Identity>().cloned().ok_or_else(|| {
            error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "auth middleware not installed",
            )
        })?;
        match identity.check(M::REQUIRED) {
            Access::Granted => Ok(Scoped {
                identity,
                _marker: PhantomData,
            }),
            Access::Unauthenticated => Err(error_response(
                StatusCode::UNAUTHORIZED,
                "authentication required",
            )),
            Access::Forbidden => Err(error_response(StatusCode::FORBIDDEN, "insufficient scope")),
        }
    }
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_token_env() {
        let auth = StaticTokenAuthenticator::from_env_value(
            "write:mllm_ci, admin:mllm_ops,read:mllm_view",
        );
        assert!(auth.has_tokens());
        assert_eq!(auth.authenticate("mllm_ci").unwrap().scope, Scope::Write);
        assert_eq!(auth.authenticate("mllm_ops").unwrap().scope, Scope::Admin);
        assert_eq!(auth.authenticate("mllm_view").unwrap().scope, Scope::Read);
        assert!(auth.authenticate("nope").is_none());
    }

    #[test]
    fn scope_hierarchy() {
        assert!(Scope::Admin > Scope::Write);
        assert!(Scope::Write > Scope::Read);
    }

    #[test]
    fn open_mode_grants_everything() {
        let id = Identity {
            scope: None,
            mode: AuthMode::Open,
            label: None,
        };
        assert!(matches!(id.check(Scope::Admin), Access::Granted));
    }

    #[test]
    fn protect_writes_allows_anonymous_reads() {
        let id = Identity {
            scope: None,
            mode: AuthMode::ProtectWrites,
            label: None,
        };
        assert!(matches!(id.check(Scope::Read), Access::Granted));
        assert!(matches!(id.check(Scope::Write), Access::Unauthenticated));
    }

    #[test]
    fn closed_mode_requires_token_for_reads() {
        let id = Identity {
            scope: None,
            mode: AuthMode::Closed,
            label: None,
        };
        assert!(matches!(id.check(Scope::Read), Access::Unauthenticated));
        let reader = Identity {
            scope: Some(Scope::Read),
            mode: AuthMode::Closed,
            label: None,
        };
        assert!(matches!(reader.check(Scope::Read), Access::Granted));
        assert!(matches!(reader.check(Scope::Write), Access::Forbidden));
    }
}
