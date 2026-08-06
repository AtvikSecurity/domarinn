//! E2E tests for the `cache` subcommands: stats, path, gc, clear.
//!
//! Most of these turn on the *legacy tier* — the cwd-relative `.domarinn/cache`
//! a pre-0.4 run wrote, which a run today layers under the suite's own cache as
//! a read-only tier. Reporting it and purging it are deliberately different
//! questions, so the fixtures build both tiers for real rather than asserting
//! on paths.

mod common;

use assert_cmd::prelude::*;
use common::{bin, suite};
use predicates::prelude::*;

#[test]
fn cache_path_and_stats() {
    let dir = tempfile::tempdir().unwrap();
    // An empty directory has no legacy tier, so both commands report one tier.
    bin()
        .args(["cache", "path"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy tier").not());
    bin()
        .args(["cache", "stats"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::starts_with("0 entries"))
        .stdout(predicate::str::contains("legacy tier").not());
}

/// The whole point of `gc` is a bound; without one it used to fall through to
/// an unbounded purge, so `domarinn cache gc` silently did what `cache clear`
/// does. The message has to name `clear`, because the operator who typed this
/// wanted one of the two and the gentler-sounding verb is the wrong one.
#[test]
fn cache_gc_without_any_bound_refuses_and_names_clear() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let seeded = cache_stats(dir.path());
    assert!(
        seeded.starts_with("1 entries"),
        "the run should have seeded exactly one entry; got: {seeded}"
    );

    bin()
        .args(["cache", "gc"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--older-than 30d"))
        .stderr(predicate::str::contains("cache clear"));

    assert_eq!(
        cache_stats(dir.path()),
        seeded,
        "a refused gc must not remove anything"
    );
}

/// `--newer-than` on its own reads "delete everything since", which is a
/// whole-store wipe spelled as housekeeping. It is only ever the recent end of
/// a window, so the destructive reading is refused rather than merely
/// discouraged.
#[test]
fn cache_gc_newer_than_without_older_than_refuses() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("domarinn.yaml"), suite("hello", "hello")).unwrap();
    bin().arg("run").current_dir(dir.path()).assert().success();
    let seeded = cache_stats(dir.path());

    bin()
        .args(["cache", "gc", "--newer-than", "1h"])
        .current_dir(dir.path())
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--older-than"))
        .stderr(predicate::str::contains("cache clear"));

    assert_eq!(
        cache_stats(dir.path()),
        seeded,
        "a refused gc must not remove anything"
    );

    // The window form is accepted — the entry is younger than 30d, so nothing
    // matches and nothing goes.
    bin()
        .args(["cache", "gc", "--older-than", "30d", "--newer-than", "90d"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 0 of 1 cache entry"));
    assert_eq!(cache_stats(dir.path()), seeded);
}

/// Targeted eviction: the poisoned entry goes and the warm cache around it
/// stays. The `removed N of M` denominator is what makes that claim checkable —
/// a bare count cannot distinguish "removed the one bad entry" from "removed
/// the only entry there was".
#[test]
fn cache_gc_by_empty_reason_removes_only_matching() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".domarinn/cache");
    seed_entry(&root, 'a', Some("refusal"));
    seed_entry(&root, 'b', Some("blank"));
    seed_entry(&root, 'c', None);
    assert_eq!(entry_count(&root), 3);

    bin()
        .args(["cache", "gc", "--empty-reason", "refusal"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 1 of 3 cache entries"));

    assert_eq!(
        entry_count(&root),
        2,
        "only the refusal should have been evicted"
    );

    // Repeatable, and an unknown reason is still evictable — `EmptyReason` is
    // an open type and this must not become the place that closes it.
    seed_entry(&root, 'd', Some("invented_next_year"));
    bin()
        .args([
            "cache",
            "gc",
            "--empty-reason",
            "blank",
            "--empty-reason",
            "invented_next_year",
        ])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 2 of 3 cache entries"));
    assert_eq!(
        entry_count(&root),
        1,
        "the entry with no reason at all must survive"
    );
}

/// The single most important invariant on this side of the change. `clear` is a
/// literally-default filter, which is the only thing a backend reads as
/// "everything" — fold an empty `--empty-reason` vec into the match as a plain
/// conjunction and this becomes a silent no-op that prints "cleared 0".
#[test]
fn cache_clear_still_removes_everything() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join(".domarinn/cache");
    seed_entry(&root, 'a', Some("refusal"));
    seed_entry(&root, 'b', None);
    seed_entry(&root, 'c', Some("truncated"));

    bin()
        .args(["cache", "clear"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("cleared 3 cache entries"));

    assert_eq!(entry_count(&root), 0);
}

/// Write one cache entry file directly, so a test can pin a body predicate
/// without needing a provider that actually refuses. `fill` picks the key, so
/// each call lands in its own shard.
fn seed_entry(root: &std::path::Path, fill: char, empty_reason: Option<&str>) {
    let hex: String = std::iter::repeat_n(fill, 64).collect();
    let shard = root.join(&hex[..2]);
    std::fs::create_dir_all(&shard).unwrap();
    let mut entry = serde_json::json!({
        "created_at": "2026-01-01T00:00:00Z",
        "output": "",
        "domarinn_version": "test",
    });
    if let Some(reason) = empty_reason {
        entry["empty_reason"] = serde_json::json!(reason);
    }
    std::fs::write(shard.join(format!("{hex}.json")), entry.to_string()).unwrap();
}

/// A run layers a cwd-relative `.domarinn/cache` under the suite's own cache as
/// a read-only tier, so `clear` has to remove it too — otherwise the next run
/// re-adopts everything the operator just cleared.
#[test]
fn cache_clear_purges_the_legacy_cwd_relative_tier_too() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy_and_suite_caches(dir.path());

    bin()
        .args(["cache", "clear", "evals"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("cleared 1 cache entry"))
        .stdout(predicate::str::contains("legacy"));

    assert_eq!(
        entry_count(&dir.path().join(".domarinn/cache")),
        0,
        "the legacy tier must be emptied, not just reported"
    );
    assert_eq!(entry_count(&dir.path().join("evals/.domarinn/cache")), 0);
}

/// A `gc` that collected only the primary tier would leave behind exactly the
/// stale entries it was asked for, and the next run would copy them forward.
#[test]
fn cache_gc_applies_the_age_filter_to_the_legacy_tier() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy_and_suite_caches(dir.path());

    // Both tiers are considered; nothing in either is 30 days old.
    bin()
        .args(["cache", "gc", "--older-than", "30d", "evals"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 0 of 1 cache entry"))
        .stdout(predicate::str::contains(
            "removed 0 of 1 legacy cache entry",
        ));
    assert_eq!(entry_count(&dir.path().join(".domarinn/cache")), 1);
    assert_eq!(entry_count(&dir.path().join("evals/.domarinn/cache")), 1);

    // `0s` puts the cutoff at now, so every entry written earlier is older.
    bin()
        .args(["cache", "gc", "--older-than", "0s", "evals"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 1 of 1 cache entry"))
        .stdout(predicate::str::contains(
            "removed 1 of 1 legacy cache entry",
        ));
    assert_eq!(entry_count(&dir.path().join(".domarinn/cache")), 0);
    assert_eq!(entry_count(&dir.path().join("evals/.domarinn/cache")), 0);
}

/// The legacy tier gets the **same** filter as the primary, `--newer-than`
/// included.
///
/// This test previously pinned the opposite. The reasoning was that the legacy
/// tier is a pre-0.4 leftover holding nothing recent except this project's own
/// entries, and that `cwd_contains`'s rule for when it is safe to purge was
/// written assuming a `gc` removes *old* things — so handing it "delete the
/// recent half" would turn a guard that spares a stranger's cache into one that
/// empties your own.
///
/// That shape is unconstructible: the CLI refuses `--newer-than` without
/// `--older-than`, so the only reachable form is a *window*. Dropping the recent
/// bound could therefore only ever **widen** the deletion — `--older-than 30d
/// --newer-than 90d` asks for the 30–90-day band and would have taken everything
/// older than 30 days from the legacy tier, including entries the operator had
/// explicitly bounded away.
///
/// `--older-than 0s --newer-than 0s` is the sharpest form: an empty window, which
/// must remove nothing from either tier.
#[test]
fn cache_gc_applies_newer_than_to_the_legacy_tier_too() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy_and_suite_caches(dir.path());

    bin()
        .args([
            "cache",
            "gc",
            "--older-than",
            "0s",
            "--newer-than",
            "0s",
            "evals",
        ])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("removed 0 of 1 cache entry"))
        .stdout(predicate::str::contains(
            "removed 0 of 1 legacy cache entry",
        ));

    // Both survive: an empty window excludes everything, on both tiers.
    assert_eq!(entry_count(&dir.path().join("evals/.domarinn/cache")), 1);
    assert_eq!(entry_count(&dir.path().join(".domarinn/cache")), 1);
}

/// The legacy tier is resolved against the *process cwd*, so for a suite
/// somewhere else on disk it is not a leftover — it is whichever project the
/// operator is standing in, and that project's cache is live. Purging it would
/// destroy a stranger's warm cache and report it as housekeeping.
#[test]
fn cache_clear_spares_the_cwd_tier_when_the_suite_is_not_under_it() {
    let here = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();

    // The project we are standing in, with a live cache of its own.
    std::fs::write(here.path().join("domarinn.yaml"), suite("mine", "mine")).unwrap();
    bin().arg("run").current_dir(here.path()).assert().success();

    // An unrelated project, named by absolute path on the command line.
    std::fs::write(
        elsewhere.path().join("domarinn.yaml"),
        suite("theirs", "theirs"),
    )
    .unwrap();
    bin()
        .arg("run")
        .current_dir(elsewhere.path())
        .assert()
        .success();

    // `stats` still names the tier — that is how an operator discovers it — but
    // must not promise a `cache clear` that will not come.
    bin()
        .args(["cache", "stats", elsewhere.path().to_str().unwrap()])
        .current_dir(here.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("legacy tier .domarinn/cache"))
        .stdout(predicate::str::contains("`cache clear` leaves it"));

    bin()
        .args(["cache", "clear", elsewhere.path().to_str().unwrap()])
        .current_dir(here.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("cleared 1 cache entry"))
        .stdout(predicate::str::contains("legacy").not());

    assert_eq!(
        entry_count(&elsewhere.path().join(".domarinn/cache")),
        0,
        "the suite named on the command line is the one to clear"
    );
    assert_eq!(
        entry_count(&here.path().join(".domarinn/cache")),
        1,
        "the cache of the directory we happen to be standing in is not a legacy tier"
    );
}

#[test]
fn cache_stats_and_path_name_the_legacy_tier_when_one_exists() {
    let dir = tempfile::tempdir().unwrap();
    seed_legacy_and_suite_caches(dir.path());

    bin()
        .args(["cache", "stats", "evals"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::starts_with("1 entries"))
        .stdout(predicate::str::contains(
            "legacy tier .domarinn/cache: 1 entries",
        ))
        .stdout(predicate::str::contains("read-only"));

    bin()
        .args(["cache", "path", "evals"])
        .current_dir(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("evals/.domarinn/cache"))
        .stdout(predicate::str::contains("legacy tier: .domarinn/cache"));
}

/// One entry in `<dir>/.domarinn/cache` (what a pre-0.4 run left beside the
/// process cwd) and one in `<dir>/evals/.domarinn/cache` (where the suite's
/// cache lives now). The two suites differ so neither run can hit the other's
/// entry and collapse the two tiers into one.
fn seed_legacy_and_suite_caches(dir: &std::path::Path) {
    std::fs::write(dir.join("domarinn.yaml"), suite("stale", "stale")).unwrap();
    bin().arg("run").current_dir(dir).assert().success();

    std::fs::create_dir_all(dir.join("evals")).unwrap();
    std::fs::write(dir.join("evals/domarinn.yaml"), suite("fresh", "fresh")).unwrap();
    bin()
        .args(["run", "evals"])
        .current_dir(dir)
        .assert()
        .success();

    assert_eq!(entry_count(&dir.join(".domarinn/cache")), 1);
    assert_eq!(entry_count(&dir.join("evals/.domarinn/cache")), 1);
}

/// `cache stats` stdout, for comparing a directory against itself across a
/// command that should not have touched it.
fn cache_stats(dir: &std::path::Path) -> String {
    let out = bin()
        .args(["cache", "stats"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "cache stats should succeed");
    String::from_utf8(out.stdout).unwrap()
}

/// Entry files under a `<root>/<shard>/<hex>.json` cache directory.
fn entry_count(root: &std::path::Path) -> usize {
    let Ok(shards) = std::fs::read_dir(root) else {
        return 0;
    };
    shards
        .filter_map(Result::ok)
        .filter_map(|shard| std::fs::read_dir(shard.path()).ok())
        .flat_map(|files| files.filter_map(Result::ok))
        .filter(|f| f.path().extension().is_some_and(|e| e == "json"))
        .count()
}
