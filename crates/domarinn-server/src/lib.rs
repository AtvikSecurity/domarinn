//! domarinn-server — the self-hostable results server + embedded web UI.
//!
//! The server ingests [`RunResult`](domarinn_core::result::RunResult)
//! documents, stores them in a hybrid SQLite layout (see [`storage`]), and
//! exposes a read/compare/export API plus a shared content-addressed cache.
//!
//! ## Auth
//! The server is **closed by default**: every `/api` call requires
//! authentication (a session, API key, or static token) unless the operator
//! explicitly opts out. The active [`AuthMode`] is resolved at startup:
//! * `DOMARINN_AUTH_MODE` (`open|protect-writes|closed`), when set, always
//!   wins,
//! * otherwise the [`ServerConfig::auth_mode`] value is used verbatim — and
//!   both the CLI and [`ServerConfig::default`] pass [`AuthMode::Closed`].
//!
//! `open` (everything anonymous) and `protect-writes` (anonymous reads,
//! authenticated writes) exist for demos and trusted-network deployments;
//! they are never inferred.
//!
//! Tokens come from `DOMARINN_TOKENS="write:domarinn_ci,admin:domarinn_ops,read:domarinn_view"`.
//! Extra settings (`DOMARINN_PUBLIC_URL`, cache limits) are read from the
//! environment inside [`serve`], never as new [`ServerConfig`] fields.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Router;
use ts_rs::TS;

use crate::auth::{
    ApiKeyAuthenticator, Authenticator, SessionAuthenticator, StaticTokenAuthenticator,
};
use crate::storage::Storage;

pub mod accounts;
pub mod auth;
pub mod cachebrowse;
pub mod domain;
pub mod dto;
pub mod extract;
pub mod localcache;
pub mod mcp;
pub mod routes;
pub mod runsets;
pub mod sso;
pub mod storage;

// Re-exported for the SAML integration tests (tests are separate crates and
// cannot reach optional dependencies directly).
#[cfg(feature = "saml")]
pub use base64;
#[cfg(feature = "saml")]
pub use samael;

/// Default cache limits when the environment does not override them.
const DEFAULT_MAX_ENTRY_BYTES: usize = 4 * 1024 * 1024; // 4 MiB
const DEFAULT_MAX_BYTES: u64 = 1024 * 1024 * 1024; // 1 GiB
const DEFAULT_MAX_AGE_DAYS: u64 = 30;

/// Server configuration.
///
/// Constructed verbatim by the CLI; the public field set is a stable contract
/// (exactly `port`, `data_dir`, `auth_mode`). Any additional settings are read
/// from the environment inside [`serve`].
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub data_dir: std::path::PathBuf,
    /// Requested auth mode, used verbatim unless `DOMARINN_AUTH_MODE`
    /// overrides it. Defaults to [`AuthMode::Closed`].
    pub auth_mode: AuthMode,
}

impl Default for ServerConfig {
    fn default() -> Self {
        ServerConfig {
            port: 8321,
            data_dir: std::path::PathBuf::from("/data"),
            auth_mode: AuthMode::Closed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, TS)]
#[serde(rename_all = "kebab-case")]
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

impl std::str::FromStr for AuthMode {
    type Err = String;

    /// Accepts the canonical `protect-writes` spelling plus the
    /// underscore-separated `protect_writes` alias (handy for shells that
    /// dislike hyphens in env values).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(AuthMode::Open),
            "protect-writes" | "protect_writes" => Ok(AuthMode::ProtectWrites),
            "closed" => Ok(AuthMode::Closed),
            other => Err(format!(
                "invalid DOMARINN_AUTH_MODE '{other}'; expected one of: open, protect-writes, closed"
            )),
        }
    }
}

/// Cache retention/size limits.
#[derive(Debug, Clone, Copy)]
pub struct CacheLimits {
    pub max_entry_bytes: usize,
    pub max_bytes: u64,
    pub max_age_days: u64,
}

impl Default for CacheLimits {
    fn default() -> Self {
        CacheLimits {
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
            max_bytes: DEFAULT_MAX_BYTES,
            max_age_days: DEFAULT_MAX_AGE_DAYS,
        }
    }
}

/// Environment-sourced settings that are not part of the [`ServerConfig`]
/// contract. Populated by [`Settings::from_env`] in production; constructed
/// directly by tests.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// Raw `DOMARINN_TOKENS` value (`scope:token,...`).
    pub tokens: Option<String>,
    /// Explicit `DOMARINN_AUTH_MODE` override (`open|protect-writes|closed`).
    pub auth_mode: Option<AuthMode>,
    /// Public base URL used to build run links (`DOMARINN_PUBLIC_URL`).
    pub public_url: Option<String>,
    pub cache_max_entry_bytes: Option<usize>,
    pub cache_max_bytes: Option<u64>,
    pub cache_max_age_days: Option<u64>,
    /// Delete runs older than this many days (`DOMARINN_RUN_MAX_AGE_DAYS`).
    /// `None` (the default) never deletes a run — eval history is expensive to
    /// produce and impossible to recreate, so retention is opt-in.
    pub run_max_age_days: Option<u64>,
    /// Bootstrap admin username (`DOMARINN_ADMIN_USER`).
    pub admin_user: Option<String>,
    /// Bootstrap admin password (`DOMARINN_ADMIN_PASSWORD`).
    pub admin_password: Option<String>,
    /// Explicit `DOMARINN_COOKIE_SECURE` override. When unset, the session
    /// cookie's `Secure` flag is derived from the `DOMARINN_PUBLIC_URL`
    /// scheme.
    pub cookie_secure: Option<bool>,
    /// SSO providers (`DOMARINN_OIDC_*` / `DOMARINN_SAML_*`).
    pub sso: sso::SsoSettings,
    /// Whether to mount the MCP endpoint (`DOMARINN_MCP_ENABLED`).
    ///
    /// Opt-in rather than always-on: the endpoint is authenticated like every
    /// other `/api/v1` route, but an operator who does not want agents
    /// touching their instance at all should be able to say so without
    /// relying on credential hygiene.
    pub mcp_enabled: Option<bool>,
    /// A local disk cache directory to expose as a second, read-only browsable
    /// tier (`DOMARINN_LOCAL_CACHE_DIR`). `None` — the default — mounts
    /// nothing, and `?tier=local` is then a 404.
    ///
    /// An environment variable rather than a `ServerConfig` field or a CLI
    /// flag: that struct's field set is a documented contract (`port`,
    /// `data_dir`, `auth_mode`), and every other knob here already obeys it.
    pub local_cache_dir: Option<std::path::PathBuf>,
    /// Files the local tier will stat before reporting truncation
    /// (`DOMARINN_LOCAL_CACHE_MAX_SCAN`).
    pub local_cache_max_scan: Option<usize>,
    /// Extra origins allowed to reach the MCP endpoint
    /// (`DOMARINN_MCP_ALLOWED_ORIGINS`, comma-separated). Drives both the
    /// spec-required `Origin` check and the route's CORS layer. When this and
    /// `public_url` are both unset, only loopback is allowed.
    pub mcp_allowed_origins: Option<String>,
}

impl Settings {
    /// Read settings from the process environment. Hard-errors if
    /// `DOMARINN_AUTH_MODE` is set to something unrecognized — falling
    /// through to open mode on a typo would be a silent security downgrade.
    pub fn from_env() -> anyhow::Result<Self> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let auth_mode = parse_auth_mode_env(env("DOMARINN_AUTH_MODE"))?;
        Ok(Settings {
            tokens: env("DOMARINN_TOKENS"),
            auth_mode,
            public_url: env("DOMARINN_PUBLIC_URL"),
            cache_max_entry_bytes: env("DOMARINN_CACHE_MAX_ENTRY_BYTES")
                .and_then(|v| v.parse().ok()),
            cache_max_bytes: env("DOMARINN_CACHE_MAX_BYTES").and_then(|v| v.parse().ok()),
            cache_max_age_days: env("DOMARINN_CACHE_MAX_AGE_DAYS").and_then(|v| v.parse().ok()),
            run_max_age_days: env("DOMARINN_RUN_MAX_AGE_DAYS").and_then(|v| v.parse().ok()),
            admin_user: env("DOMARINN_ADMIN_USER"),
            admin_password: env("DOMARINN_ADMIN_PASSWORD"),
            cookie_secure: parse_bool_env("DOMARINN_COOKIE_SECURE", env("DOMARINN_COOKIE_SECURE"))?,
            sso: sso::parse_sso_settings(&std::env::vars().collect())?,
            // Fail loud on a typo: silently leaving the endpoint unmounted
            // because someone wrote `DOMARINN_MCP_ENABLED=ture` is a bad hour.
            mcp_enabled: parse_bool_env("DOMARINN_MCP_ENABLED", env("DOMARINN_MCP_ENABLED"))?,
            mcp_allowed_origins: env("DOMARINN_MCP_ALLOWED_ORIGINS"),
            local_cache_dir: env("DOMARINN_LOCAL_CACHE_DIR").map(std::path::PathBuf::from),
            local_cache_max_scan: env("DOMARINN_LOCAL_CACHE_MAX_SCAN").and_then(|v| v.parse().ok()),
        })
    }
}

/// Parse a boolean env value, hard-erroring on anything unrecognized — the
/// same fail-loud posture as `DOMARINN_AUTH_MODE`, since a typo here would
/// silently change the session cookie's `Secure` flag.
fn parse_bool_env(name: &str, raw: Option<String>) -> anyhow::Result<Option<bool>> {
    match raw.as_deref() {
        None => Ok(None),
        Some("true") | Some("1") | Some("yes") => Ok(Some(true)),
        Some("false") | Some("0") | Some("no") => Ok(Some(false)),
        Some(other) => {
            anyhow::bail!("invalid {name} '{other}'; expected one of: true, false, 1, 0, yes, no")
        }
    }
}

/// Parse the raw `DOMARINN_AUTH_MODE` env value into an [`AuthMode`].
/// Factored out of [`Settings::from_env`] so the hard-error path is
/// unit-testable without mutating the process environment.
fn parse_auth_mode_env(raw: Option<String>) -> anyhow::Result<Option<AuthMode>> {
    raw.map(|v| v.parse::<AuthMode>().map_err(|e| anyhow::anyhow!(e)))
        .transpose()
}

/// Shared application state (cheap to clone: all fields are `Arc`-backed or `Copy`).
#[derive(Clone)]
pub struct AppState {
    pub(crate) storage: Storage,
    pub(crate) auth: Arc<dyn Authenticator>,
    pub(crate) api_key_auth: Arc<ApiKeyAuthenticator>,
    pub(crate) session_auth: Arc<SessionAuthenticator>,
    pub(crate) auth_mode: AuthMode,
    pub(crate) public_url: Option<String>,
    pub(crate) cache_limits: CacheLimits,
    /// Run-retention window, or `None` to keep every run forever (the default).
    pub(crate) run_max_age_days: Option<u64>,
    /// Whether session cookies carry the `Secure` attribute.
    pub(crate) cookie_secure: bool,
    pub(crate) sso: Arc<sso::SsoRegistry>,
    /// MCP endpoint state. `None` means the endpoint is disabled and its route
    /// is never mounted — presence *is* the feature flag.
    pub(crate) mcp: Option<Arc<mcp::McpState>>,
    /// A mounted local disk cache, browsable read-only as `?tier=local`.
    /// `None` means no such tier exists on this instance.
    pub(crate) local_cache: Option<Arc<localcache::LocalTier>>,
}

impl AppState {
    /// Open storage, bootstrap any configured admin, and derive the effective
    /// auth mode.
    pub async fn new(config: &ServerConfig, settings: Settings) -> anyhow::Result<AppState> {
        let storage = Storage::open(config.data_dir.clone()).await?;
        let authenticator =
            StaticTokenAuthenticator::from_env_value(settings.tokens.as_deref().unwrap_or(""));

        bootstrap_admin(&storage, &settings).await?;

        let auth_mode = resolve_mode(config.auth_mode, settings.auth_mode);
        let cache_limits = CacheLimits {
            max_entry_bytes: settings
                .cache_max_entry_bytes
                .unwrap_or(DEFAULT_MAX_ENTRY_BYTES),
            max_bytes: settings.cache_max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            max_age_days: settings.cache_max_age_days.unwrap_or(DEFAULT_MAX_AGE_DAYS),
        };
        let cookie_secure = settings.cookie_secure.unwrap_or_else(|| {
            settings
                .public_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://"))
        });
        let sso_registry = Arc::new(
            sso::SsoRegistry::from_settings(&settings.sso, settings.public_url.as_deref()).await?,
        );
        let mcp = mcp::state_from_settings(
            settings.mcp_enabled.unwrap_or(false),
            settings.public_url.as_deref(),
            settings.mcp_allowed_origins.as_deref(),
        );
        // A bad path costs one tier, never the process: an operator who
        // renamed a directory should lose the browse view, not their server.
        let local_cache = settings.local_cache_dir.as_ref().and_then(|dir| {
            match localcache::LocalTier::open(dir.clone(), settings.local_cache_max_scan) {
                Ok(tier) => {
                    tracing::info!(path = %tier.root().display(), "mounted local cache tier");
                    Some(Arc::new(tier))
                }
                Err(e) => {
                    tracing::warn!(error = %e, "DOMARINN_LOCAL_CACHE_DIR is unusable; not mounting a local tier");
                    None
                }
            }
        });
        Ok(AppState {
            api_key_auth: Arc::new(ApiKeyAuthenticator::new(storage.clone())),
            session_auth: Arc::new(SessionAuthenticator::new(storage.clone())),
            storage,
            auth: Arc::new(authenticator),
            auth_mode,
            public_url: settings.public_url,
            cache_limits,
            run_max_age_days: settings.run_max_age_days,
            cookie_secure,
            sso: sso_registry,
            mcp,
            local_cache,
        })
    }
}

/// Idempotently ensure the `DOMARINN_ADMIN_USER` / `DOMARINN_ADMIN_PASSWORD`
/// account exists as an enabled admin, updating its password if it changed.
async fn bootstrap_admin(storage: &Storage, settings: &Settings) -> anyhow::Result<()> {
    let (Some(username), Some(password)) = (
        settings.admin_user.as_deref(),
        settings.admin_password.as_deref(),
    ) else {
        return Ok(());
    };
    if username.is_empty() || password.is_empty() {
        return Ok(());
    }

    match storage.get_user_by_username(username.to_string()).await? {
        None => {
            let hash = auth::hash_password(password)?;
            storage
                .create_user(username.to_string(), hash, crate::domain::Role::Admin)
                .await?;
        }
        Some(existing) => {
            if existing.role != crate::domain::Role::Admin {
                storage
                    .set_user_role(existing.id.clone(), crate::domain::Role::Admin)
                    .await?;
            }
            if existing.disabled {
                storage
                    .set_user_disabled(existing.id.clone(), false)
                    .await?;
            }
            if !auth::verify_password(&existing.password_hash, password) {
                let hash = auth::hash_password(password)?;
                storage.update_password(existing.id.clone(), hash).await?;
            }
        }
    }
    Ok(())
}

/// An explicit `DOMARINN_AUTH_MODE` always wins; otherwise the configured
/// mode is used verbatim. Nothing is ever inferred from credentials — an
/// unset environment means the [`ServerConfig`] default, which is
/// [`AuthMode::Closed`].
fn resolve_mode(config_mode: AuthMode, env_mode: Option<AuthMode>) -> AuthMode {
    env_mode.unwrap_or(config_mode)
}

/// Middleware that attaches an [`auth::Identity`] to every request. It never
/// rejects — scope enforcement lives in the [`auth::Scoped`] extractor.
pub(crate) async fn auth_middleware(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let identity = auth::authenticate(
        state.auth.as_ref(),
        state.api_key_auth.as_ref(),
        state.session_auth.as_ref(),
        state.auth_mode,
        req.headers(),
    )
    .await;
    req.extensions_mut().insert(identity);
    next.run(req).await
}

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

enum PresentedOrigin {
    Host(String),
    Unparseable,
    Absent,
}

/// Classify the request's `Origin` (falling back to `Referer`): parsed to a
/// `host[:port]`, present-but-unparseable, or absent.
fn presented_origin(headers: &axum::http::HeaderMap) -> PresentedOrigin {
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
fn url_host(url: &str) -> Option<String> {
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

/// Build the axum application and its state. Exposed so integration tests can
/// drive the router via `oneshot` without binding a socket.
pub async fn build_app(
    config: &ServerConfig,
    settings: Settings,
) -> anyhow::Result<(Router, AppState)> {
    let state = AppState::new(config, settings).await?;
    let router = routes::router(state.clone());
    Ok((router, state))
}

/// Bind and serve until shutdown (Ctrl-C).
pub async fn serve(config: ServerConfig) -> anyhow::Result<()> {
    let settings = Settings::from_env()?;
    let (app, state) = build_app(&config, settings).await?;
    spawn_cache_index_backfill(state.clone());
    spawn_cache_retention(state);

    let port = config.port;
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("domarinn server listening on http://{addr}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Rows per indexing batch.
///
/// Small on purpose. Each batch takes the cache writer mutex to stamp its
/// results, and that mutex is also what every `cache_put` and every `cache_get`
/// needs — `cache_get` is a write, because it bumps the hit counter.
const INDEX_BATCH: i64 = 200;
/// Yielded between batches, so a CI burst arriving mid-backfill meets a bounded
/// stall rather than a stalled cache.
const INDEX_PAUSE: Duration = Duration::from_millis(250);
/// How often to re-check once drained. The probe is one lookup against a
/// partial index that is empty in the steady state, so this costs nothing.
const INDEX_IDLE: Duration = Duration::from_secs(300);

/// Drain the cache migration-2 columns in the background.
///
/// Entries written from now on are indexed by `cache_put` on the way in, so
/// this exists for databases that predate the migration. Deliberately *not* in
/// `storage::backfill`, which runs synchronously before the server binds its
/// port: that is fine for the runs database, bounded by run count, and would
/// turn a restart into an outage here, where a shared cache can hold a million
/// entries of up to 4 MiB each.
///
/// It idles rather than exiting when drained, so a data dir restored from an
/// older backup is indexed without needing a restart to notice.
fn spawn_cache_index_backfill(state: AppState) {
    tokio::spawn(async move {
        loop {
            match state.storage.cache_index_batch(INDEX_BATCH).await {
                Ok(0) => tokio::time::sleep(INDEX_IDLE).await,
                Ok(indexed) => {
                    tracing::debug!(indexed, "cache index backfill progressed");
                    tokio::time::sleep(INDEX_PAUSE).await;
                }
                // Never fatal: an unindexed cache still serves every read and
                // write, it just cannot be filtered or searched yet.
                Err(e) => {
                    tracing::warn!(error = %e, "cache index backfill failed");
                    tokio::time::sleep(INDEX_IDLE).await;
                }
            }
        }
    });
}

/// Hourly LRU/age retention against the configured cache limits, plus the
/// SSO login-transaction / SAML-replay-cache sweep and, when configured, the
/// run sweep.
fn spawn_cache_retention(state: AppState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            let max_age_days = state.cache_limits.max_age_days as i64;
            let target_bytes = state.cache_limits.max_bytes as i64;
            match state
                .storage
                .cache_prune(Some(max_age_days), Some(target_bytes))
                .await
            {
                Ok(pruned) if pruned > 0 => {
                    tracing::info!(pruned, "cache retention evicted entries")
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(error = %e, "cache retention failed"),
            }
            if let Err(e) = state.storage.sso_gc().await {
                tracing::warn!(error = %e, "sso gc failed");
            }
            // Opt-in: `None` never deletes a run. See storage::retention.
            if let Some(days) = state.run_max_age_days {
                match state.storage.sweep_runs(days).await {
                    Ok(swept) if swept.deleted > 0 || swept.kept > 0 => tracing::info!(
                        deleted = swept.deleted,
                        kept = swept.kept,
                        max_age_days = days,
                        "run retention swept"
                    ),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "run retention failed"),
                }
            }
        }
    });
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}

/// Export TypeScript type definitions for the server's DTO layer (response
/// bodies + request bodies) to `dir`.
///
/// Companion to [`domarinn_core::export_types`]: the CLI calls both into
/// the same output directory so `web/src/api/generated/` is the complete,
/// generated TypeScript contract for both the result/diff shapes and the
/// HTTP API. Uses the same [`ts_rs::Config`] (`with_large_int("number")`) so
/// shared core types (e.g. [`domarinn_core::ids::RunId`],
/// [`domarinn_core::result::CaseStatus`]) re-emit byte-identically
/// regardless of which crate's export runs last.
///
/// Each call below is an *export root* — `TS::export_all` walks that type's
/// field graph and also writes every transitive dependency (e.g.
/// [`crate::domain::Role`], [`crate::auth::Scope`], [`AuthMode`],
/// [`crate::auth::IdentitySource`], [`crate::dto::compare::CompareDelta`]).
/// [`crate::domain::RunStatusFilter`] is *not* reachable from any response or
/// request body (it is only used by a `Deserialize`-only query-string struct
/// in `routes.rs`), so it is listed as its own explicit root even though it
/// carries no other consumers here — the web app's run-list filter UI
/// (a later task) is expected to import it directly.
///
/// [`crate::dto::cases::CaseDetailResponse`] is deliberately *not* exported:
/// it is a `#[serde(transparent)]` newtype over `serde_json::Value` with
/// `#[ts(type = "unknown")]` on its sole field, and ts-rs exports single-field
/// tuple structs as a bare type alias — `export type CaseDetailResponse =
/// unknown;`. That adds a file with strictly less information than writing
/// `unknown` at the call site (there is no dependency graph, no shape, no
/// doc comment beyond what is already on the struct), so it is skipped.
pub fn export_api_types(dir: &std::path::Path) -> Result<(), ts_rs::ExportError> {
    use ts_rs::Config;

    use crate::accounts::{CreateKeyBody, CreateUserBody, CredentialsBody, PatchUserBody};
    use crate::domain::{
        CacheSort, CacheTier, CachedFilter, OriginFilter, RunStatusFilter, SortOrder,
    };
    use crate::dto::accounts::{
        ApiKeyCreatedResponse, ApiKeyListResponse, AuthSessionResponse, MeResponse, OkResponse,
        UserListResponse,
    };
    use crate::dto::cache::{CacheStatsResponse, PruneResponse};
    use crate::dto::cacheentries::{
        CacheEntryDetail, CacheEntryListResponse, CacheEntryRunsResponse, CacheFacetsResponse,
    };
    use crate::dto::cases::CaseListResponse;
    use crate::dto::compare::CompareResponse;
    use crate::dto::config::RunConfigResponse;
    use crate::dto::history::CaseHistoryResponse;
    use crate::dto::matrix::MatrixResponse;
    use crate::dto::meta::MetaResponse;
    use crate::dto::projects::{ProjectsResponse, SuitesResponse};
    use crate::dto::runs::{IngestResponse, RunDetailResponse, RunListResponse};
    use crate::dto::search::SearchResponse;
    use crate::routes::BaselineBody;

    let cfg = Config::new().with_out_dir(dir).with_large_int("number");

    // Response DTOs.
    RunListResponse::export_all(&cfg)?;
    RunDetailResponse::export_all(&cfg)?;
    IngestResponse::export_all(&cfg)?;
    CaseListResponse::export_all(&cfg)?;
    CompareResponse::export_all(&cfg)?;
    RunConfigResponse::export_all(&cfg)?;
    CaseHistoryResponse::export_all(&cfg)?;
    MatrixResponse::export_all(&cfg)?;
    ProjectsResponse::export_all(&cfg)?;
    SuitesResponse::export_all(&cfg)?;
    CacheStatsResponse::export_all(&cfg)?;
    CacheEntryListResponse::export_all(&cfg)?;
    CacheEntryDetail::export_all(&cfg)?;
    CacheFacetsResponse::export_all(&cfg)?;
    CacheEntryRunsResponse::export_all(&cfg)?;
    // Query-string enums: `Deserialize`-only, so unreachable from any response
    // graph, and listed as their own roots for the same reason
    // `RunStatusFilter` is — the browse UI imports them to build its controls.
    CacheTier::export_all(&cfg)?;
    CacheSort::export_all(&cfg)?;
    SortOrder::export_all(&cfg)?;
    PruneResponse::export_all(&cfg)?;
    MetaResponse::export_all(&cfg)?;
    SearchResponse::export_all(&cfg)?;
    MeResponse::export_all(&cfg)?;
    AuthSessionResponse::export_all(&cfg)?;
    UserListResponse::export_all(&cfg)?;
    ApiKeyListResponse::export_all(&cfg)?;
    ApiKeyCreatedResponse::export_all(&cfg)?;
    OkResponse::export_all(&cfg)?;

    // Request bodies.
    CredentialsBody::export_all(&cfg)?;
    CreateUserBody::export_all(&cfg)?;
    PatchUserBody::export_all(&cfg)?;
    CreateKeyBody::export_all(&cfg)?;
    BaselineBody::export_all(&cfg)?;

    // Not reachable from any response/request-body root (see the module doc
    // above) but still part of the web-facing TypeScript contract.
    RunStatusFilter::export_all(&cfg)?;
    CachedFilter::export_all(&cfg)?;
    OriginFilter::export_all(&cfg)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_mode_str() {
        assert_eq!(AuthMode::Open.as_str(), "open");
        assert_eq!(AuthMode::ProtectWrites.as_str(), "protect-writes");
        assert_eq!(AuthMode::Closed.as_str(), "closed");
    }

    #[test]
    fn mode_resolution() {
        // Env override always wins.
        assert_eq!(
            resolve_mode(AuthMode::Closed, Some(AuthMode::Open)),
            AuthMode::Open
        );
        assert_eq!(
            resolve_mode(AuthMode::Open, Some(AuthMode::Closed)),
            AuthMode::Closed
        );
        // No env: the configured mode is used verbatim — never derived.
        assert_eq!(resolve_mode(AuthMode::Closed, None), AuthMode::Closed);
        assert_eq!(resolve_mode(AuthMode::Open, None), AuthMode::Open);
        assert_eq!(
            resolve_mode(AuthMode::ProtectWrites, None),
            AuthMode::ProtectWrites
        );
        // The default config is closed.
        assert_eq!(
            resolve_mode(ServerConfig::default().auth_mode, None),
            AuthMode::Closed
        );
    }

    #[test]
    fn auth_mode_serializes_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(AuthMode::Open).unwrap(),
            serde_json::json!("open")
        );
        assert_eq!(
            serde_json::to_value(AuthMode::ProtectWrites).unwrap(),
            serde_json::json!("protect-writes")
        );
        assert_eq!(
            serde_json::to_value(AuthMode::Closed).unwrap(),
            serde_json::json!("closed")
        );
    }

    #[test]
    fn auth_mode_from_str_accepts_underscore_alias() {
        assert_eq!("open".parse::<AuthMode>().unwrap(), AuthMode::Open);
        assert_eq!(
            "protect-writes".parse::<AuthMode>().unwrap(),
            AuthMode::ProtectWrites
        );
        assert_eq!(
            "protect_writes".parse::<AuthMode>().unwrap(),
            AuthMode::ProtectWrites
        );
        assert_eq!("closed".parse::<AuthMode>().unwrap(), AuthMode::Closed);
    }

    #[test]
    fn auth_mode_env_hard_errors_on_unrecognized_value() {
        let err = parse_auth_mode_env(Some("bogus".to_string())).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bogus"), "message was: {msg}");
        assert!(msg.contains("open"), "message was: {msg}");
        assert!(msg.contains("protect-writes"), "message was: {msg}");
        assert!(msg.contains("closed"), "message was: {msg}");
    }

    #[test]
    fn auth_mode_env_none_when_unset() {
        assert_eq!(parse_auth_mode_env(None).unwrap(), None);
    }

    #[test]
    fn config_default_is_closed() {
        let config = ServerConfig::default();
        assert_eq!(config.auth_mode, AuthMode::Closed);
        assert_eq!(config.port, 8321);
    }
}
