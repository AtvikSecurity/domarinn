mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_server::{AuthMode, Settings};
use serde_json::json;

const TOKENS: &str = "read:domarinn_view,write:domarinn_ci,admin:domarinn_ops";

fn protect_writes() -> Settings {
    Settings {
        tokens: Some(TOKENS.to_string()),
        auth_mode: Some(AuthMode::ProtectWrites),
        ..Default::default()
    }
}

fn closed() -> Settings {
    Settings {
        tokens: Some(TOKENS.to_string()),
        auth_mode: Some(AuthMode::Closed),
        ..Default::default()
    }
}

#[tokio::test]
async fn open_mode_allows_everything_anonymously() {
    let (app, _dir) = test_app(Settings::default()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "open");

    // Anonymous write + admin both succeed in open mode.
    let created = post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("o1"))).await;
    assert_eq!(created.status, StatusCode::CREATED);
    let deleted = send(&app, "DELETE", "/api/v1/runs/o1", None, None, Vec::new()).await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn protect_writes_reads_open_writes_gated() {
    let (app, _dir) = test_app(protect_writes()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "protect-writes");

    // Reads are open without a token.
    assert_eq!(get(&app, "/api/v1/runs").await.status, StatusCode::OK);

    // Write with no token -> 401.
    let anon = post_json(&app, "/api/v1/runs", None, &run_value(&simple_run("p1"))).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);

    // Write with a read-only token -> 403.
    let reader = post_json(
        &app,
        "/api/v1/runs",
        Some("domarinn_view"),
        &run_value(&simple_run("p1")),
    )
    .await;
    assert_eq!(reader.status, StatusCode::FORBIDDEN);

    // Write with a write token -> ok.
    let writer = post_json(
        &app,
        "/api/v1/runs",
        Some("domarinn_ci"),
        &run_value(&simple_run("p1")),
    )
    .await;
    assert_eq!(writer.status, StatusCode::CREATED);

    // Admin route (delete) with a write token -> 403.
    let write_delete = send(
        &app,
        "DELETE",
        "/api/v1/runs/p1",
        Some("domarinn_ci"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(write_delete.status, StatusCode::FORBIDDEN);

    // Admin route with an admin token -> ok.
    let admin_delete = send(
        &app,
        "DELETE",
        "/api/v1/runs/p1",
        Some("domarinn_ops"),
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(admin_delete.status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn closed_mode_gates_reads_too() {
    let (app, _dir) = test_app(closed()).await;
    let meta = get(&app, "/api/v1/meta").await;
    assert_eq!(meta.json()["auth_mode"], "closed");

    // Open meta/health still work (they are not scoped).
    assert_eq!(get(&app, "/api/v1/health").await.status, StatusCode::OK);

    // Reads require a token.
    assert_eq!(
        get_auth(&app, "/api/v1/runs", None).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        get_auth(&app, "/api/v1/runs", Some("domarinn_view"))
            .await
            .status,
        StatusCode::OK
    );

    // Writes still require write scope.
    let reader_write = post_json(
        &app,
        "/api/v1/runs",
        Some("domarinn_view"),
        &run_value(&simple_run("c1")),
    )
    .await;
    assert_eq!(reader_write.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_token_is_treated_as_anonymous() {
    let (app, _dir) = test_app(protect_writes()).await;
    let bad = post_json(
        &app,
        "/api/v1/runs",
        Some("not-a-real-token"),
        &run_value(&simple_run("x1")),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED);
}

/// A key's effective scope is capped by its owner's **current** role, not the
/// role they held when it was minted.
///
/// Without the cap a demotion is cosmetic: the `viewer` role's whole guarantee
/// is "this account cannot write", and a key minted the day before would keep
/// writing; worse, a demoted admin would keep an admin-scoped key and could
/// lift any run set's restriction and then read its runs. Revoking every key by
/// hand on every demotion is not a control anyone actually operates.
///
/// Closed mode so reads are gated too, and the surviving read on the demoted
/// key is a real assertion rather than the anonymous waiver.
#[tokio::test]
async fn a_key_is_capped_by_its_owners_current_role() {
    const PW: &str = "accountpassword";
    let (app, _dir) = test_app_with_mode(Settings::default(), AuthMode::Closed).await;

    let setup = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": PW }),
    )
    .await;
    assert_eq!(setup.status, StatusCode::CREATED, "{:?}", setup.json());
    let admin = setup.json()["token"].as_str().unwrap().to_string();

    // Mint a key at `scope` for a freshly created account of `role`.
    let account_key = |username: &'static str, role: &'static str, scope: &'static str| {
        let app = app.clone();
        let admin = admin.clone();
        async move {
            let created = post_json(
                &app,
                "/api/v1/users",
                Some(&admin),
                &json!({ "username": username, "password": PW, "role": role }),
            )
            .await;
            assert_eq!(created.status, StatusCode::CREATED, "creating {username}");
            let login = post_json(
                &app,
                "/api/v1/auth/login",
                None,
                &json!({ "username": username, "password": PW }),
            )
            .await;
            let session = login.json()["token"].as_str().unwrap().to_string();
            let minted = post_json(
                &app,
                "/api/v1/apikeys",
                Some(&session),
                &json!({ "name": "ci", "scope": scope }),
            )
            .await;
            assert_eq!(minted.status, StatusCode::CREATED, "{:?}", minted.json());
            (
                created.json()["id"].as_str().unwrap().to_string(),
                minted.json()["key"].as_str().unwrap().to_string(),
            )
        }
    };
    let (mel_id, mel_key) = account_key("mel", "member", "write").await;
    let (dana_id, dana_key) = account_key("dana", "admin", "admin").await;
    let (_cara_id, cara_key) = account_key("cara", "member", "write").await;

    let upload = |key: String, run_id: &'static str| {
        let app = app.clone();
        async move {
            post_json(
                &app,
                "/api/v1/runs",
                Some(&key),
                &run_value(&simple_run(run_id)),
            )
            .await
            .status
        }
    };
    let restriction = |method: &'static str, key: String| {
        let app = app.clone();
        async move {
            send(
                &app,
                method,
                "/api/v1/sets/lockme/restriction",
                Some(&key),
                None,
                Vec::new(),
            )
            .await
            .status
        }
    };

    // Both keys work at their minted scope while their owners' roles back them.
    assert_eq!(
        upload(mel_key.clone(), "pre-demotion").await,
        StatusCode::CREATED
    );
    assert_eq!(
        restriction("PUT", dana_key.clone()).await,
        StatusCode::NO_CONTENT,
        "an admin's admin key restricts a set"
    );

    // Demote both owners. Root stays the last enabled admin, so dana may go.
    for (id, role) in [(mel_id, "viewer"), (dana_id, "member")] {
        let patched = send(
            &app,
            "PATCH",
            &format!("/api/v1/users/{id}"),
            Some(&admin),
            None,
            serde_json::to_vec(&json!({ "role": role })).unwrap(),
        )
        .await;
        assert_eq!(patched.status, StatusCode::OK, "demote to {role}");
    }

    // The demoted member's write key is read-only now: reads live, writes die.
    assert_eq!(
        get_auth(&app, "/api/v1/runs", Some(&mel_key)).await.status,
        StatusCode::OK,
        "a viewer's old write key still reads"
    );
    assert_eq!(
        upload(mel_key, "post-demotion").await,
        StatusCode::FORBIDDEN,
        "a viewer's old write key must not upload"
    );

    // The demoted admin's admin key can no longer touch policy.
    assert_eq!(
        restriction("DELETE", dana_key).await,
        StatusCode::FORBIDDEN,
        "a member's old admin key must not lift a restriction"
    );

    // An undemoted owner's key is untouched.
    assert_eq!(
        upload(cara_key, "undemoted").await,
        StatusCode::CREATED,
        "an undemoted member's write key still uploads"
    );
}
