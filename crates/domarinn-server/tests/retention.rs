//! Run retention: age-based eviction that never deletes something a user can
//! still reach.

mod common;

use axum::http::StatusCode;
use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::Settings;

/// `minute_offset` is minutes relative to the fixtures' *fixed* epoch
/// (2026-01-01), not to now — so every fixture run is already months old by
/// wall clock. Tests that want runs "inside the window" therefore pass a window
/// wide enough to cover that gap ([`WIDE_WINDOW_DAYS`]), rather than a small
/// number that would silently mean "everything is expired".
const DAY: i64 = 24 * 60;

/// A retention window comfortably wider than the fixture epoch's age.
const WIDE_WINDOW_DAYS: u64 = 36_500;

async fn seed(app: &axum::Router, id: &str, suite: &str, branch: &str, minute_offset: i64) {
    let run = make_run(
        id,
        Some("p"),
        Some(suite),
        vec![],
        Some(branch),
        minute_offset,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    let reply = post_json(app, "/api/v1/runs", None, &run_value(&run)).await;
    assert_eq!(reply.status, StatusCode::CREATED, "seed {id}");
}

async fn ids(app: &axum::Router) -> Vec<String> {
    get(app, "/api/v1/runs?limit=200").await.json()["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

/// The default. Eval history is expensive to produce and impossible to
/// recreate, so an upgrade must never start deleting it.
#[tokio::test]
async fn retention_is_off_unless_configured() {
    let (app, _dir) = test_app(Settings::default()).await;
    assert_eq!(Settings::default().run_max_age_days, None);
    seed(&app, "ancient", "s", "main", -3650 * DAY).await;
    seed(&app, "recent", "s", "main", 0).await;

    // Nothing sweeps on its own; the run stays until an operator opts in.
    assert_eq!(ids(&app).await.len(), 2);
}

/// The newest run of each (project, suite, branch) survives any age cutoff: it
/// is what the runs list and the status surface show. A suite that has not run
/// in a while must go stale, not vanish.
#[tokio::test]
async fn a_sweep_keeps_the_newest_run_of_every_branch() {
    let (app, dir) = test_app(Settings::default()).await;
    seed(&app, "main-old", "s", "main", -100 * DAY).await;
    seed(&app, "main-new", "s", "main", -90 * DAY).await;
    seed(&app, "feat-only", "s", "feat/x", -100 * DAY).await;

    let storage = domarinn_server::storage::Storage::open(dir.path().to_path_buf())
        .await
        .unwrap();
    let swept = storage.sweep_runs(30).await.unwrap();

    // Only `main-old` is evictable: the other two are each the newest of their
    // branch, even though both are far past the cutoff.
    assert_eq!(swept.deleted, 1);
    assert_eq!(
        swept.kept, 2,
        "protected runs are reported, not silently skipped"
    );

    let mut remaining = ids(&app).await;
    remaining.sort();
    assert_eq!(remaining, vec!["feat-only", "main-new"]);
}

/// A pinned baseline is what `--against server:baseline` resolves and what the
/// compare view diffs against. Evicting it turns a working CI gate into an
/// unresolvable-baseline error.
#[tokio::test]
async fn a_sweep_never_deletes_a_pinned_baseline() {
    let (app, dir) = test_app(Settings::default()).await;
    seed(&app, "pinned", "s", "main", -100 * DAY).await;
    seed(&app, "middle", "s", "main", -95 * DAY).await;
    seed(&app, "newest", "s", "main", -90 * DAY).await;

    let reply = send(
        &app,
        "PUT",
        "/api/v1/projects/p/suites/s/baseline",
        None,
        None,
        serde_json::to_vec(&serde_json::json!({ "run_id": "pinned" })).unwrap(),
    )
    .await;
    assert!(
        reply.status.is_success(),
        "pin baseline: {:?}",
        reply.status
    );

    let storage = domarinn_server::storage::Storage::open(dir.path().to_path_buf())
        .await
        .unwrap();
    let swept = storage.sweep_runs(30).await.unwrap();

    assert_eq!(swept.deleted, 1, "only `middle` is evictable");
    let mut remaining = ids(&app).await;
    remaining.sort();
    assert_eq!(remaining, vec!["newest", "pinned"]);
}

/// Runs inside the window are untouched no matter how many there are.
#[tokio::test]
async fn a_sweep_leaves_runs_inside_the_window_alone() {
    let (app, dir) = test_app(Settings::default()).await;
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        seed(&app, id, "s", "main", -(i as i64) * DAY).await;
    }

    let storage = domarinn_server::storage::Storage::open(dir.path().to_path_buf())
        .await
        .unwrap();
    let swept = storage.sweep_runs(WIDE_WINDOW_DAYS).await.unwrap();
    assert_eq!(swept, domarinn_server::storage::retention::Swept::default());
    assert_eq!(ids(&app).await.len(), 3);
}

/// FTS tables have no foreign key, so a swept run has to be removed from them
/// explicitly or its text stays searchable forever — the same trap `delete_run`
/// already documents.
#[tokio::test]
async fn a_sweep_removes_the_runs_search_rows() {
    let (app, dir) = test_app(Settings::default()).await;
    let mut old = make_run(
        "searchable",
        Some("p"),
        Some("s"),
        vec!["zorblatt"],
        Some("main"),
        -100 * DAY,
        &[CaseSpec::new("openai", "t1", CaseStatus::Pass)],
    );
    old.origin = Some(domarinn_core::result::RunOrigin {
        note: Some("quibblewort investigation".into()),
        ..Default::default()
    });
    post_json(&app, "/api/v1/runs", None, &run_value(&old)).await;
    // A newer run on the same branch, so the old one is actually evictable.
    seed(&app, "keeper", "s", "main", -90 * DAY).await;

    assert_eq!(
        get(&app, "/api/v1/search?q=quibblewort").await.json()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "precondition: the note is searchable before the sweep"
    );

    let storage = domarinn_server::storage::Storage::open(dir.path().to_path_buf())
        .await
        .unwrap();
    assert_eq!(storage.sweep_runs(30).await.unwrap().deleted, 1);

    assert!(
        get(&app, "/api/v1/search?q=quibblewort").await.json()["runs"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a swept run must not stay searchable"
    );
}
