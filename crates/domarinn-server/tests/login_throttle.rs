//! Password-login throttling: the per-account (untrusted) and per-device
//! (trusted) counters, the device cookie that picks between them, and the
//! guarantee they exist for — an attacker hammering an account cannot keep
//! its owner out of a browser they have signed in from before.
//!
//! Every test runs on both backends through `common` (`mise run test-pg`).
//! None of them inserts `ConnectInfo`, so the in-memory per-client limit
//! (tested in `accounts::guard`) stays out of the way.

mod common;

use axum::http::{header, StatusCode};
use axum::Router;
use common::*;
use serde_json::json;

use domarinn_server::{AuthMode, Settings};

const ADMIN_PW: &str = "rootpassword";
const MEMBER_PW: &str = "memberpass1";
const WRONG_PW: &str = "wrong-password";

/// Every attempt a window allows on either path before the next is refused.
const BUDGET: usize = 10;

/// The untrusted refusal for a first lock (5 minutes), word for word.
const UNTRUSTED_LOCKED: &str = "too many failed sign-in attempts for this account from \
     unrecognised browsers; try again in 5 minutes, or sign in from a browser you've used before";
/// The per-device refusal (a fixed 15-minute lock).
const DEVICE_LOCKED: &str =
    "too many failed sign-in attempts from this browser; try again in 15 minutes";

async fn app() -> (Router, tempfile::TempDir) {
    test_app_with_mode(Settings::default(), AuthMode::ProtectWrites).await
}

/// `POST /auth/login`, presenting `cookie` (a whole `Cookie` header value)
/// when given.
async fn login_with(app: &Router, username: &str, password: &str, cookie: Option<&str>) -> Reply {
    let body = serde_json::to_vec(&json!({ "username": username, "password": password })).unwrap();
    let headers: Vec<(&str, &str)> = cookie.map(|c| ("cookie", c)).into_iter().collect();
    send_with_headers(app, "POST", "/api/v1/auth/login", &headers, body).await
}

/// The `domarinn_device=…` pair a response set, ready to send back as a
/// `Cookie` header.
fn device_cookie_of(reply: &Reply) -> String {
    reply
        .set_cookies()
        .iter()
        .find(|c| c.starts_with("domarinn_device="))
        .and_then(|c| c.split(';').next())
        .unwrap_or_else(|| panic!("no device cookie in {:?}", reply.set_cookies()))
        .to_string()
}

fn error_of(reply: &Reply) -> String {
    reply.json()["error"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn retry_after(reply: &Reply) -> u64 {
    reply.headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
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

/// A successful login's device cookie.
async fn device_for(app: &Router, username: &str, password: &str) -> String {
    let r = login_with(app, username, password, None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", error_of(&r));
    device_cookie_of(&r)
}

/// Spend the untrusted budget of `username` with wrong passwords (each a
/// 401), then show the next attempt is refused.
async fn lock_untrusted(app: &Router, username: &str) {
    for i in 0..BUDGET {
        let r = login_with(app, username, WRONG_PW, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let r = login_with(app, username, WRONG_PW, None).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

/// The anti-goal, directly: the account is under attack and locked for
/// unrecognised browsers, yet the owner's own browser still signs in.
#[tokio::test]
async fn an_attacker_cannot_lock_the_owner_out_of_a_known_browser() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;
    let owner = device_for(&app, "root", ADMIN_PW).await;

    for i in 0..BUDGET {
        let r = login_with(&app, "root", WRONG_PW, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let locked = login_with(&app, "root", WRONG_PW, None).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_of(&locked), UNTRUSTED_LOCKED);
    let wait = retry_after(&locked);
    assert!((1..=300).contains(&wait), "retry_after={wait}");

    let known = login_with(&app, "root", ADMIN_PW, Some(&owner)).await;
    assert_eq!(known.status, StatusCode::OK, "{}", error_of(&known));
    let unknown = login_with(&app, "root", ADMIN_PW, None).await;
    assert_eq!(unknown.status, StatusCode::TOO_MANY_REQUESTS);
}

/// Neither the statuses, the bodies, nor the lock's timing tell an existing
/// account from a made-up one.
#[tokio::test]
async fn an_unknown_username_is_indistinguishable() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;

    let mut seen = Vec::new();
    for username in ["root", "ghost"] {
        let mut replies = Vec::new();
        for _ in 0..=BUDGET {
            replies.push(login_with(&app, username, WRONG_PW, None).await);
        }
        seen.push(replies);
    }
    let (root, ghost) = (&seen[0], &seen[1]);
    for (i, (r, g)) in root.iter().zip(ghost).enumerate() {
        assert_eq!(r.status, g.status, "attempt {i}");
        assert_eq!(r.body, g.body, "attempt {i}");
        assert_eq!(
            r.headers.contains_key(header::RETRY_AFTER),
            g.headers.contains_key(header::RETRY_AFTER),
            "attempt {i}"
        );
    }
    let (r, g) = (&root[BUDGET], &ghost[BUDGET]);
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_of(g), UNTRUSTED_LOCKED);
    assert!(retry_after(r).abs_diff(retry_after(g)) <= 1);
}

#[tokio::test]
async fn the_device_cookie_survives_logout() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;
    let signed_in = login_with(&app, "root", ADMIN_PW, None).await;
    assert_eq!(signed_in.status, StatusCode::OK);
    let device = device_cookie_of(&signed_in);

    let out = send(
        &app,
        "POST",
        "/api/v1/auth/logout",
        Some(&token_of(&signed_in)),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(out.status, StatusCode::OK);
    assert!(
        !out.set_cookies()
            .iter()
            .any(|c| c.starts_with("domarinn_device")),
        "logout must leave the device cookie alone: {:?}",
        out.set_cookies()
    );

    // With the untrusted path locked, only the trusted path can let it in.
    lock_untrusted(&app, "root").await;
    let back = login_with(&app, "root", ADMIN_PW, Some(&device)).await;
    assert_eq!(back.status, StatusCode::OK, "{}", error_of(&back));
}

#[tokio::test]
async fn a_device_cookie_for_another_account_is_ignored() {
    let (app, _dir) = app().await;
    let admin = setup_admin(&app).await;
    create_member(&app, &admin, "alice").await;
    let alices = device_for(&app, "alice", MEMBER_PW).await;

    lock_untrusted(&app, "root").await;
    let r = login_with(&app, "root", ADMIN_PW, Some(&alices)).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_of(&r), UNTRUSTED_LOCKED);
}

#[tokio::test]
async fn a_trusted_device_locks_only_itself() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;
    let d1 = device_for(&app, "root", ADMIN_PW).await;
    let d2 = device_for(&app, "root", ADMIN_PW).await;
    assert_ne!(d1, d2, "each browser gets its own nonce");

    for i in 0..BUDGET {
        let r = login_with(&app, "root", WRONG_PW, Some(&d1)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let locked = login_with(&app, "root", WRONG_PW, Some(&d1)).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_of(&locked), DEVICE_LOCKED);
    assert!(retry_after(&locked) <= 15 * 60);

    let other = login_with(&app, "root", WRONG_PW, Some(&d2)).await;
    assert_eq!(other.status, StatusCode::UNAUTHORIZED);
    let untrusted = login_with(&app, "root", WRONG_PW, None).await;
    assert_eq!(untrusted.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn success_clears_only_its_own_path() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;
    let device = device_for(&app, "root", ADMIN_PW).await;

    for i in 0..BUDGET - 1 {
        let r = login_with(&app, "root", WRONG_PW, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    // A trusted success clears the device's counter, not the account's.
    let trusted = login_with(&app, "root", ADMIN_PW, Some(&device)).await;
    assert_eq!(trusted.status, StatusCode::OK);

    let tenth = login_with(&app, "root", WRONG_PW, None).await;
    assert_eq!(tenth.status, StatusCode::UNAUTHORIZED);
    let eleventh = login_with(&app, "root", WRONG_PW, None).await;
    assert_eq!(eleventh.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn an_admin_reset_clears_the_untrusted_lockout() {
    let (app, _dir) = app().await;
    let admin = setup_admin(&app).await;
    let alice = create_member(&app, &admin, "alice").await;

    lock_untrusted(&app, "alice").await;
    let reset = send(
        &app,
        "PATCH",
        &format!("/api/v1/users/{alice}"),
        Some(&admin),
        None,
        serde_json::to_vec(&json!({ "password": "a-new-password" })).unwrap(),
    )
    .await;
    assert_eq!(reset.status, StatusCode::OK);

    let r = login_with(&app, "alice", "a-new-password", None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", error_of(&r));
}

async fn change_password(app: &Router, token: &str, current: &str, new: &str) -> Reply {
    post_json(
        app,
        "/api/v1/me/password",
        Some(token),
        &json!({ "current_password": current, "new_password": new }),
    )
    .await
}

#[tokio::test]
async fn change_password_guesses_are_throttled_per_account() {
    let (app, _dir) = app().await;
    let admin = setup_admin(&app).await;
    create_member(&app, &admin, "alice").await;
    let alice = token_of(&login_with(&app, "alice", MEMBER_PW, None).await);

    for i in 0..5 {
        let r = change_password(&app, &admin, "guess-guess", "whatever-new").await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "attempt {i}");
    }
    let locked = change_password(&app, &admin, ADMIN_PW, "whatever-new").await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        error_of(&locked),
        "too many failed password attempts; try again in 15 minutes"
    );
    assert!((1..=15 * 60).contains(&retry_after(&locked)));

    let theirs = change_password(&app, &alice, MEMBER_PW, "alice-new-pw").await;
    assert_eq!(theirs.status, StatusCode::OK, "{}", error_of(&theirs));
}

/// A cookie that does not verify, names a user that is gone, or comes in
/// duplicate headers is simply absent: the attempt is untrusted, never an
/// error.
#[tokio::test]
async fn garbage_device_cookies_fall_back_to_the_untrusted_path() {
    let (app, _dir) = app().await;
    let admin = setup_admin(&app).await;
    let bob = create_member(&app, &admin, "bob").await;
    let bobs = device_for(&app, "bob", MEMBER_PW).await;
    let deleted = send(
        &app,
        "DELETE",
        &format!("/api/v1/users/{bob}"),
        Some(&admin),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);

    let forged = "domarinn_device=v1.x.y.z";
    let r = login_with(&app, "root", WRONG_PW, Some(forged)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = login_with(&app, "bob", MEMBER_PW, Some(&bobs)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "the user is gone");
    let body = serde_json::to_vec(&json!({ "username": "root", "password": WRONG_PW })).unwrap();
    let r = send_with_headers(
        &app,
        "POST",
        "/api/v1/auth/login",
        &[("cookie", forged), ("cookie", "domarinn_device=v1..;x=1")],
        body,
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Two of root's untrusted attempts are spent; spend the rest the same way.
    for i in 2..BUDGET {
        let r = login_with(&app, "root", WRONG_PW, Some(forged)).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let r = login_with(&app, "root", ADMIN_PW, Some(forged)).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(error_of(&r), UNTRUSTED_LOCKED);
}

/// The account key is a hash of the trimmed username: a huge name cannot
/// bloat a row, and padding the name does not buy a fresh budget.
#[tokio::test]
async fn a_huge_username_is_a_fixed_size_key_and_padding_shares_the_budget() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;

    let huge = "a".repeat(100_000);
    let r = login_with(&app, &huge, WRONG_PW, None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    for i in 0..BUDGET {
        let r = login_with(&app, " root ", WRONG_PW, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    let r = login_with(&app, "root", ADMIN_PW, None).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

/// Two replicas over one database share one guessing budget, and each
/// accepts the other's device cookies (the signing key is in the database).
#[tokio::test]
async fn two_instances_over_one_database_share_counters() {
    let (a, b, _dir) = test_app_pair(Settings::default(), AuthMode::ProtectWrites).await;
    setup_admin(&a).await;
    let minted_on_a = device_for(&a, "root", ADMIN_PW).await;

    for i in 0..BUDGET {
        let app = if i % 2 == 0 { &a } else { &b };
        let r = login_with(app, "root", WRONG_PW, None).await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "attempt {i}");
    }
    for app in [&a, &b] {
        let r = login_with(app, "root", ADMIN_PW, None).await;
        assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    }
    let r = login_with(&b, "root", ADMIN_PW, Some(&minted_on_a)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", error_of(&r));
}

/// Attempts are reserved before the password is verified, so a burst of
/// concurrent guesses cannot all read "under budget" and all be verified.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_wrong_guesses_get_at_most_ten_verifies() {
    let (app, _dir) = app().await;
    setup_admin(&app).await;

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..20 {
        let app = app.clone();
        set.spawn(async move { login_with(&app, "root", WRONG_PW, None).await.status });
    }
    let (mut refused, mut rejected) = (0, 0);
    while let Some(status) = set.join_next().await {
        match status.unwrap() {
            StatusCode::UNAUTHORIZED => rejected += 1,
            StatusCode::TOO_MANY_REQUESTS => refused += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert!(rejected <= BUDGET, "{rejected} guesses were verified");
    assert_eq!(rejected + refused, 20);
}
