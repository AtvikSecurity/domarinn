//! domarinn-server — the self-hostable results server + embedded web UI.
//!
//! The server ingests [`RunResult`](domarinn_core::result::RunResult)
//! documents, stores them in a hybrid SQLite layout (see [`storage`]), and
//! exposes a read/compare/export API plus a shared content-addressed cache.
//!
//! ## Auth
//! The active [`AuthMode`] is derived at startup from configuration and the
//! environment:
//! * no tokens configured → [`AuthMode::Open`] (everything open),
//! * tokens configured → [`AuthMode::ProtectWrites`] by default (reads/UI open,
//!   writes + admin require a token),
//! * `DOMARINN_AUTH_MODE=closed` → [`AuthMode::Closed`] (every `/api` call
//!   requires a token).
//!
//! Tokens come from `DOMARINN_TOKENS="write:domarinn_ci,admin:domarinn_ops,read:domarinn_view"`.
//! Extra settings (`DOMARINN_PUBLIC_URL`, cache limits) are read from the
//! environment inside [`serve`], never as new [`ServerConfig`] fields.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use ts_rs::TS;

use crate::auth::{
    ApiKeyAuthenticator, Authenticator, SessionAuthenticator, StaticTokenAuthenticator,
};
use crate::storage::Storage;

pub mod accounts;
pub mod auth;
pub mod domain;
pub mod dto;
pub mod extract;
pub mod routes;
pub mod storage;

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
    /// Requested auth mode. When left at [`AuthMode::Open`], the effective mode
    /// is derived from whether tokens are configured.
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
    /// Bootstrap admin username (`DOMARINN_ADMIN_USER`).
    pub admin_user: Option<String>,
    /// Bootstrap admin password (`DOMARINN_ADMIN_PASSWORD`).
    pub admin_password: Option<String>,
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
            admin_user: env("DOMARINN_ADMIN_USER"),
            admin_password: env("DOMARINN_ADMIN_PASSWORD"),
        })
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
}

impl AppState {
    /// Open storage, bootstrap any configured admin, and derive the effective
    /// auth mode.
    pub async fn new(config: &ServerConfig, settings: Settings) -> anyhow::Result<AppState> {
        let storage = Storage::open(config.data_dir.clone()).await?;
        let authenticator =
            StaticTokenAuthenticator::from_env_value(settings.tokens.as_deref().unwrap_or(""));
        let has_tokens = authenticator.has_tokens();

        // Ensure a bootstrap admin exists before deriving the mode, so that a
        // freshly-seeded instance is protected rather than open.
        bootstrap_admin(&storage, &settings).await?;
        let has_accounts = storage.count_users().await? > 0;

        let auth_mode = resolve_mode(
            config.auth_mode,
            settings.auth_mode,
            has_tokens || has_accounts,
        );
        let cache_limits = CacheLimits {
            max_entry_bytes: settings
                .cache_max_entry_bytes
                .unwrap_or(DEFAULT_MAX_ENTRY_BYTES),
            max_bytes: settings.cache_max_bytes.unwrap_or(DEFAULT_MAX_BYTES),
            max_age_days: settings.cache_max_age_days.unwrap_or(DEFAULT_MAX_AGE_DAYS),
        };
        Ok(AppState {
            api_key_auth: Arc::new(ApiKeyAuthenticator::new(storage.clone())),
            session_auth: Arc::new(SessionAuthenticator::new(storage.clone())),
            storage,
            auth: Arc::new(authenticator),
            auth_mode,
            public_url: settings.public_url,
            cache_limits,
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

fn resolve_mode(
    config_mode: AuthMode,
    env_mode: Option<AuthMode>,
    has_credentials: bool,
) -> AuthMode {
    if let Some(mode) = env_mode {
        return mode;
    }
    if config_mode != AuthMode::Open {
        return config_mode;
    }
    if has_credentials {
        AuthMode::ProtectWrites
    } else {
        AuthMode::Open
    }
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

/// Hourly LRU/age retention against the configured cache limits.
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
    use crate::domain::RunStatusFilter;
    use crate::dto::accounts::{
        ApiKeyCreatedResponse, ApiKeyListResponse, AuthSessionResponse, MeResponse, OkResponse,
        UserListResponse,
    };
    use crate::dto::cache::{CacheStatsResponse, PruneResponse};
    use crate::dto::cases::CaseListResponse;
    use crate::dto::compare::CompareResponse;
    use crate::dto::config::RunConfigResponse;
    use crate::dto::history::CaseHistoryResponse;
    use crate::dto::matrix::MatrixResponse;
    use crate::dto::meta::MetaResponse;
    use crate::dto::projects::{ProjectsResponse, SuitesResponse};
    use crate::dto::runs::{IngestResponse, RunDetailResponse, RunListResponse};
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
    PruneResponse::export_all(&cfg)?;
    MetaResponse::export_all(&cfg)?;
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
    fn mode_derivation() {
        assert_eq!(resolve_mode(AuthMode::Open, None, false), AuthMode::Open);
        assert_eq!(
            resolve_mode(AuthMode::Open, None, true),
            AuthMode::ProtectWrites
        );
        assert_eq!(
            resolve_mode(AuthMode::Open, Some(AuthMode::Closed), true),
            AuthMode::Closed
        );
        assert_eq!(
            resolve_mode(AuthMode::Open, Some(AuthMode::Open), true),
            AuthMode::Open
        );
        // An explicit config mode wins over token-derivation.
        assert_eq!(
            resolve_mode(AuthMode::Closed, None, false),
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
    fn config_default_is_open() {
        let config = ServerConfig::default();
        assert_eq!(config.auth_mode, AuthMode::Open);
        assert_eq!(config.port, 8321);
    }
}
