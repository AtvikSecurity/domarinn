//! The client address over a real listening socket.
//!
//! Most other suites drive the router through `oneshot`, which carries no
//! `ConnectInfo`, so no `ClientIp` is resolved and the per-client login limit
//! never applies there. Here the app is served on `127.0.0.1:0` with
//! `into_make_service_with_connect_info`, exactly as `serve_on` does, and
//! driven over HTTP: the peer address is whatever the kernel reports, and
//! `X-Forwarded-For` is honoured only when that peer is a trusted proxy.
//!
//! Runs on both backends through `common` (`mise run test-pg`).

mod common;

use std::net::SocketAddr;
use std::time::Instant;

use reqwest::StatusCode;
use serde_json::{json, Value};
use tempfile::TempDir;

use domarinn_server::{AuthMode, Settings};

const ADMIN_PW: &str = "rootpassword";
const WRONG_PW: &str = "wrong-password";

/// Requests the per-client bucket allows before it refuses (its burst).
const BURST: usize = 10;

/// The per-client refusal's distinguishing phrase. The per-account
/// ("unrecognised browsers") and per-device refusals do not contain it.
const CLIENT_PHRASE: &str = "network address";

/// A server listening on loopback, its base URL, and the guards that keep its
/// database alive.
struct Served {
    base: String,
    _dir: TempDir,
    _server: tokio::task::JoinHandle<()>,
}

/// Build the app through `common` (so the backend follows
/// `DOMARINN_TEST_BACKEND`) and serve it on an ephemeral loopback port.
async fn serve(trusted_proxies: Option<&str>) -> Served {
    let settings = Settings {
        trusted_proxies: trusted_proxies.map(str::to_string),
        auth_mode: Some(AuthMode::Closed),
        ..Settings::default()
    };
    let (router, _storage, dir) = common::test_app_with_storage(settings, AuthMode::Closed).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("serve");
    });
    Served {
        base: format!("http://{addr}"),
        _dir: dir,
        _server: server,
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("reqwest client")
}

/// POST JSON to `path`, with `X-Forwarded-For: xff` when given. Returns the
/// status and the body's `error` string (empty when there is none).
async fn post(
    http: &reqwest::Client,
    served: &Served,
    path: &str,
    xff: Option<&str>,
    body: Value,
) -> (StatusCode, String) {
    let mut req = http.post(format!("{}{path}", served.base)).json(&body);
    if let Some(xff) = xff {
        req = req.header("x-forwarded-for", xff);
    }
    let resp = req.send().await.expect("send request");
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let error = body["error"].as_str().unwrap_or_default().to_string();
    (status, error)
}

async fn login(
    http: &reqwest::Client,
    served: &Served,
    xff: Option<&str>,
    username: &str,
) -> (StatusCode, String) {
    post(
        http,
        served,
        "/api/v1/auth/login",
        xff,
        json!({ "username": username, "password": WRONG_PW }),
    )
    .await
}

/// Spend per-client tokens with one `POST /auth/setup` per forwarded
/// address, all expected to answer `expect`. Setup shares the login bucket
/// and is refused before any hashing (a 409 once an admin exists, a 400 for a
/// too-short password before then), so the drain never waits on argon2: a
/// drain of logins queued ten verifies on the process-wide hash permits, and
/// on a slow runner that outlasted the 6 s refill and handed a token back
/// before the refusal under test.
async fn drain(http: &reqwest::Client, served: &Served, xffs: Vec<String>, expect: StatusCode) {
    for (i, xff) in xffs.into_iter().enumerate() {
        let (status, error) = post(
            http,
            served,
            "/api/v1/auth/setup",
            Some(&xff),
            json!({ "username": "root", "password": "x" }),
        )
        .await;
        assert_eq!(status, expect, "drain {i} from {xff}: {error}");
    }
}

async fn setup_admin(http: &reqwest::Client, served: &Served, xff: Option<&str>) {
    let (status, error) = post(
        http,
        served,
        "/api/v1/auth/setup",
        xff,
        json!({ "username": "root", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "setup: {error}");
}

#[tokio::test]
async fn xff_from_a_trusted_proxy_keys_the_client_limit_on_the_real_client() {
    let served = serve(Some("127.0.0.1")).await;
    let http = client();
    // Setup spends a token from its caller's bucket: give it its own client.
    setup_admin(&http, &served, Some("198.51.100.29")).await;

    // Ten setups forwarded for one client: an admin exists, so each is a
    // cheap 409, and only the per-client bucket notices them.
    let started = Instant::now();
    drain(
        &http,
        &served,
        vec!["198.51.100.30".to_string(); BURST],
        StatusCode::CONFLICT,
    )
    .await;
    let (status, error) = login(&http, &served, Some("198.51.100.30"), "ghost-a-last").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "{error} (after {:?}; a token refills every 6 s)",
        started.elapsed()
    );
    assert!(error.contains(CLIENT_PHRASE), "{error}");

    // Same peer (loopback), same kind of request, different forwarded client:
    // a fresh bucket, so the address really came from the header.
    let (status, error) = login(&http, &served, Some("198.51.100.31"), "ghost-b0").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{error}");
}

#[tokio::test]
async fn xff_from_an_untrusted_peer_is_ignored() {
    let served = serve(None).await;
    let http = client();
    // No admin, so each too-short setup is a 400 before any hashing.

    // Every request spoofs a fresh address. If the header were honoured each
    // would get its own bucket and the final login would be a 401; it is not,
    // so they all share the loopback peer's bucket.
    let started = Instant::now();
    let xffs = (0..BURST)
        .map(|i| format!("198.51.100.{}", 40 + i))
        .collect();
    drain(&http, &served, xffs, StatusCode::BAD_REQUEST).await;
    let (status, error) = login(&http, &served, Some("198.51.100.99"), "ghost-c-last").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "{error} (after {:?}; a token refills every 6 s)",
        started.elapsed()
    );
    assert!(error.contains(CLIENT_PHRASE), "{error}");
}
