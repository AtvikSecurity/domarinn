//! Run-set access control: the policy store, and the visibility matrix every
//! read surface is filtered by.

mod common;

use domarinn_server::domain::{Role, UserId};
use domarinn_server::runsets::{GrantLevel, RunVisibility};
use domarinn_server::storage::Storage;
use tempfile::TempDir;

async fn storage() -> (Storage, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let storage = Storage::open(dir.path().to_path_buf())
        .await
        .expect("open storage");
    (storage, dir)
}

async fn user(storage: &Storage, username: &str, role: Role) -> UserId {
    storage
        .create_user(username.to_string(), "x".to_string(), role)
        .await
        .expect("create user")
        .expect("username free")
        .id
}

// ---------------------------------------------------------------------------
// Policy storage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn restricting_a_set_is_idempotent_and_reversible() {
    let (storage, _dir) = storage().await;

    assert!(storage
        .restrict_run_set("checkout".into(), None, Some("root".into()))
        .await
        .unwrap());
    // A second call is a no-op, not a constraint violation.
    assert!(!storage
        .restrict_run_set("checkout".into(), None, Some("root".into()))
        .await
        .unwrap());

    assert!(storage
        .run_set_restricted(Some("checkout".into()), Some("smoke".into()))
        .await
        .unwrap());

    assert!(storage
        .unrestrict_run_set("checkout".into(), None)
        .await
        .unwrap());
    assert!(!storage
        .unrestrict_run_set("checkout".into(), None)
        .await
        .unwrap());
    assert!(!storage
        .run_set_restricted(Some("checkout".into()), Some("smoke".into()))
        .await
        .unwrap());
}

#[tokio::test]
async fn a_project_restriction_covers_every_suite_a_suite_one_covers_only_itself() {
    let (storage, _dir) = storage().await;
    storage
        .restrict_run_set("secret".into(), None, None)
        .await
        .unwrap();
    storage
        .restrict_run_set("open".into(), Some("locked".into()), None)
        .await
        .unwrap();

    for suite in [Some("a".to_string()), Some("b".to_string()), None] {
        assert!(
            storage
                .run_set_restricted(Some("secret".into()), suite.clone())
                .await
                .unwrap(),
            "project-level restriction must cover suite {suite:?}"
        );
    }

    assert!(storage
        .run_set_restricted(Some("open".into()), Some("locked".into()))
        .await
        .unwrap());
    assert!(!storage
        .run_set_restricted(Some("open".into()), Some("public".into()))
        .await
        .unwrap());
    // A suite-level restriction never covers the project's suite-less runs.
    assert!(!storage
        .run_set_restricted(Some("open".into()), None)
        .await
        .unwrap());
    // A run with no project can never be covered.
    assert!(!storage.run_set_restricted(None, None).await.unwrap());
}

#[tokio::test]
async fn grants_are_upserted_per_scope_and_list_with_their_usernames() {
    let (storage, _dir) = storage().await;
    let alice = user(&storage, "alice", Role::Member).await;
    let bob = user(&storage, "bob", Role::Viewer).await;

    storage
        .upsert_run_set_grant(
            "checkout".into(),
            None,
            alice.clone(),
            GrantLevel::View,
            Some("root".into()),
        )
        .await
        .unwrap();
    // Re-granting the same scope re-levels rather than duplicating.
    storage
        .upsert_run_set_grant(
            "checkout".into(),
            None,
            alice.clone(),
            GrantLevel::Manage,
            Some("root".into()),
        )
        .await
        .unwrap();
    storage
        .upsert_run_set_grant("checkout".into(), None, bob.clone(), GrantLevel::View, None)
        .await
        .unwrap();
    // A suite-level grant is a different row from the project-level one.
    storage
        .upsert_run_set_grant(
            "checkout".into(),
            Some("smoke".into()),
            bob.clone(),
            GrantLevel::Upload,
            None,
        )
        .await
        .unwrap();

    let project_grants = storage
        .list_run_set_grants("checkout".into(), None)
        .await
        .unwrap();
    let listed: Vec<(&str, GrantLevel)> = project_grants
        .iter()
        .map(|g| (g.username.as_str(), g.level))
        .collect();
    assert_eq!(
        listed,
        [("alice", GrantLevel::Manage), ("bob", GrantLevel::View)]
    );

    let suite_grants = storage
        .list_run_set_grants("checkout".into(), Some("smoke".into()))
        .await
        .unwrap();
    assert_eq!(suite_grants.len(), 1);
    assert_eq!(suite_grants[0].username, "bob");
    assert_eq!(suite_grants[0].level, GrantLevel::Upload);

    // The covering level takes the strongest of the rows that reach the suite.
    assert_eq!(
        storage
            .run_set_grant_level(Some("checkout".into()), Some("smoke".into()), bob.clone())
            .await
            .unwrap(),
        Some(GrantLevel::Upload)
    );
    assert_eq!(
        storage
            .run_set_grant_level(Some("checkout".into()), Some("other".into()), bob.clone())
            .await
            .unwrap(),
        Some(GrantLevel::View)
    );
    assert_eq!(
        storage
            .run_set_grant_level(Some("checkout".into()), Some("smoke".into()), alice.clone())
            .await
            .unwrap(),
        Some(GrantLevel::Manage)
    );

    assert!(storage
        .delete_run_set_grant("checkout".into(), Some("smoke".into()), bob.clone())
        .await
        .unwrap());
    assert!(!storage
        .delete_run_set_grant("checkout".into(), Some("smoke".into()), bob.clone())
        .await
        .unwrap());
    // Deleting the suite grant left the project one alone.
    assert_eq!(
        storage
            .run_set_grant_level(Some("checkout".into()), Some("smoke".into()), bob)
            .await
            .unwrap(),
        Some(GrantLevel::View)
    );
}

#[tokio::test]
async fn set_access_answers_every_class_and_level() {
    let (storage, _dir) = storage().await;
    let viewer = user(&storage, "viewer", Role::Viewer).await;
    let uploader = user(&storage, "uploader", Role::Member).await;
    let stranger = user(&storage, "stranger", Role::Member).await;

    storage
        .restrict_run_set("locked".into(), None, None)
        .await
        .unwrap();
    storage
        .upsert_run_set_grant(
            "locked".into(),
            None,
            viewer.clone(),
            GrantLevel::View,
            None,
        )
        .await
        .unwrap();
    storage
        .upsert_run_set_grant(
            "locked".into(),
            None,
            uploader.clone(),
            GrantLevel::Upload,
            None,
        )
        .await
        .unwrap();

    let check = |vis: RunVisibility, project: Option<&str>, needed| {
        let storage = storage.clone();
        let project = project.map(str::to_string);
        async move {
            storage
                .set_access(vis, project, Some("smoke".into()), needed)
                .await
                .unwrap()
        }
    };

    // Admins are never filtered.
    assert!(check(RunVisibility::Full, Some("locked"), GrantLevel::Manage).await);

    // Public: fine on an open set at any level, closed on a restricted one.
    assert!(check(RunVisibility::Public, Some("open"), GrantLevel::Upload).await);
    assert!(check(RunVisibility::Public, None, GrantLevel::Upload).await);
    assert!(!check(RunVisibility::Public, Some("locked"), GrantLevel::View).await);
    assert!(!check(RunVisibility::Public, Some("locked"), GrantLevel::Upload).await);

    // A user with no grant is exactly as blocked as an anonymous caller.
    let nobody = RunVisibility::User(stranger);
    assert!(check(nobody.clone(), Some("open"), GrantLevel::Upload).await);
    assert!(!check(nobody, Some("locked"), GrantLevel::View).await);

    // Grant levels are ordered: view reads but does not upload.
    let viewing = RunVisibility::User(viewer);
    assert!(check(viewing.clone(), Some("locked"), GrantLevel::View).await);
    assert!(!check(viewing.clone(), Some("locked"), GrantLevel::Upload).await);
    assert!(!check(viewing, Some("locked"), GrantLevel::Manage).await);

    let uploading = RunVisibility::User(uploader);
    assert!(check(uploading.clone(), Some("locked"), GrantLevel::View).await);
    assert!(check(uploading.clone(), Some("locked"), GrantLevel::Upload).await);
    assert!(!check(uploading, Some("locked"), GrantLevel::Manage).await);
}

// ---------------------------------------------------------------------------
// The visibility matrix
// ---------------------------------------------------------------------------

use axum::http::StatusCode;
use axum::Router;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::{AuthMode, Settings};
use serde_json::{json, Value};

const PW: &str = "matrixpassword";
const WRITE_TOKEN: &str = "tok_ci_write";
const ADMIN_TOKEN: &str = "tok_ops_admin";
const MCP: &str = "/api/v1/mcp";
const MCP_VERSION: &str = "2026-07-28";

/// The four runs the matrix is asserted over.
const R_LOCKED: &str = "run_locked";
const R_PRIVATE: &str = "run_private";
const R_PUBLIC: &str = "run_public";
const R_NULL: &str = "run_null";

/// Every fixture run carries this tag, so one search query reaches all four and
/// the filtering — not the query — decides what comes back.
const SHARED_TAG: &str = "gronkleberry";

struct Fixture {
    app: Router,
    _storage: Storage,
    _dir: TempDir,
    /// Session tokens, by role in the story.
    admin: String,
    /// Member holding `upload` on the whole `locked` project.
    grantee: String,
    /// Member with no grants at all.
    stranger: String,
    /// Viewer holding `view` on `open/private` only.
    watcher: String,
    /// The (hashed) key of the single case every fixture run carries.
    case_key: String,
}

async fn login(app: &Router, username: &str) -> String {
    let r = post_json(
        app,
        "/api/v1/auth/login",
        None,
        &json!({ "username": username, "password": PW }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "login {username}: {:?}", r.json());
    r.json()["token"].as_str().unwrap().to_string()
}

/// Seed the whole access-control story: two restricted sets (one whole project,
/// one suite inside an otherwise-open project), one open suite, one run with no
/// project at all, and four accounts spanning every grant shape.
async fn fixture() -> Fixture {
    let settings = Settings {
        tokens: Some(format!("write:{WRITE_TOKEN},admin:{ADMIN_TOKEN}")),
        mcp_enabled: Some(true),
        ..Default::default()
    };
    // `protect-writes`, so anonymous reads are legal and the `Public` class can
    // be exercised with no credential at all.
    let (app, storage, dir) = test_app_with_storage(settings, AuthMode::ProtectWrites).await;

    let setup = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": PW }),
    )
    .await;
    assert_eq!(setup.status, StatusCode::CREATED);
    let admin = setup.json()["token"].as_str().unwrap().to_string();

    for (username, role) in [
        ("grantee", "member"),
        ("stranger", "member"),
        ("watcher", "viewer"),
    ] {
        let created = post_json(
            &app,
            "/api/v1/users",
            Some(&admin),
            &json!({ "username": username, "password": PW, "role": role }),
        )
        .await;
        assert_eq!(created.status, StatusCode::CREATED, "creating {username}");
    }
    let grantee = login(&app, "grantee").await;
    let stranger = login(&app, "stranger").await;
    let watcher = login(&app, "watcher").await;

    let mut case_key = String::new();
    for (id, project, suite) in [
        (R_LOCKED, Some("locked"), Some("s1")),
        (R_PRIVATE, Some("open"), Some("private")),
        (R_PUBLIC, Some("open"), Some("public")),
        (R_NULL, None, None),
    ] {
        let run = make_run(
            id,
            project,
            suite,
            vec![SHARED_TAG],
            Some("main"),
            0,
            &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
        );
        case_key = run.cases[0].case_key.to_string();
        storage.ingest_run(run, Some("root".into())).await.unwrap();
    }

    storage
        .restrict_run_set("locked".into(), None, Some("root".into()))
        .await
        .unwrap();
    storage
        .restrict_run_set("open".into(), Some("private".into()), Some("root".into()))
        .await
        .unwrap();

    let id_of = |username: &str| {
        let storage = storage.clone();
        let username = username.to_string();
        async move {
            storage
                .get_user_by_username(username)
                .await
                .unwrap()
                .unwrap()
                .id
        }
    };
    storage
        .upsert_run_set_grant(
            "locked".into(),
            None,
            id_of("grantee").await,
            GrantLevel::Upload,
            Some("root".into()),
        )
        .await
        .unwrap();
    storage
        .upsert_run_set_grant(
            "open".into(),
            Some("private".into()),
            id_of("watcher").await,
            GrantLevel::View,
            Some("root".into()),
        )
        .await
        .unwrap();

    Fixture {
        app,
        _storage: storage,
        _dir: dir,
        admin,
        grantee,
        stranger,
        watcher,
        case_key,
    }
}

fn ids(value: &Value, path: &str) -> Vec<String> {
    let mut out: Vec<String> = value[path]
        .as_array()
        .unwrap_or_else(|| panic!("no array at '{path}' in {value}"))
        .iter()
        .map(|r| {
            r.get("id")
                .or_else(|| r.get("run_id"))
                .and_then(Value::as_str)
                .unwrap()
                .to_string()
        })
        .collect();
    out.sort();
    out
}

fn sorted(items: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = items.iter().map(|s| s.to_string()).collect();
    out.sort();
    out
}

/// Every caller class, and exactly which runs it may see. This is the contract;
/// the assertions below are all just projections of it.
fn matrix(f: &Fixture) -> Vec<(&'static str, Option<String>, Vec<String>)> {
    vec![
        (
            "admin session",
            Some(f.admin.clone()),
            sorted(&[R_LOCKED, R_PRIVATE, R_PUBLIC, R_NULL]),
        ),
        (
            "admin static token",
            Some(ADMIN_TOKEN.to_string()),
            sorted(&[R_LOCKED, R_PRIVATE, R_PUBLIC, R_NULL]),
        ),
        (
            "member with an upload grant on `locked`",
            Some(f.grantee.clone()),
            sorted(&[R_LOCKED, R_PUBLIC, R_NULL]),
        ),
        (
            "member with no grants",
            Some(f.stranger.clone()),
            sorted(&[R_PUBLIC, R_NULL]),
        ),
        (
            "viewer with a view grant on `open/private`",
            Some(f.watcher.clone()),
            sorted(&[R_PRIVATE, R_PUBLIC, R_NULL]),
        ),
        (
            "non-admin static token",
            Some(WRITE_TOKEN.to_string()),
            sorted(&[R_PUBLIC, R_NULL]),
        ),
        ("anonymous", None, sorted(&[R_PUBLIC, R_NULL])),
    ]
}

#[tokio::test]
async fn the_run_list_shows_exactly_what_each_caller_may_see() {
    let f = fixture().await;
    for (who, token, expected) in matrix(&f) {
        let r = get_auth(&f.app, "/api/v1/runs", token.as_deref()).await;
        assert_eq!(r.status, StatusCode::OK, "{who}");
        assert_eq!(ids(&r.json(), "runs"), expected, "GET /runs as {who}");
    }
}

#[tokio::test]
async fn an_invisible_run_is_a_404_on_every_child_endpoint() {
    let f = fixture().await;
    for (who, token, expected) in matrix(&f) {
        for run in [R_LOCKED, R_PRIVATE, R_PUBLIC, R_NULL] {
            let visible = expected.iter().any(|id| id == run);
            let want = if visible {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            for path in [
                format!("/api/v1/runs/{run}"),
                format!("/api/v1/runs/{run}/cases"),
                format!("/api/v1/runs/{run}/matrix"),
                format!("/api/v1/runs/{run}/export"),
                format!("/api/v1/runs/{run}/config"),
                format!("/api/v1/runs/{run}/compare/{R_PUBLIC}"),
            ] {
                let r = get_auth(&f.app, &path, token.as_deref()).await;
                assert_eq!(r.status, want, "{path} as {who}");
                if !visible {
                    // Indistinguishable from a run that never existed: a 403
                    // would confirm the id is real.
                    assert!(
                        r.json()["error"].as_str().unwrap().contains("not found"),
                        "{path} as {who}: {:?}",
                        r.json()
                    );
                }
            }
            // The case detail endpoint needs a real case key.
            let r = get_auth(
                &f.app,
                &format!("/api/v1/runs/{run}/cases/{}", f.case_key),
                token.as_deref(),
            )
            .await;
            assert_eq!(
                r.status == StatusCode::NOT_FOUND,
                !visible,
                "case detail on {run} as {who}"
            );
        }
    }
}

#[tokio::test]
async fn search_never_returns_a_hit_from_an_invisible_run() {
    let f = fixture().await;
    for (who, token, expected) in matrix(&f) {
        let r = get_auth(
            &f.app,
            &format!("/api/v1/search?q={SHARED_TAG}"),
            token.as_deref(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{who}");
        assert_eq!(ids(&r.json(), "runs"), expected, "search runs as {who}");
    }
}

#[tokio::test]
async fn the_project_and_suite_catalogs_omit_fully_invisible_sets() {
    let f = fixture().await;
    let projects = |token: Option<String>| {
        let app = f.app.clone();
        async move {
            let r = get_auth(&app, "/api/v1/projects", token.as_deref()).await;
            assert_eq!(r.status, StatusCode::OK);
            let mut names: Vec<String> = r.json()["projects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["project"].as_str().unwrap().to_string())
                .collect();
            names.sort();
            names
        }
    };
    assert_eq!(projects(Some(f.admin.clone())).await, ["locked", "open"]);
    assert_eq!(projects(Some(f.grantee.clone())).await, ["locked", "open"]);
    assert_eq!(projects(Some(f.stranger.clone())).await, ["open"]);
    assert_eq!(projects(Some(f.watcher.clone())).await, ["open"]);
    assert_eq!(projects(None).await, ["open"]);

    let suites = |token: Option<String>| {
        let app = f.app.clone();
        async move {
            let r = get_auth(&app, "/api/v1/projects/open/suites", token.as_deref()).await;
            assert_eq!(r.status, StatusCode::OK);
            let mut names: Vec<String> = r.json()["suites"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s["suite"].as_str().unwrap().to_string())
                .collect();
            names.sort();
            names
        }
    };
    assert_eq!(suites(Some(f.admin.clone())).await, ["private", "public"]);
    assert_eq!(suites(Some(f.watcher.clone())).await, ["private", "public"]);
    assert_eq!(suites(Some(f.stranger.clone())).await, ["public"]);
    assert_eq!(suites(None).await, ["public"]);
}

#[tokio::test]
async fn case_history_is_filtered_by_the_run_it_walks() {
    let f = fixture().await;
    let path = format!(
        "/api/v1/projects/open/suites/private/cases/{}/history",
        f.case_key
    );
    for (who, token, expected) in matrix(&f) {
        let r = get_auth(&f.app, &path, token.as_deref()).await;
        let want = if expected.iter().any(|id| id == R_PRIVATE) {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        assert_eq!(r.status, want, "case history as {who}");
    }
}

/// `find_runs` reaches the same storage as `GET /runs`, so it has to reach the
/// same answer — including via `group_by`, which is the projects catalog.
#[tokio::test]
async fn the_mcp_find_runs_tool_obeys_the_same_matrix() {
    let f = fixture().await;
    for (who, token, expected) in matrix(&f) {
        let headers: Vec<(String, String)> = {
            let mut h = vec![
                ("mcp-protocol-version".to_string(), MCP_VERSION.to_string()),
                ("mcp-method".to_string(), "tools/call".to_string()),
                ("mcp-name".to_string(), "find_runs".to_string()),
            ];
            if let Some(token) = &token {
                h.push(("authorization".to_string(), format!("Bearer {token}")));
            }
            h
        };
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": "find_runs",
                "arguments": { "limit": 50 },
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": MCP_VERSION,
                    "io.modelcontextprotocol/clientCapabilities": {},
                },
            }
        });
        let r = send_with_headers(
            &f.app,
            "POST",
            MCP,
            &borrowed,
            serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{who}: {:?}", r.json());
        let structured = &r.json()["result"]["structuredContent"];
        assert_eq!(ids(structured, "runs"), expected, "mcp find_runs as {who}");
    }
}

/// The `cached=exclude` count spans the whole filtered set rather than one
/// page, so it is a disclosure channel of its own: it must count only runs the
/// caller could otherwise have seen.
#[tokio::test]
async fn the_cached_hidden_count_never_counts_an_invisible_run() {
    let settings = Settings::default();
    let (app, storage, _dir) = test_app_with_storage(settings, AuthMode::ProtectWrites).await;

    let setup = post_json(
        &app,
        "/api/v1/auth/setup",
        None,
        &json!({ "username": "root", "password": PW }),
    )
    .await;
    let admin = setup.json()["token"].as_str().unwrap().to_string();

    // Two fully-cached passing runs — exactly what `cached=exclude` hides.
    for (id, project) in [("cached_locked", "locked"), ("cached_open", "open")] {
        let run = make_run(
            id,
            Some(project),
            Some("s1"),
            vec![],
            Some("main"),
            0,
            &[CaseSpec::new("openai", "t1", CaseStatus::Pass).cached(true)],
        );
        storage.ingest_run(run, None).await.unwrap();
    }
    storage
        .restrict_run_set("locked".into(), None, None)
        .await
        .unwrap();

    let hidden = |token: Option<String>| {
        let app = app.clone();
        async move {
            let r = get_auth(&app, "/api/v1/runs?cached=exclude", token.as_deref()).await;
            assert_eq!(r.status, StatusCode::OK);
            (
                r.json()["cached_hidden"].as_i64().unwrap(),
                r.json()["runs"].as_array().unwrap().len(),
            )
        }
    };
    assert_eq!(hidden(Some(admin)).await, (2, 0), "admin sees both hidden");
    assert_eq!(
        hidden(None).await,
        (1, 0),
        "an anonymous caller must not be told the restricted run exists"
    );
}

/// `/cache/entries/{key}/runs` names run ids by cache key. A restricted run
/// must not surface there either.
#[tokio::test]
async fn the_cache_entry_run_list_is_filtered_too() {
    let key = "sha256:".to_string() + &"ab".repeat(32);
    let (app, storage, _dir) = test_app_with_storage(Settings::default(), AuthMode::Open).await;

    for (id, project) in [("keyed_locked", "locked"), ("keyed_open", "open")] {
        let mut run = make_run(
            id,
            Some(project),
            Some("s1"),
            vec![],
            Some("main"),
            0,
            &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
        );
        run.cases[0].cache_key = Some(key.clone());
        storage.ingest_run(run, None).await.unwrap();
    }

    let listed = |app: Router, key: String| async move {
        let r = get(&app, &format!("/api/v1/cache/entries/{key}/runs")).await;
        assert_eq!(r.status, StatusCode::OK);
        ids(&r.json(), "cases")
    };
    assert_eq!(
        listed(app.clone(), key.clone()).await,
        sorted(&["keyed_locked", "keyed_open"]),
        "before any restriction, both runs are named"
    );

    storage
        .restrict_run_set("locked".into(), None, None)
        .await
        .unwrap();
    assert_eq!(listed(app, key).await, sorted(&["keyed_open"]));
}

// ---------------------------------------------------------------------------
// Writes into a restricted set
// ---------------------------------------------------------------------------

/// Uploading into `locked` (restricted, project-wide). The global `write` scope
/// is still required and unchanged; the grant check is an *additional* gate.
#[tokio::test]
async fn uploading_into_a_restricted_set_requires_an_upload_grant() {
    let f = fixture().await;

    let upload = |token: Option<String>, id: &'static str, project: &'static str| {
        let app = f.app.clone();
        async move {
            let run = make_run(
                id,
                Some(project),
                Some("s1"),
                vec![],
                Some("main"),
                1,
                &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
            );
            post_json(&app, "/api/v1/runs", token.as_deref(), &run_value(&run))
                .await
                .status
        }
    };

    // The member holding `upload` on the whole project may write into it.
    assert_eq!(
        upload(Some(f.grantee.clone()), "up_grantee", "locked").await,
        StatusCode::CREATED
    );
    // An admin is never gated.
    assert_eq!(
        upload(Some(f.admin.clone()), "up_admin", "locked").await,
        StatusCode::CREATED
    );
    assert_eq!(
        upload(Some(ADMIN_TOKEN.to_string()), "up_admin_tok", "locked").await,
        StatusCode::CREATED
    );

    // A member with no grant at all.
    assert_eq!(
        upload(Some(f.stranger.clone()), "up_stranger", "locked").await,
        StatusCode::FORBIDDEN
    );
    // A shared CI token carries `write` scope but holds no grants.
    assert_eq!(
        upload(Some(WRITE_TOKEN.to_string()), "up_ci", "locked").await,
        StatusCode::FORBIDDEN
    );

    // Unrestricted sets are untouched: the same tokens still upload freely.
    assert_eq!(
        upload(Some(WRITE_TOKEN.to_string()), "up_ci_open", "elsewhere").await,
        StatusCode::CREATED
    );
    assert_eq!(
        upload(Some(f.stranger.clone()), "up_stranger_open", "elsewhere").await,
        StatusCode::CREATED
    );
}

/// A `view` grant reads; it does not write. The level ordering has to hold at
/// the route, not just in the storage helper.
#[tokio::test]
async fn a_view_only_grant_cannot_upload_into_the_set_it_can_read() {
    let f = fixture().await;
    let run = make_run(
        "up_watcher",
        Some("open"),
        Some("private"),
        vec![],
        Some("main"),
        1,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    // `watcher` is a viewer, so it lacks `write` scope entirely — grant that
    // much by using a member who holds only `view` on the same set.
    let created = post_json(
        &f.app,
        "/api/v1/users",
        Some(&f.admin),
        &json!({ "username": "viewgrant", "password": PW, "role": "member" }),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);
    let user_id = created.json()["id"].as_str().unwrap().to_string();
    f._storage
        .upsert_run_set_grant(
            "open".into(),
            Some("private".into()),
            user_id.into(),
            GrantLevel::View,
            None,
        )
        .await
        .unwrap();
    let token = login(&f.app, "viewgrant").await;

    let r = post_json(&f.app, "/api/v1/runs", Some(&token), &run_value(&run)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{:?}", r.json());

    // The same caller reading that set is fine.
    let read = get_auth(&f.app, &format!("/api/v1/runs/{R_PRIVATE}"), Some(&token)).await;
    assert_eq!(read.status, StatusCode::OK);
}

/// Setting or clearing a suite's baseline is a write into that set, and obeys
/// the same gate as an upload.
#[tokio::test]
async fn setting_a_baseline_on_a_restricted_suite_needs_the_same_grant() {
    let f = fixture().await;
    let body = json!({ "run_id": R_LOCKED });
    let put = |token: Option<String>| {
        let app = f.app.clone();
        let body = body.clone();
        async move {
            send(
                &app,
                "PUT",
                "/api/v1/projects/locked/suites/s1/baseline",
                token.as_deref(),
                None,
                serde_json::to_vec(&body).unwrap(),
            )
            .await
            .status
        }
    };
    assert_eq!(put(Some(f.stranger.clone())).await, StatusCode::FORBIDDEN);
    assert_eq!(
        put(Some(WRITE_TOKEN.to_string())).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(put(Some(f.grantee.clone())).await, StatusCode::OK);
    assert_eq!(put(Some(f.admin.clone())).await, StatusCode::OK);

    let delete = |token: Option<String>| {
        let app = f.app.clone();
        async move {
            send(
                &app,
                "DELETE",
                "/api/v1/projects/locked/suites/s1/baseline",
                token.as_deref(),
                None,
                Vec::new(),
            )
            .await
            .status
        }
    };
    assert_eq!(
        delete(Some(f.stranger.clone())).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        delete(Some(f.grantee.clone())).await,
        StatusCode::NO_CONTENT
    );
}
