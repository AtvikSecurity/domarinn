//! Run-set policy storage: restrictions, per-user grants, and the `set_access`
//! decision the write paths and (in a later task) the access endpoints ask.
//!
//! The route-level consequences of these rules — who sees which run, and who
//! may write where — live in `runsets.rs`.

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

/// Managing a set's policy is never a default-open operation.
///
/// `view` and `upload` are free on an unrestricted set — that is what
/// default-open means, and it is why an anonymous upload in `open` mode still
/// works. `manage` is not: it is the power to *create* a restriction and hand
/// out grants, so bootstrapping it from nothing has to require an admin. A
/// covering `manage` grant may sit dormant on an unrestricted set and still
/// count.
#[tokio::test]
async fn managing_a_set_is_never_free_even_where_it_is_unrestricted() {
    let (storage, _dir) = storage().await;
    let manager = user(&storage, "manager", Role::Member).await;
    let uploader = user(&storage, "uploader", Role::Member).await;
    let nobody = user(&storage, "nobody", Role::Member).await;

    // Grants exist on `wide`, which is deliberately NOT restricted, and on
    // `narrow`, which is.
    storage
        .restrict_run_set("narrow".into(), None, None)
        .await
        .unwrap();
    for project in ["wide", "narrow"] {
        storage
            .upsert_run_set_grant(
                project.into(),
                None,
                manager.clone(),
                GrantLevel::Manage,
                None,
            )
            .await
            .unwrap();
        storage
            .upsert_run_set_grant(
                project.into(),
                None,
                uploader.clone(),
                GrantLevel::Upload,
                None,
            )
            .await
            .unwrap();
    }

    let can = |vis: RunVisibility, project: &'static str, needed| {
        let storage = storage.clone();
        async move {
            storage
                .set_access(vis, Some(project.into()), Some("s1".into()), needed)
                .await
                .unwrap()
        }
    };

    for project in ["wide", "narrow", "never-seen-before"] {
        assert!(
            !can(RunVisibility::Public, project, GrantLevel::Manage).await,
            "an anonymous caller must never manage '{project}'"
        );
        assert!(
            !can(
                RunVisibility::User(nobody.clone()),
                project,
                GrantLevel::Manage
            )
            .await,
            "a user with no grant must never manage '{project}'"
        );
        assert!(
            !can(
                RunVisibility::User(uploader.clone()),
                project,
                GrantLevel::Manage
            )
            .await,
            "an upload grant must not confer manage on '{project}'"
        );
        assert!(
            can(RunVisibility::Full, project, GrantLevel::Manage).await,
            "an admin manages '{project}'"
        );
    }

    // A covering manage grant counts wherever it is, restricted or not.
    for project in ["wide", "narrow"] {
        assert!(
            can(
                RunVisibility::User(manager.clone()),
                project,
                GrantLevel::Manage
            )
            .await,
            "a manage grant manages '{project}'"
        );
    }

    // Default-open is unchanged for the levels it was ever about.
    for needed in [GrantLevel::View, GrantLevel::Upload] {
        assert!(can(RunVisibility::Public, "wide", needed).await, "{needed}");
        assert!(
            can(RunVisibility::User(nobody.clone()), "wide", needed).await,
            "{needed}"
        );
    }
}
