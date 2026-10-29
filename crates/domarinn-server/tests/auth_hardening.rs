//! Account-security behavior around passwords and sessions: a username
//! spray sparing everyone else, session revocation on password reset / disable, the
//! self-service password change, and the expired-session sweep.

mod common;

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tower::ServiceExt;

use domarinn_server::auth;
use domarinn_server::domain::Role;
use domarinn_server::{AuthMode, Settings};

const ADMIN_PW: &str = "rootpassword";
const MEMBER_PW: &str = "memberpass1";

fn protected() -> Settings {
    Settings {
        auth_mode: Some(AuthMode::ProtectWrites),
        ..Default::default()
    }
}

async fn login(app: &Router, username: &str, password: &str) -> Reply {
    post_json(
        app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": username, "password": password }),
    )
    .await
}

fn token_of(reply: &Reply) -> String {
    reply.json()["token"].as_str().unwrap().to_string()
}

async fn setup_admin(app: &Router) -> String {
    let r = post_json(
        app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
    token_of(&r)
}

/// Create `username` as a member; returns its user id.
async fn create_member(app: &Router, admin: &str, username: &str) -> String {
    let created = post_json(
        app,
        "/api/v1/users",
        Some(admin),
        &json!({ "username": username, "password": MEMBER_PW, "role": "member" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    created.json()["id"].as_str().unwrap().to_string()
}

async fn patch(app: &Router, uri: &str, token: Option<&str>, body: &Value) -> Reply {
    send(
        app,
        "PATCH",
        uri,
        token,
        None,
        serde_json::to_vec(body).unwrap(),
    )
    .await
}

async fn authenticated(app: &Router, token: &str) -> bool {
    get_auth(app, "/api/v1/auth/me", Some(token)).await.json()["authenticated"] == true
}

async fn change_password(app: &Router, token: Option<&str>, current: &str, new: &str) -> Reply {
    post_json(
        app,
        "/api/v1/me/password",
        token,
        &json!({ "current_password": current, "new_password": new }),
    )
    .await
}

// ---------------------------------------------------------------------------
// Login throttling
// ---------------------------------------------------------------------------

/// A login as if it arrived over a real socket from `peer`: the `oneshot`
/// harness has no `ConnectInfo`, so it is injected as a request extension,
/// which is exactly where `into_make_service_with_connect_info` puts it.
async fn login_from(app: &Router, peer: SocketAddr, username: &str, password: &str) -> StatusCode {
    let body = serde_json::to_vec(&json!({ "username": username, "password": password })).unwrap();
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/login")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(peer));
    app.clone().oneshot(req).await.unwrap().status()
}

/// However many usernames are sprayed, nobody else's correct password is
/// refused: each made-up name only spends its own per-account budget, and
/// argon2 cost is bounded by the hashing semaphore and the per-client limit,
/// never by refusing other accounts. (Per-account and per-device limits are
/// covered in `tests/login_throttle.rs`.)
#[tokio::test(flavor = "multi_thread")]
async fn a_spray_of_usernames_never_refuses_a_colleagues_correct_password() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    create_member(&app, &admin, "alice").await;

    // Concurrent, so the argon2 verifies really do contend for permits.
    let mut set = tokio::task::JoinSet::new();
    for i in 0..40u8 {
        let app = app.clone();
        let sprayer: SocketAddr = format!("198.51.100.{}:40000", i + 1).parse().unwrap();
        set.spawn(
            async move { login_from(&app, sprayer, &format!("ghost{i}"), "nope-nope").await },
        );
    }
    while let Some(status) = set.join_next().await {
        assert_eq!(status.unwrap(), StatusCode::UNAUTHORIZED);
    }

    let colleague: SocketAddr = "203.0.113.7:50000".parse().unwrap();
    assert_eq!(
        login_from(&app, colleague, "root", ADMIN_PW).await,
        StatusCode::OK
    );
    assert_eq!(
        login_from(&app, colleague, "alice", MEMBER_PW).await,
        StatusCode::OK
    );
}

// ---------------------------------------------------------------------------
// Session revocation on admin reset / disable
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_password_reset_revokes_the_targets_sessions() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    let alice_id = create_member(&app, &admin, "alice").await;
    let alice = token_of(&login(&app, "alice", MEMBER_PW).await);
    assert!(authenticated(&app, &alice).await);

    let reset = patch(
        &app,
        &format!("/api/v1/users/{alice_id}"),
        Some(&admin),
        &json!({ "password": "a-brand-new-pw" }),
    )
    .await;
    assert_eq!(reset.status, StatusCode::OK);

    assert!(!authenticated(&app, &alice).await, "old session must die");
    assert!(authenticated(&app, &admin).await, "admin is untouched");
    assert_eq!(
        login(&app, "alice", "a-brand-new-pw").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_admin_resetting_their_own_password_keeps_the_presenting_session() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    let other = token_of(&login(&app, "root", ADMIN_PW).await);
    let me = get_auth(&app, "/api/v1/auth/me", Some(&admin)).await.json();
    let id = me["user"]["id"].as_str().unwrap().to_string();

    let reset = patch(
        &app,
        &format!("/api/v1/users/{id}"),
        Some(&admin),
        &json!({ "password": "rotated-root-pw" }),
    )
    .await;
    assert_eq!(reset.status, StatusCode::OK);
    assert!(authenticated(&app, &admin).await);
    assert!(!authenticated(&app, &other).await);
}

#[tokio::test]
async fn disabling_revokes_sessions_so_reenabling_does_not_resurrect_them() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    let alice_id = create_member(&app, &admin, "alice").await;
    let alice = token_of(&login(&app, "alice", MEMBER_PW).await);
    let uri = format!("/api/v1/users/{alice_id}");

    let off = patch(&app, &uri, Some(&admin), &json!({ "disabled": true })).await;
    assert_eq!(off.status, StatusCode::OK);
    let on = patch(&app, &uri, Some(&admin), &json!({ "disabled": false })).await;
    assert_eq!(on.status, StatusCode::OK);

    assert!(!authenticated(&app, &alice).await);
    assert_eq!(login(&app, "alice", MEMBER_PW).await.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Self-service password change
// ---------------------------------------------------------------------------

#[tokio::test]
async fn change_password_keeps_the_caller_and_revokes_other_sessions() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    create_member(&app, &admin, "alice").await;
    let laptop = token_of(&login(&app, "alice", MEMBER_PW).await);
    let phone = token_of(&login(&app, "alice", MEMBER_PW).await);

    let r = change_password(&app, Some(&laptop), MEMBER_PW, "alice-new-pw").await;
    assert_eq!(r.status, StatusCode::OK, "body: {:?}", r.json());
    assert_eq!(r.json()["ok"], true);

    assert!(authenticated(&app, &laptop).await, "caller stays signed in");
    assert!(!authenticated(&app, &phone).await, "other session revoked");
    assert_eq!(
        login(&app, "alice", MEMBER_PW).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, "alice", "alice-new-pw").await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn change_password_validates_before_changing_anything() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;

    let wrong = change_password(&app, Some(&admin), "not-my-password", "whatever-new").await;
    // 403, never 401: the web client treats 401 as "session gone".
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);

    let short = change_password(&app, Some(&admin), ADMIN_PW, "short").await;
    assert_eq!(short.status, StatusCode::BAD_REQUEST);

    let unknown_field = post_json(
        &app,
        "/api/v1/me/password",
        Some(&admin),
        &json!({ "current_password": ADMIN_PW, "new_password": "longenough", "x": 1 }),
    )
    .await;
    assert_eq!(unknown_field.status, StatusCode::UNPROCESSABLE_ENTITY);

    // Nothing above changed the password.
    assert_eq!(login(&app, "root", ADMIN_PW).await.status, StatusCode::OK);

    let anon = change_password(&app, None, ADMIN_PW, "longenough").await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn change_password_guesses_are_throttled() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    for _ in 0..5 {
        let r = change_password(&app, Some(&admin), "guess-guess", "whatever-new").await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let locked = change_password(&app, Some(&admin), ADMIN_PW, "whatever-new").await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(locked.headers.contains_key(header::RETRY_AFTER));
    assert_eq!(
        locked.json()["error"],
        "too many failed password attempts; try again in 15 minutes"
    );
    // The refusal changed nothing, and it is its own counter: login is fine.
    assert_eq!(login(&app, "root", ADMIN_PW).await.status, StatusCode::OK);
}

#[tokio::test]
async fn change_password_refuses_static_tokens_and_sso_accounts() {
    let (app, storage, _dir) = test_app_with_storage(
        Settings {
            tokens: Some("admin:tok_admin".to_string()),
            auth_mode: Some(AuthMode::ProtectWrites),
            ..Default::default()
        },
        AuthMode::ProtectWrites,
    )
    .await;
    let r = change_password(&app, Some("tok_admin"), "anything-at-all", "longenough").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // An SSO-provisioned account: the empty password-hash sentinel.
    let sso = storage
        .create_user("sso-user".to_string(), String::new(), Role::Member)
        .await
        .unwrap()
        .unwrap();
    let token = auth::generate_session_token();
    storage
        .create_session(
            auth::token_hash(&token),
            sso.id.clone(),
            auth::session_expiry(chrono::Utc::now().timestamp_millis()),
        )
        .await
        .unwrap();
    let r = change_password(&app, Some(&token), "", "longenough").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["error"].as_str().unwrap().contains("SSO"));
}

// ---------------------------------------------------------------------------
// Expired-session sweep
// ---------------------------------------------------------------------------

#[tokio::test]
async fn expired_sessions_are_pruned_and_live_ones_kept() {
    let (app, storage, _dir) = test_app_with_storage(protected(), AuthMode::ProtectWrites).await;
    let admin = setup_admin(&app).await;
    let me = get_auth(&app, "/api/v1/auth/me", Some(&admin)).await.json();
    let id = domarinn_server::domain::UserId::new(me["user"]["id"].as_str().unwrap());

    let now = chrono::Utc::now().timestamp_millis();
    for (i, expires_at) in [now - 1, now - 86_400_000].into_iter().enumerate() {
        storage
            .create_session(format!("expired-{i}"), id.clone(), expires_at)
            .await
            .unwrap();
    }

    assert_eq!(storage.prune_expired_sessions().await.unwrap(), 2);
    assert_eq!(storage.prune_expired_sessions().await.unwrap(), 0);
    assert!(authenticated(&app, &admin).await, "live session survives");
}
