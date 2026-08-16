//! The local run store: resolving a run reference to a stored `result.json`.
//!
//! One place owns where runs live and how a user-supplied reference maps onto
//! them, so `run`, `diff`, `view`, `share`, `ci-summary`, and `runs` cannot
//! disagree about it.
//!
//! Two properties are worth stating because they are easy to lose:
//!
//! * **Every branch verifies the file exists before returning a path.**
//!   Returning a plausible-but-absent path just moves the failure to whoever
//!   opens it, where the message is `No such file` against a path the user
//!   never typed.
//! * **A failed lookup says what *is* available.** A run id is an opaque ULID
//!   nobody memorizes; "could not resolve 'run-abc'" with no alternatives is a
//!   dead end.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use domarinn_core::RunResult;
use serde::Deserialize;

/// How many run ids a "could not resolve" message lists.
const SUGGESTION_LIMIT: usize = 5;

/// The local run store, holding one `<run_id>/result.json` per persisted run
/// plus a plain-text `latest` pointer file.
///
/// `DOMARINN_RUNS_DIR` overrides the default so a caller can point at another
/// checkout's history — or, in tests, at a temporary directory — without
/// changing the working directory out from under everything else.
pub(crate) fn runs_dir() -> PathBuf {
    match std::env::var("DOMARINN_RUNS_DIR") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => Path::new(".domarinn").join("runs"),
    }
}

/// The `latest` pointer file: the id of the most recently persisted run of
/// *any* suite.
pub(crate) fn latest_pointer() -> PathBuf {
    runs_dir().join("latest")
}

/// Just enough of a stored run to identify it without decoding cases.
#[derive(Deserialize)]
struct RunIdent {
    run_id: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    suite: Option<String>,
    finished_at: DateTime<Utc>,
    #[serde(default)]
    git: Option<GitIdent>,
}

/// The one git field a scan needs: `branch:<name>` resolution filters on it.
#[derive(Deserialize)]
struct GitIdent {
    #[serde(default)]
    branch: Option<String>,
}

/// A run found on disk.
pub struct StoredRun {
    pub run_id: String,
    pub project: Option<String>,
    pub suite: Option<String>,
    pub finished_at: DateTime<Utc>,
    /// The recorded `git.branch`, when the run happened inside a repository.
    pub branch: Option<String>,
    pub path: PathBuf,
}

/// Every readable run in the store, newest first.
///
/// A corrupt or half-written run is skipped rather than fatal: it must not be
/// able to stop a lookup from finding an older, valid run. Scanning is
/// affordable because this is a developer's local history and a lookup happens
/// once per command.
pub fn scan() -> Vec<StoredRun> {
    let dir = runs_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut runs: Vec<StoredRun> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let path = e.path().join("result.json");
            let ident: RunIdent = read_json(&path).ok()?;
            Some(StoredRun {
                run_id: ident.run_id,
                project: ident.project,
                suite: ident.suite,
                finished_at: ident.finished_at,
                branch: ident.git.and_then(|g| g.branch),
                path,
            })
        })
        .collect();
    runs.sort_by_key(|run| std::cmp::Reverse(run.finished_at));
    runs
}

/// Deserialize a JSON file without buffering it into a `String` first.
///
/// A run document with large outputs is easily tens of megabytes; reading to a
/// `String` and then parsing holds two copies at once for no benefit.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .map_err(|e| format!("parsing {}: {e}", path.display()))
}

/// Resolve a run reference to an existing `result.json` path.
///
/// Accepts, in order: `latest`, a path to a `result.json`, a path to a run
/// directory, or a bare run id under the store.
pub fn resolve_run_path(reference: &str) -> Result<PathBuf, String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err("empty run reference".to_string());
    }
    if reference == "latest" {
        return resolve_latest();
    }

    let path = Path::new(reference);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if path.is_dir() {
        let candidate = path.join("result.json");
        return if candidate.is_file() {
            Ok(candidate)
        } else {
            Err(format!(
                "'{reference}' is a directory but holds no result.json"
            ))
        };
    }

    // Bare run id. Reject anything with path structure: a run id is a single
    // opaque segment, and joining `../..` onto the store would read outside it.
    if reference.contains(['/', '\\']) || reference == ".." {
        return Err(format!(
            "'{reference}' is not a run id, and no file exists at that path"
        ));
    }
    let candidate = runs_dir().join(reference).join("result.json");
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err(unresolved(reference))
}

/// Resolve `latest`, falling back to a scan when the pointer is stale.
///
/// The pointer is a plain file rewritten on every run, so it outlives the run
/// it names: pruning `.domarinn/runs`, or copying a store between machines,
/// leaves it dangling. Falling back to the newest run on disk is both what the
/// user meant and strictly better than failing.
fn resolve_latest() -> Result<PathBuf, String> {
    if let Ok(id) = std::fs::read_to_string(latest_pointer()) {
        let candidate = runs_dir().join(id.trim()).join("result.json");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    match scan().into_iter().next() {
        Some(run) => Ok(run.path),
        None => Err(format!(
            "no runs found in {}; run a suite first",
            runs_dir().display()
        )),
    }
}

/// The "could not resolve" message, naming a few ids that *do* exist.
fn unresolved(reference: &str) -> String {
    let dir = runs_dir();
    let known = scan();
    if known.is_empty() {
        return format!(
            "could not resolve run reference '{reference}': no runs found in {}",
            dir.display()
        );
    }
    let mut ids: Vec<String> = known
        .iter()
        .take(SUGGESTION_LIMIT)
        .map(|r| r.run_id.clone())
        .collect();
    if known.len() > SUGGESTION_LIMIT {
        ids.push(format!("… and {} more", known.len() - SUGGESTION_LIMIT));
    }
    format!(
        "could not resolve run reference '{reference}'. \
         Known runs (newest first): {}. Pass `latest`, a run id, or a path to a result.json.",
        ids.join(", ")
    )
}

/// Load a run by reference.
pub fn load_run(reference: &str) -> Result<RunResult, String> {
    read_json(&resolve_run_path(reference)?)
}

/// The newest stored run of the same `(project, suite)` as `head`, excluding
/// `head` itself.
///
/// `--against latest` must not use the `latest` pointer file. That pointer
/// records the last run of *any* suite, so in a repo with more than one suite
/// it silently diffs one suite against another — and `diff_runs` joins on
/// `case_key` without a suite guard, so the result looks plausible rather than
/// empty.
pub fn latest_for_suite(head: &RunResult) -> Option<PathBuf> {
    scan()
        .into_iter()
        .find(|run| {
            run.run_id != head.run_id.as_str()
                && run.project == head.project
                && run.suite == head.suite
        })
        .map(|run| run.path)
}

/// The newest stored runs of `head`'s suite on `branch`, fully loaded, newest
/// first — the input to a composite merge. Excludes `head` itself (it persists
/// to the store before comparison, so on its own branch it is always the
/// newest candidate) and caps the walk at the shared
/// [`BRANCH_LOOKBACK`](domarinn_core::composite::BRANCH_LOOKBACK) window.
///
/// A run whose ident matched but whose full document fails to parse is skipped
/// for the same reason `scan` skips it: one corrupt file must not make a
/// baseline unresolvable.
pub fn runs_on_branch(head: &RunResult, branch: &str) -> Vec<RunResult> {
    scan()
        .into_iter()
        .filter(|run| {
            run.run_id != head.run_id.as_str()
                && run.project == head.project
                && run.suite == head.suite
                && run.branch.as_deref() == Some(branch)
        })
        .take(domarinn_core::composite::BRANCH_LOOKBACK)
        .filter_map(|run| read_json(&run.path).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// `DOMARINN_RUNS_DIR` is process-global, so the tests that set it must not
    /// interleave.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct Store {
        _guard: MutexGuard<'static, ()>,
        dir: tempfile::TempDir,
    }

    impl Store {
        fn new() -> Store {
            let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = tempfile::TempDir::new().unwrap();
            // SAFETY: serialized by ENV_LOCK; no other thread reads the var
            // while a Store is alive.
            unsafe { std::env::set_var("DOMARINN_RUNS_DIR", dir.path()) };
            Store { _guard: guard, dir }
        }

        /// A minimal but *complete* stored run: `RunResult` has required
        /// fields beyond the identity ones, and a fixture missing them would
        /// exercise `RunIdent` while silently never testing `load_run`.
        fn write(&self, run_id: &str, suite: Option<&str>, minute: i64) {
            let run = serde_json::json!({
                "schema_version": domarinn_core::result::RESULT_SCHEMA_VERSION,
                "run_id": run_id,
                "project": "proj",
                "suite": suite,
                "started_at": "2026-01-01T00:00:00Z",
                "finished_at": format!("2026-01-01T00:{minute:02}:00Z"),
                "config_digest": "sha256:test",
                "config_snapshot": {},
                "cases": [],
                "summary": {
                    "total": 0, "passed": 0, "failed": 0, "errored": 0, "skipped": 0,
                    "duration_ms": 0, "prompt_tokens": 0, "completion_tokens": 0,
                    "cache_hits": 0, "cache_misses": 0,
                },
            });
            let dir = self.dir.path().join(run_id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("result.json"),
                serde_json::to_vec_pretty(&run).unwrap(),
            )
            .unwrap();
        }

        fn point_latest_at(&self, id: &str) {
            std::fs::write(self.dir.path().join("latest"), id).unwrap();
        }
    }

    impl Drop for Store {
        fn drop(&mut self) {
            // SAFETY: still holding ENV_LOCK.
            unsafe { std::env::remove_var("DOMARINN_RUNS_DIR") };
        }
    }

    #[test]
    fn resolves_a_bare_run_id() {
        let store = Store::new();
        store.write("run-a", Some("s"), 0);
        let path = resolve_run_path("run-a").unwrap();
        assert!(path.ends_with("run-a/result.json"));
    }

    #[test]
    fn resolves_latest_through_the_pointer() {
        let store = Store::new();
        store.write("run-a", Some("s"), 0);
        store.write("run-b", Some("s"), 5);
        store.point_latest_at("run-a");
        // The pointer wins even though run-b is newer: it records what the last
        // command actually produced.
        assert!(resolve_run_path("latest")
            .unwrap()
            .ends_with("run-a/result.json"));
    }

    /// The pointer outlives the run it names — pruning the store, or copying it
    /// between machines, leaves it dangling.
    #[test]
    fn a_stale_latest_pointer_falls_back_to_the_newest_run() {
        let store = Store::new();
        store.write("run-a", Some("s"), 0);
        store.write("run-b", Some("s"), 5);
        store.point_latest_at("run-deleted");
        assert!(resolve_run_path("latest")
            .unwrap()
            .ends_with("run-b/result.json"));
    }

    #[test]
    fn latest_on_an_empty_store_says_to_run_a_suite() {
        let _store = Store::new();
        let err = resolve_run_path("latest").unwrap_err();
        assert!(err.contains("run a suite first"), "{err}");
    }

    #[test]
    fn an_unknown_id_lists_the_ids_that_do_exist() {
        let store = Store::new();
        store.write("run-a", Some("s"), 0);
        store.write("run-b", Some("s"), 5);
        let err = resolve_run_path("run-typo").unwrap_err();
        assert!(err.contains("run-a"), "{err}");
        assert!(err.contains("run-b"), "{err}");
        assert!(err.contains("Known runs"), "{err}");
    }

    #[test]
    fn an_unknown_id_on_an_empty_store_says_so_plainly() {
        let _store = Store::new();
        let err = resolve_run_path("run-a").unwrap_err();
        assert!(err.contains("no runs found"), "{err}");
    }

    #[test]
    fn a_directory_without_a_result_json_is_reported_as_such() {
        let store = Store::new();
        let empty = store.dir.path().join("bare");
        std::fs::create_dir_all(&empty).unwrap();
        let err = resolve_run_path(empty.to_str().unwrap()).unwrap_err();
        assert!(err.contains("holds no result.json"), "{err}");
    }

    /// A bare id is joined onto the store, so it must not be able to carry path
    /// structure out of it. `..` resolves as an ordinary directory reference
    /// first and is refused there for having no `result.json`; what matters is
    /// that none of these ever reads outside the store.
    #[test]
    fn a_traversing_reference_is_refused_rather_than_joined() {
        let _store = Store::new();
        for reference in ["../../etc/passwd", "a/b"] {
            let err = resolve_run_path(reference).unwrap_err();
            assert!(err.contains("not a run id"), "{reference}: {err}");
        }
        assert!(resolve_run_path("..").is_err());
    }

    #[test]
    fn an_empty_reference_is_refused() {
        let _store = Store::new();
        assert!(resolve_run_path("   ").unwrap_err().contains("empty"));
    }

    #[test]
    fn scan_returns_newest_first_and_skips_corrupt_runs() {
        let store = Store::new();
        store.write("run-a", Some("s"), 0);
        store.write("run-b", Some("s"), 5);
        let broken = store.dir.path().join("run-broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("result.json"), b"{not json").unwrap();

        let runs = scan();
        let ids: Vec<&str> = runs.iter().map(|r| r.run_id.as_str()).collect();
        assert_eq!(ids, ["run-b", "run-a"]);
    }

    #[test]
    fn latest_for_suite_ignores_other_suites_and_the_head_run() {
        let store = Store::new();
        store.write("run-old", Some("alpha"), 0);
        store.write("run-other", Some("beta"), 5);
        store.write("run-head", Some("alpha"), 10);

        let head: RunResult = read_json(&resolve_run_path("run-head").unwrap()).unwrap();
        let base = latest_for_suite(&head).expect("a same-suite predecessor exists");
        assert!(
            base.ends_with("run-old/result.json"),
            "must skip the newer run of a different suite, and itself"
        );
    }

    #[test]
    fn latest_for_suite_is_none_when_the_suite_has_only_one_run() {
        let store = Store::new();
        store.write("run-other", Some("beta"), 5);
        store.write("run-head", Some("alpha"), 10);
        let head: RunResult = read_json(&resolve_run_path("run-head").unwrap()).unwrap();
        assert!(latest_for_suite(&head).is_none());
    }
}
