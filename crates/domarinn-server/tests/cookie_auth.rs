//! Cookie-session transport and the CSRF origin check.
//!
//! Browser sessions ride an `HttpOnly` `domarinn_session` cookie; the
//! `Authorization` header always wins when both are present, and cookies may
//! only ever carry sessions (never static tokens or API keys). Cookie-authed
//! mutations additionally pass a same-origin check.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_server::{AuthMode, Settings};
use serde_json::json;

const ADMIN_CREDS: &str = r#"{"username":"root","password":"hunter2hunter2"}"#;

/// Closed-mode app with one admin claimed via setup; returns the app and the
/// admin's session token.
async fn app_with_admin() -> (axum::Router, tempfile::TempDir, String) {
    let (app, dir) = test_app_with_mode(Settings::default(), AuthMode::Closed).await;
    let created = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &serde_json::from_str(ADMIN_CREDS).unwrap(),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let token = created.json()["token"].as_str().unwrap().to_string();
    (app, dir, token)
}

fn cookie_header(token: &str) -> String {
    format!("domarinn_session={token}")
}

#[tokio::test]
async fn login_and_setup_set_a_hardened_session_cookie() {
    let (app, _dir, _token) = app_with_admin().await;

    let login = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &serde_json::from_str(ADMIN_CREDS).unwrap(),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);

    let cookies = login.set_cookies();
    assert_eq!(cookies.len(), 1, "exactly one Set-Cookie: {cookies:?}");
    let cookie = &cookies[0];
    assert!(cookie.starts_with("domarinn_session=mses_"), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
    assert!(cookie.contains("Path=/"), "{cookie}");
    assert!(cookie.contains("Max-Age=2592000"), "{cookie}");
    // No DOMARINN_PUBLIC_URL configured -> not Secure by default.
    assert!(!cookie.contains("Secure"), "{cookie}");

    // The cookie's token matches the JSON body's token.
    let json_token = login.json()["token"].as_str().unwrap().to_string();
    assert!(cookie.contains(&json_token));
}

#[tokio::test]
async fn cookie_authenticates_reads_in_closed_mode() {
    let (app, _dir, token) = app_with_admin().await;

    // No credential -> 401.
    assert_eq!(
        get(&app, "/api/v1/runs").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Session cookie alone -> authenticated.
    let read = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[("cookie", &cookie_header(&token))],
        Vec::new(),
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);

    // `me` reflects the cookie identity.
    let me = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie_header(&token))],
        Vec::new(),
    )
    .await;
    assert_eq!(me.json()["authenticated"], true);
    assert_eq!(me.json()["source"], "session");
    assert_eq!(me.json()["user"]["username"], "root");
}

#[tokio::test]
async fn authorization_header_beats_cookie() {
    let (app, _dir, token) = app_with_admin().await;

    // A garbage bearer with a valid cookie: the explicit header wins, so the
    // request is anonymous -> 401. The cookie must not rescue it.
    let read = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[
            ("authorization", "Bearer not-a-real-token"),
            ("cookie", &cookie_header(&token)),
        ],
        Vec::new(),
    )
    .await;
    assert_eq!(read.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cookies_never_carry_static_tokens_or_api_keys() {
    let settings = Settings {
        tokens: Some("admin:domarinn_ops".to_string()),
        ..Default::default()
    };
    let (app, _dir) = test_app_with_mode(settings, AuthMode::Closed).await;

    // The static admin token works as a bearer...
    assert_eq!(
        get_auth(&app, "/api/v1/runs", Some("domarinn_ops"))
            .await
            .status,
        StatusCode::OK
    );

    // ...but smuggled through the session cookie it stays anonymous.
    let via_cookie = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[("cookie", &cookie_header("domarinn_ops"))],
        Vec::new(),
    )
    .await;
    assert_eq!(via_cookie.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cookie_mutations_enforce_same_origin() {
    let (app, _dir, token) = app_with_admin().await;
    let cookie = cookie_header(&token);
    let run = serde_json::to_vec(&run_value(&simple_run("csrf1"))).unwrap();

    // Cross-origin cookie POST -> 403 (never reaches the handler).
    let evil = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[
            ("cookie", &cookie),
            ("origin", "https://evil.example"),
            ("host", "domarinn.internal"),
        ],
        run.clone(),
    )
    .await;
    assert_eq!(evil.status, StatusCode::FORBIDDEN);
    assert_eq!(evil.json()["error"], "cross-origin request rejected");

    // Same-origin (Origin matches Host) -> accepted.
    let good = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[
            ("cookie", &cookie),
            ("origin", "http://domarinn.internal"),
            ("host", "domarinn.internal"),
        ],
        run.clone(),
    )
    .await;
    assert_eq!(good.status, StatusCode::CREATED);

    // No Origin/Referer at all (non-browser client using the cookie) -> allowed.
    let headerless = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[("cookie", &cookie)],
        serde_json::to_vec(&run_value(&simple_run("csrf2"))).unwrap(),
    )
    .await;
    assert_eq!(headerless.status, StatusCode::CREATED);

    // Cookie GETs are never origin-checked.
    let read = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[("cookie", &cookie), ("origin", "https://evil.example")],
        Vec::new(),
    )
    .await;
    assert_eq!(read.status, StatusCode::OK);

    // An `Origin: null` (sandboxed iframe) is present-but-unparseable and
    // fails closed rather than slipping through.
    let null_origin = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[("cookie", &cookie), ("origin", "null")],
        serde_json::to_vec(&run_value(&simple_run("csrfnull"))).unwrap(),
    )
    .await;
    assert_eq!(null_origin.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_url_origin_is_accepted() {
    let (app, dir) = {
        let settings = Settings {
            public_url: Some("https://results.example.com".to_string()),
            cookie_secure: Some(false),
            ..Default::default()
        };
        test_app_with_mode(settings, AuthMode::Closed).await
    };
    let _dir = dir;
    let created = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &serde_json::from_str(ADMIN_CREDS).unwrap(),
    )
    .await;
    let token = created.json()["token"].as_str().unwrap().to_string();

    // Origin matches DOMARINN_PUBLIC_URL even when Host differs (proxy hop).
    let good = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[
            ("cookie", &cookie_header(&token)),
            ("origin", "https://results.example.com"),
            ("host", "10.0.0.7:8321"),
        ],
        serde_json::to_vec(&run_value(&simple_run("csrf3"))).unwrap(),
    )
    .await;
    assert_eq!(good.status, StatusCode::CREATED);
}

#[tokio::test]
async fn bearer_mutations_ignore_origin() {
    let (app, _dir, token) = app_with_admin().await;

    // Same request as the evil-origin case, but the credential is a header —
    // structurally CSRF-immune, so no origin check applies.
    let write = send_with_headers(
        &app,
        "POST",
        "/api/v1/runs",
        &[
            ("authorization", &format!("Bearer {token}")),
            ("origin", "https://evil.example"),
            ("host", "domarinn.internal"),
        ],
        serde_json::to_vec(&run_value(&simple_run("csrf4"))).unwrap(),
    )
    .await;
    assert_eq!(write.status, StatusCode::CREATED);
}

#[tokio::test]
async fn secure_flag_follows_public_url_scheme() {
    let settings = Settings {
        public_url: Some("https://results.example.com".to_string()),
        ..Default::default()
    };
    let (app, _dir) = test_app_with_mode(settings, AuthMode::Closed).await;
    let created = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &serde_json::from_str(ADMIN_CREDS).unwrap(),
    )
    .await;
    let cookies = created.set_cookies();
    assert!(cookies[0].contains("Secure"), "{cookies:?}");
}

#[tokio::test]
async fn logout_revokes_the_cookie_session_and_clears_the_cookie() {
    let (app, _dir, token) = app_with_admin().await;
    let cookie = cookie_header(&token);

    // Logout presenting only the cookie (no Origin — curl-style; and with an
    // Origin in the browser case, covered by the same-origin test above).
    let out = send_with_headers(
        &app,
        "POST",
        "/api/v1/auth/logout",
        &[("cookie", &cookie)],
        Vec::new(),
    )
    .await;
    assert_eq!(out.status, StatusCode::OK);
    let cleared = out.set_cookies();
    assert_eq!(cleared.len(), 1);
    assert!(cleared[0].starts_with("domarinn_session=;"), "{cleared:?}");
    assert!(cleared[0].contains("Max-Age=0"), "{cleared:?}");

    // The session row is gone: the cookie no longer authenticates.
    let read = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[("cookie", &cookie)],
        Vec::new(),
    )
    .await;
    assert_eq!(read.status, StatusCode::UNAUTHORIZED);

    // Anonymous logout is still a 401.
    let anon = post_json(&app, "/api/v1/auth/logout", None, &json!({})).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}
