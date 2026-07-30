//! The `/api/v1/sets*` surface: the run-set browser and its access lists.
//!
//! The visibility rules these endpoints inherit are proven in `runsets.rs`; the
//! policy store beneath them in `runset_policy.rs`. What is asserted here is the
//! HTTP contract — which caller class gets which body, and which status a
//! refusal carries.

mod common;

use axum::http::StatusCode;
use axum::Router;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::runsets::GrantLevel;
use domarinn_server::storage::Storage;
use domarinn_server::{AuthMode, Settings};
use serde_json::{json, Value};
use tempfile::TempDir;

const PW: &str = "setspassword";
const ADMIN_TOKEN: &str = "tok_ops_admin";

struct Fixture {
    app: Router,
    storage: Storage,
    _dir: TempDir,
    /// The bootstrap admin's session.
    admin: String,
    /// Member holding `manage` on the whole `locked` project.
    manager: String,
    /// Member holding only `view` on `locked`.
    viewer_grant: String,
    /// Member with no grants at all.
    stranger: String,
    /// Viewer-role account with no grants.
    watcher: String,
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

async fn user_id(storage: &Storage, username: &str) -> String {
    storage
        .get_user_by_username(username.to_string())
        .await
        .unwrap()
        .unwrap()
        .id
        .as_str()
        .to_string()
}

/// Two projects: `open` (unrestricted, two suites with deliberately distinct
/// pass rates) and `locked` (restricted project-wide, one suite).
async fn fixture() -> Fixture {
    let settings = Settings {
        tokens: Some(format!("admin:{ADMIN_TOKEN}")),
        ..Default::default()
    };
    // `protect-writes` so anonymous reads are legal and the `Public` class can
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
        ("manager", "member"),
        ("viewgrant", "member"),
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

    // `open/alpha`: three runs whose pass rates are 0.5, 1.0, 0.0 in time order.
    for (id, offset, specs) in [
        (
            "a1",
            0,
            vec![
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Fail),
            ],
        ),
        (
            "a2",
            10,
            vec![
                CaseSpec::new("openai", "t1", CaseStatus::Pass),
                CaseSpec::new("openai", "t2", CaseStatus::Pass),
            ],
        ),
        (
            "a3",
            20,
            vec![
                CaseSpec::new("openai", "t1", CaseStatus::Fail),
                CaseSpec::new("openai", "t2", CaseStatus::Fail),
            ],
        ),
    ] {
        let run = make_run(
            id,
            Some("open"),
            Some("alpha"),
            vec![],
            None,
            offset,
            &specs,
        );
        storage.ingest_run(run, Some("root".into())).await.unwrap();
    }
    let beta = make_run(
        "b1",
        Some("open"),
        Some("beta"),
        vec![],
        None,
        5,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    storage.ingest_run(beta, Some("root".into())).await.unwrap();
    let locked = make_run(
        "l1",
        Some("locked"),
        Some("s1"),
        vec![],
        None,
        0,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    storage
        .ingest_run(locked, Some("root".into()))
        .await
        .unwrap();

    storage
        .set_baseline(
            "open".into(),
            "beta".into(),
            "b1".into(),
            domarinn_server::runsets::RunVisibility::Full,
        )
        .await
        .unwrap();

    storage
        .restrict_run_set("locked".into(), None, Some("root".into()))
        .await
        .unwrap();
    for (username, level) in [
        ("manager", GrantLevel::Manage),
        ("viewgrant", GrantLevel::View),
    ] {
        let id = user_id(&storage, username).await;
        storage
            .upsert_run_set_grant("locked".into(), None, id.into(), level, Some("root".into()))
            .await
            .unwrap();
    }

    let manager = login(&app, "manager").await;
    let viewer_grant = login(&app, "viewgrant").await;
    let stranger = login(&app, "stranger").await;
    let watcher = login(&app, "watcher").await;

    Fixture {
        app,
        storage,
        _dir: dir,
        admin,
        manager,
        viewer_grant,
        stranger,
        watcher,
    }
}

/// The `projects` array of `GET /sets`, keyed by project name.
async fn sets(app: &Router, token: Option<&str>) -> Vec<Value> {
    let r = get_auth(app, "/api/v1/sets", token).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    r.json()["projects"].as_array().unwrap().clone()
}

fn names(projects: &[Value]) -> Vec<String> {
    projects
        .iter()
        .map(|p| p["project"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(projects: &'a [Value], project: &str) -> &'a Value {
    projects
        .iter()
        .find(|p| p["project"] == project)
        .unwrap_or_else(|| panic!("no project '{project}' in {projects:?}"))
}

// ---------------------------------------------------------------------------
// Read surface
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_sets_listing_shows_each_caller_only_the_sets_they_may_see() {
    let f = fixture().await;

    // Sorted by project name, ascending.
    assert_eq!(
        names(&sets(&f.app, Some(&f.admin)).await),
        ["locked", "open"]
    );
    assert_eq!(
        names(&sets(&f.app, Some(ADMIN_TOKEN)).await),
        ["locked", "open"]
    );
    assert_eq!(
        names(&sets(&f.app, Some(&f.manager)).await),
        ["locked", "open"]
    );
    assert_eq!(
        names(&sets(&f.app, Some(&f.viewer_grant)).await),
        ["locked", "open"]
    );
    // No grant: the restricted project is omitted entirely, not zeroed out.
    assert_eq!(names(&sets(&f.app, Some(&f.stranger)).await), ["open"]);
    assert_eq!(names(&sets(&f.app, Some(&f.watcher)).await), ["open"]);
    assert_eq!(names(&sets(&f.app, None).await), ["open"]);
}

#[tokio::test]
async fn the_listing_reports_restriction_and_the_callers_own_level() {
    let f = fixture().await;

    // Admins ride no grants, so they are told about none.
    let admin = sets(&f.app, Some(&f.admin)).await;
    assert_eq!(find(&admin, "locked")["restricted"], json!(true));
    assert_eq!(find(&admin, "locked")["my_level"], Value::Null);
    assert_eq!(find(&admin, "open")["restricted"], json!(false));

    let manager = sets(&f.app, Some(&f.manager)).await;
    assert_eq!(find(&manager, "locked")["my_level"], json!("manage"));
    // Ungranted *and* unrestricted is null, not a synthesized level.
    assert_eq!(find(&manager, "open")["my_level"], Value::Null);

    let viewer = sets(&f.app, Some(&f.viewer_grant)).await;
    assert_eq!(find(&viewer, "locked")["my_level"], json!("view"));

    assert_eq!(
        find(&sets(&f.app, None).await, "open")["my_level"],
        Value::Null
    );
}

#[tokio::test]
async fn the_listing_aggregates_over_every_visible_run_of_the_project() {
    let f = fixture().await;
    let projects = sets(&f.app, Some(&f.admin)).await;
    let open = find(&projects, "open");

    assert_eq!(open["run_count"], json!(4));
    assert_eq!(open["suite_count"], json!(2));
    assert_eq!(open["case_count"], json!(7));
    assert_eq!(open["pass_count"], json!(4));
    assert_eq!(open["fail_count"], json!(3));
    assert_eq!(open["error_count"], json!(0));
    assert!(
        open["last_run_at"].as_i64().is_some(),
        "last_run_at is epoch-ms: {open:?}"
    );
    // One latest pass rate per suite, suites in name order: alpha then beta.
    assert_eq!(open["recent_pass_rates"], json!([0.0, 1.0]));
}

#[tokio::test]
async fn a_project_detail_lists_its_suites_with_an_oldest_first_sparkline() {
    let f = fixture().await;
    let r = get_auth(&f.app, "/api/v1/sets/open", Some(&f.admin)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let body = r.json();
    assert_eq!(body["project"], "open");
    assert_eq!(body["restricted"], json!(false));

    let suites = body["suites"].as_array().unwrap();
    assert_eq!(
        suites
            .iter()
            .map(|s| s["suite"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );

    let alpha = &suites[0];
    assert_eq!(alpha["run_count"], json!(3));
    assert_eq!(alpha["case_count"], json!(6));
    assert_eq!(alpha["pass_count"], json!(3));
    assert_eq!(alpha["fail_count"], json!(3));
    // Oldest -> newest, which is what the web Sparkline consumes.
    assert_eq!(alpha["sparkline"], json!([0.5, 1.0, 0.0]));
    assert_eq!(alpha["latest_pass_rate"], json!(0.0));
    assert_eq!(alpha["baseline_run_id"], Value::Null);
    assert_eq!(alpha["restricted"], json!(false));

    let beta = &suites[1];
    assert_eq!(beta["baseline_run_id"], json!("b1"));
    assert_eq!(beta["sparkline"], json!([1.0]));
}

#[tokio::test]
async fn an_invisible_project_or_suite_is_a_404_not_an_empty_body() {
    let f = fixture().await;
    for (who, token) in [
        ("stranger", Some(f.stranger.clone())),
        ("watcher", Some(f.watcher.clone())),
        ("anonymous", None),
    ] {
        for path in ["/api/v1/sets/locked", "/api/v1/sets/locked/suites/s1"] {
            let r = get_auth(&f.app, path, token.as_deref()).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{path} as {who}");
        }
    }
    // A project that never existed answers identically.
    let r = get_auth(&f.app, "/api/v1/sets/nope", Some(&f.admin)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = get_auth(&f.app, "/api/v1/sets/open/suites/nope", Some(&f.admin)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // The grant holders do see it.
    for token in [&f.admin, &f.manager, &f.viewer_grant] {
        let r = get_auth(&f.app, "/api/v1/sets/locked", Some(token)).await;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
        assert_eq!(r.json()["restricted"], json!(true));
    }
}

#[tokio::test]
async fn a_suite_detail_carries_the_same_aggregates_as_its_row() {
    let f = fixture().await;
    let r = get_auth(&f.app, "/api/v1/sets/open/suites/alpha", Some(&f.admin)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    let body = r.json();
    assert_eq!(body["project"], "open");
    assert_eq!(body["suite"], "alpha");
    assert_eq!(body["run_count"], json!(3));
    assert_eq!(body["case_count"], json!(6));
    assert_eq!(body["sparkline"], json!([0.5, 1.0, 0.0]));
    assert_eq!(body["latest_pass_rate"], json!(0.0));
    assert_eq!(body["baseline_run_id"], Value::Null);
    assert_eq!(body["restricted"], json!(false));
    assert_eq!(body["my_level"], Value::Null);

    // A suite inside a restricted project reports the covering restriction.
    let r = get_auth(&f.app, "/api/v1/sets/locked/suites/s1", Some(&f.manager)).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["restricted"], json!(true));
    assert_eq!(r.json()["my_level"], json!("manage"));
}

// ---------------------------------------------------------------------------
// Access lists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_access_list_is_readable_only_by_admins_and_managers() {
    let f = fixture().await;
    let path = "/api/v1/sets/locked/access";

    for (who, token) in [
        ("admin session", Some(f.admin.clone())),
        ("admin token", Some(ADMIN_TOKEN.to_string())),
        ("manager", Some(f.manager.clone())),
    ] {
        let r = get_auth(&f.app, path, token.as_deref()).await;
        assert_eq!(r.status, StatusCode::OK, "{who}: {:?}", r.json());
        assert_eq!(r.json()["restricted"], json!(true));
        let mut holders: Vec<String> = r.json()["grants"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["username"].as_str().unwrap().to_string())
            .collect();
        holders.sort();
        assert_eq!(holders, ["manager", "viewgrant"], "{who}");
    }

    // A `view` grant reads runs, not the access list — and the refusal is a
    // 404, so it discloses nothing a manage-less caller did not already know.
    for (who, token) in [
        ("view grant holder", Some(f.viewer_grant.clone())),
        ("ungranted member", Some(f.stranger.clone())),
        ("viewer role", Some(f.watcher.clone())),
        ("anonymous", None),
    ] {
        let r = get_auth(&f.app, path, token.as_deref()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{who}: {:?}", r.json());
    }

    // The same holds for an *unrestricted* set: the default-open waiver stops
    // at upload, so nobody manages `open` without an explicit grant.
    let r = get_auth(&f.app, "/api/v1/sets/open/access", Some(&f.stranger)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{:?}", r.json());
    let r = get_auth(&f.app, "/api/v1/sets/open/access", Some(&f.admin)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["restricted"], json!(false));
    assert_eq!(r.json()["grants"], json!([]));
}

/// `restricted` on the access list is the **exact-scope** answer, because it
/// describes the one row the toggle beside it creates and deletes. The browse
/// views answer the covering question instead — a suite inside a restricted
/// project is unreachable there, but its restriction is not its own to remove.
#[tokio::test]
async fn the_access_list_reports_the_restriction_its_toggle_owns() {
    let f = fixture().await;
    assert_eq!(
        restriction(
            &f.app,
            "PUT",
            "/api/v1/sets/open/restriction",
            Some(&f.admin)
        )
        .await,
        StatusCode::NO_CONTENT
    );

    let project = get_auth(&f.app, "/api/v1/sets/open/access", Some(&f.admin)).await;
    assert_eq!(project.json()["restricted"], json!(true));

    let suite = get_auth(
        &f.app,
        "/api/v1/sets/open/suites/alpha/access",
        Some(&f.admin),
    )
    .await;
    assert_eq!(suite.status, StatusCode::OK, "{:?}", suite.json());
    assert_eq!(
        suite.json()["restricted"],
        json!(false),
        "the suite carries no restriction row of its own"
    );

    // The browse view still tells the truth about reachability.
    let browsed = get_auth(&f.app, "/api/v1/sets/open/suites/alpha", Some(&f.admin)).await;
    assert_eq!(browsed.json()["restricted"], json!(true));
}

#[tokio::test]
async fn a_grant_row_carries_the_fields_the_access_panel_renders() {
    let f = fixture().await;
    let r = get_auth(&f.app, "/api/v1/sets/locked/access", Some(&f.admin)).await;
    let grant = r.json()["grants"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["username"] == "manager")
        .cloned()
        .unwrap();
    assert_eq!(grant["level"], json!("manage"));
    assert_eq!(grant["created_by"], json!("root"));
    assert!(grant["created_at"].as_i64().is_some(), "{grant:?}");
    assert_eq!(
        grant["user_id"].as_str().unwrap(),
        user_id(&f.storage, "manager").await
    );
}

/// A dormant `manage` grant on an unrestricted set still opens the panel — the
/// access list is how a set gets pre-provisioned before it is ever locked.
#[tokio::test]
async fn a_dormant_manage_grant_opens_an_unrestricted_sets_access_list() {
    let f = fixture().await;
    let id = user_id(&f.storage, "stranger").await;
    f.storage
        .upsert_run_set_grant(
            "open".into(),
            Some("alpha".into()),
            id.into(),
            GrantLevel::Manage,
            Some("root".into()),
        )
        .await
        .unwrap();

    let r = get_auth(
        &f.app,
        "/api/v1/sets/open/suites/alpha/access",
        Some(&f.stranger),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["restricted"], json!(false));
    // The project-level list stays closed: the grant covers the suite only.
    let r = get_auth(&f.app, "/api/v1/sets/open/access", Some(&f.stranger)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Restriction toggling
// ---------------------------------------------------------------------------

async fn restriction(app: &Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
    send(app, method, path, token, None, b"{}".to_vec())
        .await
        .status
}

#[tokio::test]
async fn only_an_admin_may_toggle_a_restriction() {
    let f = fixture().await;
    let path = "/api/v1/sets/open/suites/alpha/restriction";

    // A manager can read the panel but not throw the switch.
    let id = user_id(&f.storage, "manager").await;
    f.storage
        .upsert_run_set_grant(
            "open".into(),
            Some("alpha".into()),
            id.into(),
            GrantLevel::Manage,
            Some("root".into()),
        )
        .await
        .unwrap();
    let r = get_auth(
        &f.app,
        "/api/v1/sets/open/suites/alpha/access",
        Some(&f.manager),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    assert_eq!(
        restriction(&f.app, "PUT", path, Some(&f.manager)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        restriction(&f.app, "PUT", path, Some(&f.stranger)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        restriction(&f.app, "PUT", path, None).await,
        StatusCode::UNAUTHORIZED
    );

    assert_eq!(
        restriction(&f.app, "PUT", path, Some(&f.admin)).await,
        StatusCode::NO_CONTENT
    );
    // Idempotent: a second create is still a success.
    assert_eq!(
        restriction(&f.app, "PUT", path, Some(&f.admin)).await,
        StatusCode::NO_CONTENT
    );
    assert!(f
        .storage
        .run_set_restricted(Some("open".into()), Some("alpha".into()))
        .await
        .unwrap());

    assert_eq!(
        restriction(&f.app, "DELETE", path, Some(&f.manager)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        restriction(&f.app, "DELETE", path, Some(&f.admin)).await,
        StatusCode::NO_CONTENT
    );
    // Nothing left to remove.
    assert_eq!(
        restriction(&f.app, "DELETE", path, Some(&f.admin)).await,
        StatusCode::NOT_FOUND
    );
}

/// Restricting a suite immediately narrows what the run reads return — the
/// endpoint writes the same table the visibility predicate joins.
#[tokio::test]
async fn restricting_a_suite_takes_effect_on_the_next_read() {
    let f = fixture().await;
    assert_eq!(
        get_auth(&f.app, "/api/v1/sets/open/suites/beta", None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        restriction(
            &f.app,
            "PUT",
            "/api/v1/sets/open/suites/beta/restriction",
            Some(&f.admin)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        get_auth(&f.app, "/api/v1/sets/open/suites/beta", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    let open = find(&sets(&f.app, None).await, "open").clone();
    assert_eq!(open["suite_count"], json!(1));
    assert_eq!(open["run_count"], json!(3));
}

// ---------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------

async fn put_grant(
    app: &Router,
    path: &str,
    token: Option<&str>,
    level: GrantLevel,
) -> (StatusCode, Value) {
    let body = serde_json::to_vec(&json!({ "level": level.as_str() })).unwrap();
    let r = send(app, "PUT", path, token, None, body).await;
    (r.status, r.json())
}

#[tokio::test]
async fn a_manager_may_add_relevel_and_remove_grants() {
    let f = fixture().await;
    let target = user_id(&f.storage, "stranger").await;
    let path = format!("/api/v1/sets/locked/grants/{target}");

    let (status, body) = put_grant(&f.app, &path, Some(&f.manager), GrantLevel::View).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");
    assert_eq!(
        f.storage
            .run_set_grant_level(Some("locked".into()), None, target.clone().into())
            .await
            .unwrap(),
        Some(GrantLevel::View)
    );

    // Upsert, not insert: the same call re-levels.
    let (status, _) = put_grant(&f.app, &path, Some(&f.manager), GrantLevel::Upload).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        f.storage
            .run_set_grant_level(Some("locked".into()), None, target.clone().into())
            .await
            .unwrap(),
        Some(GrantLevel::Upload)
    );

    // The new holder can now see the set.
    assert_eq!(
        names(&sets(&f.app, Some(&f.stranger)).await),
        ["locked", "open"]
    );

    let r = send(&f.app, "DELETE", &path, Some(&f.manager), None, Vec::new()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = send(&f.app, "DELETE", &path, Some(&f.manager), None, Vec::new()).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "nothing left to remove");
    assert_eq!(names(&sets(&f.app, Some(&f.stranger)).await), ["open"]);
}

#[tokio::test]
async fn a_non_manager_cannot_grant_itself_anything() {
    let f = fixture().await;
    let stranger_id = user_id(&f.storage, "stranger").await;
    let path = format!("/api/v1/sets/locked/grants/{stranger_id}");

    for (who, token) in [
        ("ungranted member", Some(f.stranger.clone())),
        ("view grant holder", Some(f.viewer_grant.clone())),
        ("anonymous", None),
    ] {
        let (status, body) = put_grant(&f.app, &path, token.as_deref(), GrantLevel::Manage).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{who}: {body:?}");
    }
    assert_eq!(
        f.storage
            .run_set_grant_level(Some("locked".into()), None, stranger_id.into())
            .await
            .unwrap(),
        None
    );

    // And not on the wide-open project either — the waiver stops at upload.
    let manager_id = user_id(&f.storage, "manager").await;
    let (status, _) = put_grant(
        &f.app,
        &format!("/api/v1/sets/open/grants/{manager_id}"),
        Some(&f.stranger),
        GrantLevel::Manage,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn granting_to_an_unknown_user_is_a_404() {
    let f = fixture().await;
    let (status, body) = put_grant(
        &f.app,
        "/api/v1/sets/locked/grants/usr_nobody",
        Some(&f.admin),
        GrantLevel::View,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body:?}");
    assert!(
        body["error"].as_str().unwrap().contains("user"),
        "the refusal names the missing user: {body:?}"
    );
}

/// Access may be provisioned before the first upload, so neither endpoint may
/// require the set to exist in `runs`.
#[tokio::test]
async fn grants_and_restrictions_work_on_a_set_with_no_runs() {
    let f = fixture().await;
    let target = user_id(&f.storage, "stranger").await;

    assert_eq!(
        restriction(
            &f.app,
            "PUT",
            "/api/v1/sets/unborn/suites/nightly/restriction",
            Some(&f.admin)
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = put_grant(
        &f.app,
        &format!("/api/v1/sets/unborn/suites/nightly/grants/{target}"),
        Some(&f.admin),
        GrantLevel::Upload,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body:?}");

    let r = get_auth(
        &f.app,
        "/api/v1/sets/unborn/suites/nightly/access",
        Some(&f.admin),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.json());
    assert_eq!(r.json()["restricted"], json!(true));
    assert_eq!(r.json()["grants"].as_array().unwrap().len(), 1);

    // The browser itself is still runs-derived: nothing to show yet.
    assert_eq!(
        get_auth(&f.app, "/api/v1/sets/unborn", Some(&f.admin))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

/// A suite-scoped grant is stored and listed at exactly that scope; the
/// project-level list is a different set.
#[tokio::test]
async fn suite_scoped_grants_are_separate_from_project_scoped_ones() {
    let f = fixture().await;
    let target = user_id(&f.storage, "stranger").await;

    let (status, _) = put_grant(
        &f.app,
        &format!("/api/v1/sets/open/suites/alpha/grants/{target}"),
        Some(&f.admin),
        GrantLevel::View,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let suite = get_auth(
        &f.app,
        "/api/v1/sets/open/suites/alpha/access",
        Some(&f.admin),
    )
    .await;
    assert_eq!(suite.json()["grants"].as_array().unwrap().len(), 1);
    let project = get_auth(&f.app, "/api/v1/sets/open/access", Some(&f.admin)).await;
    assert_eq!(project.json()["grants"], json!([]));
}
