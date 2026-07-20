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
use std::str::FromStr;

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::{request::Parts, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use ts_rs::TS;

use crate::domain::Role;
use crate::storage::{ApiKeyAuth, SessionUser, Storage};
use crate::AuthMode;

/// Prefix for API-key secrets (`mllm_<hex>`).
pub const API_KEY_PREFIX: &str = "mllm_";
/// Prefix for session-token secrets (`mses_<hex>`).
pub const SESSION_PREFIX: &str = "mses_";
/// Random bytes behind every session token / API key (256 bits of entropy).
const RANDOM_BYTES: usize = 32;
/// How many leading characters of a key are stored/displayed as its `prefix`.
const PREFIX_LEN: usize = 12;
/// Session lifetime: 30 days.
const SESSION_TTL_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Access scopes, ordered so that a higher scope subsumes lower ones
/// (`Admin` ⊃ `Write` ⊃ `Read`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read = 1,
    Write = 2,
    Admin = 3,
}

impl Scope {
    pub(crate) fn parse(s: &str) -> Option<Scope> {
        s.trim().parse().ok()
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Scope::Read => "read",
            Scope::Write => "write",
            Scope::Admin => "admin",
        }
    }

    /// Scope granted to a user of the given role.
    pub(crate) fn for_role(role: Role) -> Scope {
        match role {
            Role::Admin => Scope::Admin,
            Role::Member => Scope::Write,
        }
    }
}

impl FromStr for Scope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read" => Ok(Scope::Read),
            "write" => Ok(Scope::Write),
            "admin" => Ok(Scope::Admin),
            other => Err(format!(
                "invalid scope '{other}'; expected one of: read, write, admin"
            )),
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

/// Where a request's credentials came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentitySource {
    Anonymous,
    /// A configured `MEASURELLM_TOKENS` static token.
    Static,
    /// A local-account API key (`mllm_...`).
    ApiKey,
    /// A local-account browser session (`mses_...`).
    Session,
}

impl IdentitySource {
    pub fn as_str(&self) -> &'static str {
        match self {
            IdentitySource::Anonymous => "anonymous",
            IdentitySource::Static => "static",
            IdentitySource::ApiKey => "apikey",
            IdentitySource::Session => "session",
        }
    }
}

/// The authenticated (or anonymous) identity attached to every request.
#[derive(Debug, Clone)]
pub struct Identity {
    /// The scope granted by valid credentials, or `None` when anonymous.
    pub scope: Option<Scope>,
    /// The active auth mode.
    pub mode: AuthMode,
    /// A human label for the principal, used for `uploaded_by`
    /// (username for accounts, scope label for static tokens).
    pub label: Option<String>,
    /// The backing user id, when the credentials resolve to a local account.
    pub user_id: Option<String>,
    /// The backing username, when known.
    pub username: Option<String>,
    /// The account role (`admin` | `member`), when known.
    pub role: Option<Role>,
    /// Which authenticator resolved the request.
    pub source: IdentitySource,
    /// The presenting session's token hash, so `logout` can revoke it.
    pub session_token_hash: Option<String>,
}

impl Identity {
    fn anonymous(mode: AuthMode) -> Identity {
        Identity {
            scope: None,
            mode,
            label: None,
            user_id: None,
            username: None,
            role: None,
            source: IdentitySource::Anonymous,
            session_token_hash: None,
        }
    }

    /// Whether any authenticator matched (static token, API key, or session).
    pub fn is_authenticated(&self) -> bool {
        self.source != IdentitySource::Anonymous
    }
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

/// Storage-backed authenticator for local-account API keys (`mllm_...`).
pub struct ApiKeyAuthenticator {
    storage: Storage,
}

impl ApiKeyAuthenticator {
    pub fn new(storage: Storage) -> ApiKeyAuthenticator {
        ApiKeyAuthenticator { storage }
    }

    async fn resolve(&self, token: &str) -> Option<ApiKeyAuth> {
        let prefix = key_prefix(token);
        let hash = token_hash(token);
        self.storage
            .lookup_api_key(prefix, hash)
            .await
            .ok()
            .flatten()
    }
}

/// Storage-backed authenticator for local-account sessions (`mses_...`).
pub struct SessionAuthenticator {
    storage: Storage,
}

impl SessionAuthenticator {
    pub fn new(storage: Storage) -> SessionAuthenticator {
        SessionAuthenticator { storage }
    }

    async fn resolve(&self, token: &str) -> Option<SessionUser> {
        self.storage
            .lookup_session(token_hash(token))
            .await
            .ok()
            .flatten()
    }
}

/// Build an [`Identity`] from the request headers, consulting the full
/// authenticator chain in order: static token → API key → session. Static
/// tokens are matched first (constant-time, no I/O); the account-backed
/// lookups are dispatched by token prefix so at most one DB hit occurs.
pub async fn authenticate(
    static_auth: &dyn Authenticator,
    api_keys: &ApiKeyAuthenticator,
    sessions: &SessionAuthenticator,
    mode: AuthMode,
    headers: &HeaderMap,
) -> Identity {
    let mut identity = Identity::anonymous(mode);
    let Some(token) = bearer_token(headers) else {
        return identity;
    };

    // 1. Static tokens (exact, constant-time match against configuration).
    if let Some(grant) = static_auth.authenticate(&token) {
        identity.scope = Some(grant.scope);
        identity.label = Some(grant.label);
        identity.source = IdentitySource::Static;
        return identity;
    }

    // 2. Account API keys.
    if token.starts_with(API_KEY_PREFIX) {
        if let Some(key) = api_keys.resolve(&token).await {
            identity.scope = Some(key.scope);
            identity.label = Some(key.username.clone());
            identity.user_id = Some(key.user_id);
            identity.username = Some(key.username);
            identity.role = Some(key.role);
            identity.source = IdentitySource::ApiKey;
            return identity;
        }
    }

    // 3. Account sessions.
    if token.starts_with(SESSION_PREFIX) {
        if let Some(user) = sessions.resolve(&token).await {
            identity.scope = Some(Scope::for_role(user.role));
            identity.label = Some(user.username.clone());
            identity.user_id = Some(user.user_id);
            identity.username = Some(user.username);
            identity.role = Some(user.role);
            identity.source = IdentitySource::Session;
            identity.session_token_hash = Some(token_hash(&token));
            return identity;
        }
    }

    identity
}

// ---------------------------------------------------------------------------
// Password hashing (argon2) and token generation
// ---------------------------------------------------------------------------

/// Hash a plaintext password into an argon2 PHC string with a random salt.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    let mut salt_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut salt_bytes);
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|e| anyhow::anyhow!("encoding salt: {e}"))?;
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("hashing password: {e}"))?
        .to_string();
    Ok(hash)
}

/// Verify a plaintext password against a stored argon2 PHC string.
pub fn verify_password(stored_hash: &str, password: &str) -> bool {
    match PasswordHash::new(stored_hash) {
        Ok(parsed) => Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

fn random_hex() -> String {
    let mut buf = [0u8; RANDOM_BYTES];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Mint a fresh session token (`mses_<64 hex>`).
pub fn generate_session_token() -> String {
    format!("{SESSION_PREFIX}{}", random_hex())
}

/// Mint a fresh API key (`mllm_<64 hex>`).
pub fn generate_api_key() -> String {
    format!("{API_KEY_PREFIX}{}", random_hex())
}

/// The leading `PREFIX_LEN` characters of a key, used for display + lookup.
pub fn key_prefix(token: &str) -> String {
    token.chars().take(PREFIX_LEN).collect()
}

/// sha256 hex of a high-entropy secret (session token or API key). argon2 is
/// reserved for low-entropy user passwords; these tokens need only a fast,
/// pre-image-resistant hash.
pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// The expiry timestamp (epoch ms) for a session minted now.
pub fn session_expiry(now_ms: i64) -> i64 {
    now_ms + SESSION_TTL_MS
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

    fn ident(scope: Option<Scope>, mode: AuthMode) -> Identity {
        Identity {
            scope,
            ..Identity::anonymous(mode)
        }
    }

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
    fn scope_serde_matches_label() {
        for scope in [Scope::Read, Scope::Write, Scope::Admin] {
            let serde_value = serde_json::to_value(scope).unwrap();
            assert_eq!(serde_value, serde_json::json!(scope.label()));
            assert_eq!(scope.label().parse::<Scope>().unwrap(), scope);
        }
        assert!("bogus".parse::<Scope>().is_err());
    }

    #[test]
    fn role_scope_mapping() {
        assert_eq!(Scope::for_role(Role::Admin), Scope::Admin);
        assert_eq!(Scope::for_role(Role::Member), Scope::Write);
    }

    #[test]
    fn open_mode_grants_everything() {
        let id = ident(None, AuthMode::Open);
        assert!(matches!(id.check(Scope::Admin), Access::Granted));
    }

    #[test]
    fn protect_writes_allows_anonymous_reads() {
        let id = ident(None, AuthMode::ProtectWrites);
        assert!(matches!(id.check(Scope::Read), Access::Granted));
        assert!(matches!(id.check(Scope::Write), Access::Unauthenticated));
    }

    #[test]
    fn closed_mode_requires_token_for_reads() {
        let id = ident(None, AuthMode::Closed);
        assert!(matches!(id.check(Scope::Read), Access::Unauthenticated));
        let reader = ident(Some(Scope::Read), AuthMode::Closed);
        assert!(matches!(reader.check(Scope::Read), Access::Granted));
        assert!(matches!(reader.check(Scope::Write), Access::Forbidden));
    }

    #[test]
    fn password_hash_round_trips() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password(&hash, "correct horse battery staple"));
        assert!(!verify_password(&hash, "wrong password"));
        assert!(!verify_password("not-a-phc-string", "whatever"));
    }

    #[test]
    fn token_formats_and_prefix() {
        let session = generate_session_token();
        assert!(session.starts_with("mses_"));
        assert_eq!(session.len(), "mses_".len() + 64);

        let key = generate_api_key();
        assert!(key.starts_with("mllm_"));
        assert_eq!(key.len(), "mllm_".len() + 64);
        assert_eq!(key_prefix(&key).len(), 12);
        assert!(key.starts_with(&key_prefix(&key)));

        // Hashing is deterministic and hides the secret.
        assert_eq!(token_hash(&key), token_hash(&key));
        assert_ne!(token_hash(&key), key);
    }
}
