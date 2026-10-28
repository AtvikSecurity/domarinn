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

/// Send one wrong login per `(xff, username)` pair, all at once, and assert
/// each is a 401. Concurrent, so every `check_client` lands within
/// milliseconds of the first: sequential argon2 verifies in a debug build on
/// a slow runner could otherwise outlast the 6 s refill and hand the bucket a
/// token back before the refusal under test.
async fn drain(http: &reqwest::Client, served: &Served, attempts: Vec<(String, String)>) {
    let mut set = tokio::task::JoinSet::new();
    for (i, (xff, username)) in attempts.into_iter().enumerate() {
        let http = http.clone();
        let base = served.base.clone();
        set.spawn(async move {
            let resp = http
                .post(format!("{base}/api/v1/auth/login"))
                .header("x-forwarded-for", xff)
                .json(&json!({ "username": username, "password": WRONG_PW }))
                .send()
                .await
                .expect("send request");
            (i, resp.status(), resp.text().await.unwrap_or_default())
        });
    }
    while let Some(joined) = set.join_next().await {
        let (i, status, body) = joined.expect("login task");
        assert_eq!(status, StatusCode::UNAUTHORIZED, "attempt {i}: {body}");
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

    // A distinct unknown username per attempt, so every account counter sees
    // one failure and only the per-client bucket can run out.
    let started = Instant::now();
    let attempts = (0..BURST)
        .map(|i| ("198.51.100.30".to_string(), format!("ghost-a{i}")))
        .collect();
    drain(&http, &served, attempts).await;
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
    // No setup: it would spend one of the loopback bucket's tokens, and login
    // answers an unknown username with the same 401 whether or not any user
    // exists, so all eleven requests below are logins.

    // Every request spoofs a fresh address and a fresh username. If the header
    // were honoured each would get its own bucket and all would be 401; it is
    // not, so they all share the loopback peer's bucket.
    let started = Instant::now();
    let attempts = (0..BURST)
        .map(|i| (format!("198.51.100.{}", 40 + i), format!("ghost-c{i}")))
        .collect();
    drain(&http, &served, attempts).await;
    let (status, error) = login(&http, &served, Some("198.51.100.99"), "ghost-c-last").await;
    assert_eq!(
        status,
        StatusCode::TOO_MANY_REQUESTS,
        "{error} (after {:?}; a token refills every 6 s)",
        started.elapsed()
    );
    assert!(error.contains(CLIENT_PHRASE), "{error}");
}
