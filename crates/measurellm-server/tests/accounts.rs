//! Integration tests for the local-accounts subsystem: setup, login, sessions,
//! API keys, and user administration, driven through the real router.

mod common;

use axum::http::StatusCode;
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tempfile::TempDir;

use measurellm_server::{build_app, AuthMode, ServerConfig, Settings};

const ADMIN_PW: &str = "rootpassword";
const MEMBER_PW: &str = "memberpass1";

/// A protect-writes app with no static tokens: writes/admin need accounts.
fn protected() -> Settings {
    Settings {
        auth_mode: Some("protect-writes".to_string()),
        ..Default::default()
    }
}

/// Run `/auth/setup` to create the first admin and return its session token.
async fn setup_admin(app: &Router) -> String {
    let r = post_json(
        app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "setup body: {:?}", r.json());
    r.json()["token"].as_str().unwrap().to_string()
}

/// Create a user via the admin API, then log in as them and return the session.
async fn create_and_login(app: &Router, admin: &str, username: &str, role: &str) -> String {
    let created = post_json(
        app,
        "/api/v1/users",
        Some(admin),
        &json!({ "username": username, "password": MEMBER_PW, "role": role }),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let login = post_json(
        app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": username, "password": MEMBER_PW }),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);
    login.json()["token"].as_str().unwrap().to_string()
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

async fn delete(app: &Router, uri: &str, token: Option<&str>) -> Reply {
    send(app, "DELETE", uri, token, None, Vec::new()).await
}

// ---------------------------------------------------------------------------
// Setup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn setup_creates_first_admin_then_conflicts() {
    let (app, _dir) = test_app(protected()).await;

    let before = get(&app, "/api/v1/meta").await;
    assert_eq!(before.json()["setup_required"], true);
    assert_eq!(before.json()["auth_mode"], "protect-writes");

    let token = setup_admin(&app).await;
    assert!(token.starts_with("mses_"));

    let after = get(&app, "/api/v1/meta").await;
    assert_eq!(after.json()["setup_required"], false);

    // A second setup is refused now that a user exists.
    let second = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "other", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(second.status, StatusCode::CONFLICT);

    // Short passwords are rejected up front.
    let (fresh, _fresh_dir) = test_app(protected()).await;
    let weak = post_json(
        &fresh,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": "short" }),
    )
    .await;
    assert_eq!(weak.status, StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_success_wrong_password_and_disabled() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;

    // Correct credentials.
    let ok = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "root", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(ok.json()["user"]["username"], "root");
    assert_eq!(ok.json()["user"]["role"], "admin");
    assert!(ok.json()["token"].as_str().unwrap().starts_with("mses_"));

    // Wrong password.
    let wrong = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "root", "password": "not-the-password" }),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);

    // Unknown user.
    let unknown = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "ghost", "password": ADMIN_PW }),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::UNAUTHORIZED);

    // A disabled user cannot log in.
    let created = post_json(
        &app,
        "/api/v1/users",
        Some(&admin),
        &json!({ "username": "alice", "password": MEMBER_PW, "role": "member" }),
    )
    .await;
    let alice_id = created.json()["id"].as_str().unwrap().to_string();
    let disabled = patch(
        &app,
        &format!("/api/v1/users/{alice_id}"),
        Some(&admin),
        &json!({ "disabled": true }),
    )
    .await;
    assert_eq!(disabled.status, StatusCode::OK);
    let denied = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "alice", "password": MEMBER_PW }),
    )
    .await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// /auth/me across identity sources
// ---------------------------------------------------------------------------

#[tokio::test]
async fn me_reflects_static_session_and_api_key() {
    let settings = Settings {
        tokens: Some("write:mllm_ci".to_string()),
        ..Default::default()
    };
    let (app, _dir) = test_app(settings).await;
    let session = setup_admin(&app).await;

    // Anonymous.
    let anon = get(&app, "/api/v1/auth/me").await;
    assert_eq!(anon.json()["authenticated"], false);
    assert_eq!(anon.json()["source"], "anonymous");

    // Static token.
    let stat = get_auth(&app, "/api/v1/auth/me", Some("mllm_ci")).await;
    assert_eq!(stat.json()["authenticated"], true);
    assert_eq!(stat.json()["source"], "static");
    assert_eq!(stat.json()["scope"], "write");
    assert!(stat.json()["user"].is_null());

    // Session (admin role -> admin scope).
    let sess = get_auth(&app, "/api/v1/auth/me", Some(&session)).await;
    assert_eq!(sess.json()["source"], "session");
    assert_eq!(sess.json()["scope"], "admin");
    assert_eq!(sess.json()["user"]["username"], "root");

    // API key.
    let created = post_json(
        &app,
        "/api/v1/apikeys",
        Some(&session),
        &json!({ "name": "ci", "scope": "read" }),
    )
    .await;
    let key = created.json()["key"].as_str().unwrap().to_string();
    let via_key = get_auth(&app, "/api/v1/auth/me", Some(&key)).await;
    assert_eq!(via_key.json()["source"], "apikey");
    assert_eq!(via_key.json()["scope"], "read");
    assert_eq!(via_key.json()["user"]["username"], "root");
}

// ---------------------------------------------------------------------------
// API keys
// ---------------------------------------------------------------------------

#[tokio::test]
async fn api_key_lifecycle_and_scope_ceiling() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;

    // Create a key: secret shown exactly once, list never reveals it.
    let created = post_json(
        &app,
        "/api/v1/apikeys",
        Some(&admin),
        &json!({ "name": "deploy", "scope": "write" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let key = created.json()["key"].as_str().unwrap().to_string();
    let key_id = created.json()["id"].as_str().unwrap().to_string();
    assert!(key.starts_with("mllm_"));
    assert_eq!(created.json()["scope"], "write");

    let listed = get_auth(&app, "/api/v1/apikeys", Some(&admin)).await;
    let keys = listed.json()["keys"].as_array().unwrap().clone();
    assert_eq!(keys.len(), 1);
    assert!(keys[0].get("key").is_none(), "secret must never be listed");
    assert_eq!(keys[0]["revoked"], false);

    // The key authenticates a subsequent write.
    let write = post_json(
        &app,
        "/api/v1/runs",
        Some(&key),
        &run_value(&simple_run("k1")),
    )
    .await;
    assert_eq!(write.status, StatusCode::CREATED);

    // A member may not mint a key above their own (Write) scope.
    let member = create_and_login(&app, &admin, "bob", "member").await;
    let too_high = post_json(
        &app,
        "/api/v1/apikeys",
        Some(&member),
        &json!({ "scope": "admin" }),
    )
    .await;
    assert_eq!(too_high.status, StatusCode::FORBIDDEN);
    // ... but a read key (<= write) is fine, and the default is their own scope.
    let ok_key = post_json(&app, "/api/v1/apikeys", Some(&member), &json!({})).await;
    assert_eq!(ok_key.status, StatusCode::CREATED);
    assert_eq!(ok_key.json()["scope"], "write");

    // Revoke, then the key no longer authenticates.
    let revoked = delete(&app, &format!("/api/v1/apikeys/{key_id}"), Some(&admin)).await;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);

    let after = get_auth(&app, "/api/v1/auth/me", Some(&key)).await;
    assert_eq!(after.json()["authenticated"], false);
    let blocked = post_json(
        &app,
        "/api/v1/runs",
        Some(&key),
        &run_value(&simple_run("k2")),
    )
    .await;
    assert_eq!(blocked.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_key_delete_requires_owner_or_admin() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;
    let member = create_and_login(&app, &admin, "carol", "member").await;

    // Member mints a key.
    let created = post_json(&app, "/api/v1/apikeys", Some(&member), &json!({})).await;
    let key_id = created.json()["id"].as_str().unwrap().to_string();

    // Another member cannot revoke it.
    let mallory = create_and_login(&app, &admin, "mallory", "member").await;
    let forbidden = delete(&app, &format!("/api/v1/apikeys/{key_id}"), Some(&mallory)).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);

    // The admin can (owner-or-admin).
    let ok = delete(&app, &format!("/api/v1/apikeys/{key_id}"), Some(&admin)).await;
    assert_eq!(ok.status, StatusCode::NO_CONTENT);
}

// ---------------------------------------------------------------------------
// User administration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_crud_last_admin_and_authz() {
    let (app, _dir) = test_app(protected()).await;
    let admin = setup_admin(&app).await;

    // Admin lists users (just root so far).
    let list = get_auth(&app, "/api/v1/users", Some(&admin)).await;
    assert_eq!(list.json()["users"].as_array().unwrap().len(), 1);

    // Create a member and confirm role.
    let member = create_and_login(&app, &admin, "dave", "member").await;

    // A member may not touch the admin API.
    assert_eq!(
        get_auth(&app, "/api/v1/users", Some(&member)).await.status,
        StatusCode::FORBIDDEN
    );
    let member_create = post_json(
        &app,
        "/api/v1/users",
        Some(&member),
        &json!({ "username": "eve", "password": MEMBER_PW, "role": "member" }),
    )
    .await;
    assert_eq!(member_create.status, StatusCode::FORBIDDEN);

    // Fetch dave's id and promote him to admin via PATCH.
    let users = get_auth(&app, "/api/v1/users", Some(&admin)).await;
    let dave_id = users.json()["users"]
        .as_array()
        .unwrap()
        .iter()
        .find(|u| u["username"] == "dave")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let promoted = patch(
        &app,
        &format!("/api/v1/users/{dave_id}"),
        Some(&admin),
        &json!({ "role": "admin" }),
    )
    .await;
    assert_eq!(promoted.status, StatusCode::OK);
    assert_eq!(promoted.json()["role"], "admin");

    // Root is no longer the last admin, so it can be deleted.
    let root_id = get_auth(&app, "/api/v1/auth/me", Some(&admin)).await.json()["user"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let del_root = delete(&app, &format!("/api/v1/users/{root_id}"), Some(&admin)).await;
    assert_eq!(del_root.status, StatusCode::NO_CONTENT);

    // Dave (now the only admin) logs in and cannot be deleted.
    let dave = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "dave", "password": MEMBER_PW }),
    )
    .await;
    let dave_token = dave.json()["token"].as_str().unwrap().to_string();
    let del_last = delete(&app, &format!("/api/v1/users/{dave_id}"), Some(&dave_token)).await;
    assert_eq!(del_last.status, StatusCode::CONFLICT);

    // Deleting a missing user is a 404.
    let missing = delete(&app, "/api/v1/users/does-not-exist", Some(&dave_token)).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Sessions / logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_revokes_the_session() {
    let (app, _dir) = test_app(protected()).await;
    let token = setup_admin(&app).await;

    // The session works.
    assert_eq!(
        get_auth(&app, "/api/v1/auth/me", Some(&token)).await.json()["authenticated"],
        true
    );

    // Logout revokes it.
    let out = send(
        &app,
        "POST",
        "/api/v1/auth/logout",
        Some(&token),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(out.status, StatusCode::OK);

    // The same token is now anonymous, and gated routes reject it.
    let after = get_auth(&app, "/api/v1/auth/me", Some(&token)).await;
    assert_eq!(after.json()["authenticated"], false);
    let write = post_json(
        &app,
        "/api/v1/runs",
        Some(&token),
        &run_value(&simple_run("s1")),
    )
    .await;
    assert_eq!(write.status, StatusCode::UNAUTHORIZED);

    // Anonymous logout is a 401.
    let anon = send(&app, "POST", "/api/v1/auth/logout", None, None, Vec::new()).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Bootstrap admin from settings/env
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bootstrap_admin_is_idempotent_and_rotates_password() {
    let dir = TempDir::new().unwrap();
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: AuthMode::Open,
    };
    let settings = Settings {
        admin_user: Some("boss".to_string()),
        admin_password: Some("bosspassword".to_string()),
        ..Default::default()
    };
    let (app, _state) = build_app(&config, settings).await.unwrap();

    // A seeded account flips the effective mode to protect-writes.
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "protect-writes");
    assert_eq!(meta.json()["setup_required"], false);

    let login = post_json(
        &app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "boss", "password": "bosspassword" }),
    )
    .await;
    assert_eq!(login.status, StatusCode::OK);

    // Re-open the same data dir with a rotated password: idempotent, one user,
    // old password now fails and the new one works.
    let rotated = Settings {
        admin_user: Some("boss".to_string()),
        admin_password: Some("rotatedpassword".to_string()),
        ..Default::default()
    };
    let (app2, _state2) = build_app(&config, rotated).await.unwrap();

    let old = post_json(
        &app2,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "boss", "password": "bosspassword" }),
    )
    .await;
    assert_eq!(old.status, StatusCode::UNAUTHORIZED);

    let new = post_json(
        &app2,
        "/api/v1/auth/login",
        None,
        &json!({ "username": "boss", "password": "rotatedpassword" }),
    )
    .await;
    assert_eq!(new.status, StatusCode::OK);
    let token = new.json()["token"].as_str().unwrap().to_string();
    let users = get_auth(&app2, "/api/v1/users", Some(&token)).await;
    assert_eq!(users.json()["users"].as_array().unwrap().len(), 1);
}
