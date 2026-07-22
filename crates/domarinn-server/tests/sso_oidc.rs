//! OIDC login flow, end to end against an in-process mock IdP: start
//! redirect, code+state callback, id_token verification (RS256, nonce),
//! JIT provisioning, role mapping/re-sync, and the failure paths.

mod common;

use axum::http::StatusCode;
use axum::Router;
use common::mock_oidc::{MockIdp, TokenSpec};
use common::*;
use domarinn_server::sso::parse_sso_settings;
use domarinn_server::{AuthMode, Settings};
use serde_json::json;

const PUBLIC_URL: &str = "http://app.test";

/// Closed-mode app with one OIDC provider ("test") pointed at the mock IdP.
async fn sso_app(mock: &MockIdp, extra: &[(&str, &str)]) -> (Router, tempfile::TempDir) {
    let mut vars: Vec<(String, String)> = vec![
        ("DOMARINN_OIDC_PROVIDERS".into(), "test".into()),
        ("DOMARINN_OIDC_TEST_ISSUER".into(), mock.issuer.clone()),
        ("DOMARINN_OIDC_TEST_CLIENT_ID".into(), "cid".into()),
        ("DOMARINN_OIDC_TEST_CLIENT_SECRET".into(), "sec".into()),
    ];
    vars.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let sso = parse_sso_settings(&vars.into_iter().collect()).expect("sso settings");
    let settings = Settings {
        public_url: Some(PUBLIC_URL.to_string()),
        sso,
        ..Default::default()
    };
    test_app_with_mode(settings, AuthMode::Closed).await
}

fn location(reply: &Reply) -> String {
    reply
        .headers
        .get("location")
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string()
}

fn query_param(url: &str, name: &str) -> Option<String> {
    let absolute = if url.starts_with('/') {
        format!("http://relative.test{url}")
    } else {
        url.to_string()
    };
    url::Url::parse(&absolute)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.to_string())
}

/// The value of a named cookie in the reply's Set-Cookie headers.
fn set_cookie_value(reply: &Reply, name: &str) -> Option<String> {
    reply.set_cookies().iter().find_map(|c| {
        c.strip_prefix(&format!("{name}="))
            .map(|rest| rest.split(';').next().unwrap_or("").to_string())
    })
}

struct StartedLogin {
    state: String,
    nonce: String,
}

/// Drive the start endpoint and pull the flow parameters out of the IdP
/// authorize URL.
async fn start_login(app: &Router, return_to: Option<&str>) -> StartedLogin {
    let uri = match return_to {
        Some(r) => format!(
            "/api/v1/auth/oidc/test/start?return_to={}",
            r.replace('?', "%3F").replace('&', "%26")
        ),
        None => "/api/v1/auth/oidc/test/start".to_string(),
    };
    let reply = get(app, &uri).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);
    let auth_url = location(&reply);

    let state = query_param(&auth_url, "state").expect("state param");
    let nonce = query_param(&auth_url, "nonce").expect("nonce param");
    assert!(query_param(&auth_url, "code_challenge").is_some(), "PKCE");
    assert_eq!(
        query_param(&auth_url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    let scope = query_param(&auth_url, "scope").expect("scope param");
    assert!(
        scope.contains("openid") && scope.contains("email"),
        "{scope}"
    );

    // The txn cookie carries the state value (login-CSRF binding).
    assert_eq!(
        set_cookie_value(&reply, "domarinn_txn").as_deref(),
        Some(state.as_str())
    );
    StartedLogin { state, nonce }
}

async fn callback(app: &Router, code: &str, state: &str, cookie_state: Option<&str>) -> Reply {
    let uri = format!("/api/v1/auth/oidc/test/callback?code={code}&state={state}");
    let cookie;
    let mut headers: Vec<(&str, &str)> = Vec::new();
    if let Some(value) = cookie_state {
        cookie = format!("domarinn_txn={value}");
        headers.push(("cookie", &cookie));
    }
    send_with_headers(app, "GET", &uri, &headers, Vec::new()).await
}

fn admin_spec(login: &StartedLogin) -> TokenSpec {
    TokenSpec {
        sub: "sub-ops".to_string(),
        email: Some("ops@example.com".to_string()),
        email_verified: Some(true),
        name: Some("Ops Person".to_string()),
        groups: Some(vec!["admins".to_string()]),
        nonce: login.nonce.clone(),
        client_id: "cid".to_string(),
    }
}

#[tokio::test]
async fn full_round_trip_provisions_admin_and_sets_session_cookie() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(&mock, &[("DOMARINN_OIDC_TEST_ADMIN_GROUPS", "admins")]).await;

    let login = start_login(&app, Some("/runs/abc?tab=diff")).await;
    mock.expect_token("code1", admin_spec(&login));

    let reply = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER, "{:?}", reply.json());
    assert_eq!(location(&reply), "/runs/abc?tab=diff");

    let session = set_cookie_value(&reply, "domarinn_session").expect("session cookie");
    assert!(session.starts_with("mses_"));
    // The txn cookie is cleared on the same response.
    assert!(reply
        .set_cookies()
        .iter()
        .any(|c| c.starts_with("domarinn_txn=;")));

    // The session works, and the identity mapped to an admin.
    let cookie = format!("domarinn_session={session}");
    let me = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie)],
        vec![],
    )
    .await;
    assert_eq!(me.json()["authenticated"], true);
    assert_eq!(me.json()["user"]["username"], "ops");
    assert_eq!(me.json()["user"]["role"], "admin");
    assert_eq!(me.json()["source"], "session");

    // Second login with the same subject reuses the account.
    let login2 = start_login(&app, None).await;
    mock.expect_token("code2", admin_spec(&login2));
    let reply2 = callback(&app, "code2", &login2.state, Some(&login2.state)).await;
    assert_eq!(reply2.status, StatusCode::SEE_OTHER);
    assert_eq!(location(&reply2), "/");

    let users =
        send_with_headers(&app, "GET", "/api/v1/users", &[("cookie", &cookie)], vec![]).await;
    let list = users.json()["users"].as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "no duplicate user: {list:?}");
}

#[tokio::test]
async fn callback_requires_matching_txn_cookie_and_is_single_use() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(&mock, &[]).await;

    // No cookie at all.
    let login = start_login(&app, None).await;
    mock.expect_token("code1", admin_spec(&login));
    let no_cookie = callback(&app, "code1", &login.state, None).await;
    assert_eq!(no_cookie.status, StatusCode::SEE_OTHER);
    assert!(location(&no_cookie).starts_with("/login?sso_error=invalid_state"));

    // Mismatched cookie.
    let mismatched = callback(&app, "code1", &login.state, Some("someone-elses-state")).await;
    assert!(location(&mismatched).starts_with("/login?sso_error=invalid_state"));

    // A successful callback consumes the transaction...
    let ok = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert_eq!(location(&ok), "/");

    // ...so replaying the exact same callback fails.
    let replay = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert!(location(&replay).starts_with("/login?sso_error=invalid_state"));
}

#[tokio::test]
async fn wrong_nonce_is_rejected() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(&mock, &[]).await;

    let login = start_login(&app, None).await;
    let mut spec = admin_spec(&login);
    spec.nonce = "not-the-nonce".to_string();
    mock.expect_token("code1", spec);

    let reply = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert!(location(&reply).starts_with("/login?sso_error=provider_error"));
}

#[tokio::test]
async fn email_domain_restriction_blocks_provisioning() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(
        &mock,
        &[("DOMARINN_OIDC_TEST_ALLOWED_EMAIL_DOMAINS", "good.example")],
    )
    .await;

    // Claim the instance so we can inspect the user list afterwards.
    let created = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({"username": "root", "password": "hunter2hunter2"}),
    )
    .await;
    let admin_token = created.json()["token"].as_str().unwrap().to_string();

    // Wrong domain.
    let login = start_login(&app, None).await;
    let mut spec = admin_spec(&login);
    spec.email = Some("ops@evil.example".to_string());
    mock.expect_token("code1", spec);
    let reply = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert!(location(&reply).starts_with("/login?sso_error=email_not_allowed"));

    // An explicitly-unverified email must not satisfy the allowlist either.
    let login2 = start_login(&app, None).await;
    let mut spec2 = admin_spec(&login2);
    spec2.email = Some("ops@good.example".to_string());
    spec2.email_verified = Some(false);
    mock.expect_token("code2", spec2);
    let reply2 = callback(&app, "code2", &login2.state, Some(&login2.state)).await;
    assert!(location(&reply2).starts_with("/login?sso_error=email_not_allowed"));

    // An ABSENT email_verified claim is treated as unverified, not trusted.
    let login3 = start_login(&app, None).await;
    let mut spec3 = admin_spec(&login3);
    spec3.email = Some("ops@good.example".to_string());
    spec3.email_verified = None;
    mock.expect_token("code3", spec3);
    let reply3 = callback(&app, "code3", &login3.state, Some(&login3.state)).await;
    assert!(location(&reply3).starts_with("/login?sso_error=email_not_allowed"));

    // No SSO user was created.
    let users = get_auth(&app, "/api/v1/users", Some(&admin_token)).await;
    assert_eq!(users.json()["users"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unverified_email_does_not_grant_admin_by_email() {
    let mock = MockIdp::spawn().await;
    // Admin mapping is by email, no groups; the IdP omits email_verified.
    let (app, _dir) = sso_app(
        &mock,
        &[("DOMARINN_OIDC_TEST_ADMIN_EMAILS", "ops@example.com")],
    )
    .await;

    let login = start_login(&app, None).await;
    let mut spec = admin_spec(&login);
    spec.email = Some("ops@example.com".to_string());
    spec.email_verified = None; // unverified
    spec.groups = Some(vec![]);
    mock.expect_token("code1", spec);
    let reply = callback(&app, "code1", &login.state, Some(&login.state)).await;
    assert_eq!(reply.status, StatusCode::SEE_OTHER);

    // Provisioned (no allowlist), but NOT as admin — the unverified email
    // must not match the admin-email mapping.
    let session = set_cookie_value(&reply, "domarinn_session").unwrap();
    let cookie = format!("domarinn_session={session}");
    let me = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie)],
        vec![],
    )
    .await;
    assert_eq!(
        me.json()["user"]["role"],
        "member",
        "unverified email ≠ admin"
    );
}

#[tokio::test]
async fn role_resyncs_on_each_login_but_never_demotes_the_last_admin() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(&mock, &[("DOMARINN_OIDC_TEST_ADMIN_GROUPS", "admins")]).await;

    // Sole account: the SSO user becomes the only (last) admin.
    let login = start_login(&app, None).await;
    mock.expect_token("code1", admin_spec(&login));
    let first = callback(&app, "code1", &login.state, Some(&login.state)).await;
    let session = set_cookie_value(&first, "domarinn_session").unwrap();
    let cookie = format!("domarinn_session={session}");
    let me = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie)],
        vec![],
    )
    .await;
    assert_eq!(me.json()["user"]["role"], "admin");

    // Group revoked at the IdP, but this is the last enabled admin: kept.
    let login2 = start_login(&app, None).await;
    let mut spec = admin_spec(&login2);
    spec.groups = Some(vec![]);
    mock.expect_token("code2", spec);
    let second = callback(&app, "code2", &login2.state, Some(&login2.state)).await;
    let session2 = set_cookie_value(&second, "domarinn_session").unwrap();
    let cookie2 = format!("domarinn_session={session2}");
    let me2 = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie2)],
        vec![],
    )
    .await;
    assert_eq!(me2.json()["user"]["role"], "admin", "last admin kept");

    // With a second local admin present, the demotion goes through.
    let create = send_with_headers(
        &app,
        "POST",
        "/api/v1/users",
        &[("cookie", &cookie2)],
        serde_json::to_vec(&json!({
            "username": "root2", "password": "hunter2hunter2", "role": "admin"
        }))
        .unwrap(),
    )
    .await;
    assert_eq!(create.status, StatusCode::CREATED, "{:?}", create.json());

    let login3 = start_login(&app, None).await;
    let mut spec3 = admin_spec(&login3);
    spec3.groups = Some(vec![]);
    mock.expect_token("code3", spec3);
    let third = callback(&app, "code3", &login3.state, Some(&login3.state)).await;
    let session3 = set_cookie_value(&third, "domarinn_session").unwrap();
    let cookie3 = format!("domarinn_session={session3}");
    let me3 = send_with_headers(
        &app,
        "GET",
        "/api/v1/auth/me",
        &[("cookie", &cookie3)],
        vec![],
    )
    .await;
    assert_eq!(me3.json()["user"]["role"], "member", "demoted by re-sync");
}

#[tokio::test]
async fn sso_only_users_cannot_password_login_and_unknown_provider_404s() {
    let mock = MockIdp::spawn().await;
    let (app, _dir) = sso_app(&mock, &[]).await;

    let login = start_login(&app, None).await;
    mock.expect_token("code1", admin_spec(&login));
    callback(&app, "code1", &login.state, Some(&login.state)).await;

    // The JIT user ("ops") has no password; any password attempt is a
    // generic 401 (no account enumeration).
    let attempt = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({"username": "ops", "password": ""}),
    )
    .await;
    assert_eq!(attempt.status, StatusCode::UNAUTHORIZED);
    let attempt2 = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({"username": "ops", "password": "hunter2hunter2"}),
    )
    .await;
    assert_eq!(attempt2.status, StatusCode::UNAUTHORIZED);

    // Unknown providers 404 like any other missing resource.
    let unknown = get(&app, "/api/v1/auth/oidc/nope/start").await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
}
