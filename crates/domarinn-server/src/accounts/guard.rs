//! Throttling for the password endpoints.
//!
//! Two layers, checked in this order:
//!
//! 1. **Per client, in memory.** A token bucket per client address (IPv6 by
//!    /64), checked before any storage or argon2 work, whatever credentials
//!    arrive. It bounds what one address can cost this replica's CPU, so a
//!    username spray from one address costs the attacker time instead. It
//!    stays per replica on purpose: each replica's CPU is what it protects.
//!    No `ClientIp` (no connect info, as under the `oneshot` harness) means
//!    no client to key on, and the layer is skipped.
//! 2. **Per account or per device, in the database.** Shared by every
//!    replica, so N replicas never mean N budgets. A browser holding a valid
//!    device cookie for the account is charged to its own `device:` counter;
//!    everyone else shares the account's `account:` counter. However hard an
//!    attacker drives the account counter, the owner's known browser is on a
//!    counter the attacker cannot touch.
//!
//! The self-service password change has its own per-account counter and no
//! per-client limit: its caller is authenticated, and only they can trip it.

use std::time::{Duration, Instant};

use anyhow::Context;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use sha2::{Digest, Sha256};

use super::device::DeviceKey;
use crate::clientip::ClientIp;
use crate::domain::UserId;
use crate::ratelimit::TokenBuckets;
use crate::routes::ApiError;
use crate::storage::{AttemptPolicy, AttemptVerdict, Storage};

const MINUTE_MS: i64 = 60 * 1000;

/// Sign-in from a browser without a device cookie for the account: 10
/// failures per window, then a lock of 5 minutes doubling to an hour. While
/// locked the password is not even checked; the owner's way in is a known
/// browser, SSO, or an admin reset.
pub(crate) const UNTRUSTED: AttemptPolicy = AttemptPolicy {
    max_failures: 10,
    lock_base_ms: 5 * MINUTE_MS,
    lock_cap_ms: 60 * MINUTE_MS,
};
/// Sign-in from a recognised browser. Only that browser's own failures count,
/// so a flat 15-minute lock is enough: nobody else can trip it.
pub(crate) const DEVICE: AttemptPolicy = AttemptPolicy {
    max_failures: 10,
    lock_base_ms: 15 * MINUTE_MS,
    lock_cap_ms: 15 * MINUTE_MS,
};
/// Guesses at `current_password` on `POST /me/password`.
pub(crate) const PASSWORD_CHANGE: AttemptPolicy = AttemptPolicy {
    max_failures: 5,
    lock_base_ms: 15 * MINUTE_MS,
    lock_cap_ms: 15 * MINUTE_MS,
};

/// Per-client budget: a burst of 10, refilling at 10 a minute. No person
/// types ten passwords a minute.
const CLIENT_PER_MINUTE: u32 = 10;
const CLIENT_BURST: u32 = 10;
/// Cap on tracked client buckets; a full bucket is the default state, so
/// evicting idle ones loses nothing.
const CLIENT_MAX_TRACKED: usize = 100_000;

/// The `server_secrets` row device cookies are signed with.
const DEVICE_COOKIE_SECRET: &str = "device_cookie_key";

/// The password endpoints' throttle state, one per process.
pub(crate) struct LoginGuard {
    client: TokenBuckets,
    /// Signs and verifies device cookies. Loaded once at startup from the
    /// database, so every replica shares it.
    pub(crate) device_key: DeviceKey,
    /// Password verifies `login` has run, so tests can prove a refusal came
    /// before argon2.
    #[cfg(test)]
    pub(crate) verifies: std::sync::atomic::AtomicU64,
}

impl LoginGuard {
    /// Build the guard, minting the device-cookie key on first start.
    pub(crate) async fn load(storage: &Storage) -> anyhow::Result<Self> {
        let secret = storage
            .server_secret(DEVICE_COOKIE_SECRET.to_string())
            .await
            .context("loading the device cookie key")?;
        Ok(LoginGuard {
            client: TokenBuckets::new(CLIENT_PER_MINUTE, CLIENT_BURST, CLIENT_MAX_TRACKED),
            device_key: DeviceKey::from_hex(&secret)?,
            #[cfg(test)]
            verifies: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Spend one of `client`'s tokens, or the 429 to send instead. With no
    /// known client there is nothing to key on, and nothing is refused.
    // The error *is* the response the handler returns, once per request, so
    // boxing it would only add an allocation to the refusal path.
    #[allow(clippy::result_large_err)]
    pub(crate) fn check_client(&self, client: Option<&ClientIp>) -> Result<(), Response> {
        let Some(ip) = client else { return Ok(()) };
        self.client
            .take(&ip.bucket(), Instant::now())
            .map_err(|wait| throttled(Refusal::Client, wait, client))
    }
}

/// The untrusted counter for a login name. Hashed, so the key is a fixed size
/// however long the submitted name; trimmed, because the lookup trims too and
/// `" root "` must not buy a fresh budget for `root`. A name with no account
/// gets a counter exactly like one that has, so lockouts reveal nothing.
pub(crate) fn account_key(username: &str) -> String {
    let digest = Sha256::digest(username.trim().as_bytes());
    format!("account:{}", hex::encode(digest))
}

/// The trusted counter for one browser, by its device cookie's nonce.
pub(crate) fn device_key(nonce: &str) -> String {
    format!("device:{nonce}")
}

/// The counter for guesses at an account's current password.
pub(crate) fn password_change_key(id: &UserId) -> String {
    format!("pwchange:{id}")
}

/// Reserve an attempt against `key`, or the wait until it unlocks. A storage
/// error is an error, never a pass: failing open here would hand an attacker
/// unlimited guesses whenever the database is struggling.
pub(crate) async fn begin_attempt(
    storage: &Storage,
    key: String,
    policy: AttemptPolicy,
) -> anyhow::Result<Option<Duration>> {
    let now_ms = Utc::now().timestamp_millis();
    Ok(
        match storage.begin_login_attempt(key, now_ms, policy).await? {
            AttemptVerdict::Allowed => None,
            AttemptVerdict::Locked { retry_after_ms } => Some(Duration::from_millis(
                u64::try_from(retry_after_ms).unwrap_or(0),
            )),
        },
    )
}

/// Which limit refused a request; it decides the sentence the caller sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    Client,
    Untrusted,
    Device,
    PasswordChange,
}

impl Refusal {
    fn kind(self) -> &'static str {
        match self {
            Refusal::Client => "client",
            Refusal::Untrusted => "untrusted",
            Refusal::Device => "device",
            Refusal::PasswordChange => "password_change",
        }
    }

    /// What happened and when to retry. The untrusted sentence also points at
    /// the way out, and is the same whether or not the account exists.
    fn message(self, wait: &str) -> String {
        match self {
            Refusal::Client => {
                format!("too many sign-in attempts from your network address; try again in {wait}")
            }
            Refusal::Untrusted => format!(
                "too many failed sign-in attempts for this account from unrecognised browsers; \
                 try again in {wait}, or sign in from a browser you've used before"
            ),
            Refusal::Device => {
                format!("too many failed sign-in attempts from this browser; try again in {wait}")
            }
            Refusal::PasswordChange => {
                format!("too many failed password attempts; try again in {wait}")
            }
        }
    }
}

/// A 429 carrying `Retry-After` in whole seconds (rounded up and never zero,
/// so a client that honours it always gets in), and a body sentence stating
/// the wait for clients (the web UI) that only show the body.
///
/// Logged with the limit and the client bucket only: never the username,
/// which would put whatever an attacker types (or a user's password typed
/// into the wrong box) into the logs.
pub(crate) fn throttled(
    refusal: Refusal,
    retry_after: Duration,
    client: Option<&ClientIp>,
) -> Response {
    let secs = retry_after.as_secs_f64().ceil().max(1.0) as u64;
    let wait = Duration::from_secs(secs);
    tracing::warn!(
        refusal = refusal.kind(),
        client = client.map(ClientIp::bucket).as_deref().unwrap_or("-"),
        retry_after_secs = secs,
        "password attempt refused by throttle"
    );
    let mut response = ApiError::status(
        StatusCode::TOO_MANY_REQUESTS,
        refusal.message(&human_wait(wait)),
    )
    .into_response();
    response
        .headers_mut()
        .insert(axum::http::header::RETRY_AFTER, HeaderValue::from(secs));
    response
}

/// "14 minutes", "1 minute", "45 seconds" — rounded up, so the wait a user is
/// told is never shorter than the real one.
fn human_wait(wait: Duration) -> String {
    let secs = wait.as_secs().max(1);
    if secs < 60 {
        let unit = if secs == 1 { "second" } else { "seconds" };
        return format!("{secs} {unit}");
    }
    let mins = secs.div_ceil(60);
    let unit = if mins == 1 { "minute" } else { "minutes" };
    format!("{mins} {unit}")
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::atomic::Ordering;

    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use axum::Router;
    use serde_json::json;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::*;
    use crate::{build_app, AppState, AuthMode, ServerConfig, Settings};

    const ADMIN_PW: &str = "rootpassword";

    async fn app_with_admin() -> (Router, AppState, TempDir) {
        let dir = TempDir::new().unwrap();
        let config = ServerConfig {
            port: 0,
            data_dir: dir.path().to_path_buf(),
            auth_mode: AuthMode::ProtectWrites,
        };
        let (router, state) = build_app(&config, Settings::default()).await.unwrap();
        let (status, _) = post(
            &router,
            None,
            "/api/v1/auth/setup",
            json!({ "username": "root", "password": ADMIN_PW }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        (router, state, dir)
    }

    /// POST as if over a real socket from `peer` (when given): the `oneshot`
    /// harness has no connect info, so it goes in as a request extension,
    /// where `into_make_service_with_connect_info` would put it.
    async fn post(
        router: &Router,
        peer: Option<&str>,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        if let Some(peer) = peer {
            let addr: SocketAddr = format!("{peer}:40000").parse().unwrap();
            req.extensions_mut().insert(ConnectInfo(addr));
        }
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn login_from(
        router: &Router,
        peer: &str,
        username: &str,
        password: &str,
    ) -> (StatusCode, String) {
        post(
            router,
            Some(peer),
            "/api/v1/auth/login",
            json!({ "username": username, "password": password }),
        )
        .await
    }

    #[tokio::test]
    async fn the_client_limit_refuses_before_any_argon2_runs() {
        let (router, state, _dir) = app_with_admin().await;
        for i in 0..10 {
            let (status, _) = login_from(&router, "198.51.100.20", "root", "wrong-pw").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
        }
        // Even the right password: the client is out of tokens.
        let (status, body) = login_from(&router, "198.51.100.20", "root", ADMIN_PW).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body.contains("network address"), "{body}");
        assert_eq!(state.login_guard.verifies.load(Ordering::SeqCst), 10);
    }

    #[tokio::test]
    async fn the_client_limit_is_per_address() {
        let (router, _state, _dir) = app_with_admin().await;
        // Distinct names, so no account counter is anywhere near its limit
        // and only the client limit can refuse.
        for i in 0..10 {
            let (status, _) =
                login_from(&router, "198.51.100.20", &format!("ghost{i}"), "wrong-pw").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}");
        }
        let (status, _) = login_from(&router, "198.51.100.20", "ghost10", "wrong-pw").await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

        let (status, _) = login_from(&router, "198.51.100.21", "root", "wrong-pw").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn setup_spends_the_client_budget_too() {
        let dir = TempDir::new().unwrap();
        let config = ServerConfig {
            port: 0,
            data_dir: dir.path().to_path_buf(),
            auth_mode: AuthMode::ProtectWrites,
        };
        let (router, _state) = build_app(&config, Settings::default()).await.unwrap();
        // Too short to hash, so each attempt is a cheap 400.
        let short = json!({ "username": "root", "password": "x" });
        for _ in 0..10 {
            let (status, _) = post(
                &router,
                Some("198.51.100.30"),
                "/api/v1/auth/setup",
                short.clone(),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (status, body) =
            post(&router, Some("198.51.100.30"), "/api/v1/auth/setup", short).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(body.contains("network address"), "{body}");
    }

    #[test]
    fn retry_after_rounds_up_and_is_never_zero() {
        for (wait, expected) in [(0, "1"), (1, "1"), (1_001, "2"), (300_000, "300")] {
            let response = throttled(Refusal::Untrusted, Duration::from_millis(wait), None);
            assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(
                response.headers()[axum::http::header::RETRY_AFTER],
                expected
            );
        }
    }

    #[test]
    fn the_account_key_is_fixed_size_and_ignores_padding() {
        assert_eq!(account_key(" root "), account_key("root"));
        assert_ne!(account_key("root"), account_key("rooT"));
        assert_eq!(
            account_key(&"a".repeat(100_000)).len(),
            account_key("a").len()
        );
    }

    #[test]
    fn the_wait_is_stated_in_rounded_up_human_units() {
        assert_eq!(human_wait(Duration::from_secs(1)), "1 second");
        assert_eq!(human_wait(Duration::from_secs(45)), "45 seconds");
        assert_eq!(human_wait(Duration::from_secs(60)), "1 minute");
        assert_eq!(human_wait(Duration::from_secs(61)), "2 minutes");
        assert_eq!(human_wait(Duration::from_secs(15 * 60)), "15 minutes");
    }
}
