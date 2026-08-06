//! Storage-level tests for the fallback-attribution columns: migration 17's
//! `cases.answered_by_provider_id`, migration 18's `runs.fallback_count`, and
//! the backfill fast path that stamps pre-feature case rows without decoding
//! their blobs.
//!
//! Split from `backfill.rs` along the feature seam — same conventions: raw
//! `rusqlite` against the SQLite file, past the HTTP surface.

mod common;

use std::path::Path;

use common::*;
use domarinn_core::result::CaseStatus;
use domarinn_server::storage::Storage;
use rusqlite::Connection;
use tempfile::TempDir;

/// Open a plain read/write connection straight to the runs database file so
/// tests can inspect (and corrupt) columns the public API does not expose.
fn raw(dir: &Path) -> Connection {
    Connection::open(dir.join("domarinn.db")).expect("open raw runs db")
}

fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

/// The case's cell identity, which fallback attribution must never move.
struct CellIdentity {
    provider_id: String,
}

fn cell_rows(conn: &Connection, run_id: &str) -> Vec<CellIdentity> {
    let mut stmt = conn
        .prepare("SELECT provider_id FROM cases WHERE run_id = ?1 ORDER BY idx")
        .unwrap();
    stmt.query_map([run_id], |r| {
        Ok(CellIdentity {
            provider_id: r.get(0)?,
        })
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A two-case run: idx 0 = openai/t1 (pass), idx 1 = anthropic/t2 (fail).
fn two_case_run(id: &str) -> domarinn_core::result::RunResult {
    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec!["nightly"],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Pass),
            CaseSpec::new("anthropic", "t2", CaseStatus::Fail),
        ],
    )
}

/// A two-case run on the `primary` provider whose first case was answered by
/// the `reserve` fallback instead. The cell stays keyed on `primary` — only the
/// answerer differs.
fn fallback_run(id: &str) -> domarinn_core::result::RunResult {
    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("primary", "t1", CaseStatus::Pass).answered_by("reserve"),
            CaseSpec::new("primary", "t2", CaseStatus::Pass),
        ],
    )
}

/// `(answered_by_provider_id, ...)` for a run's cases, in `idx` order.
fn answered_by(conn: &Connection, run_id: &str) -> Vec<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT answered_by_provider_id FROM cases WHERE run_id = ?1 ORDER BY idx")
        .unwrap();
    stmt.query_map([run_id], |r| r.get::<_, Option<String>>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn fallback_count(conn: &Connection, run_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT fallback_count FROM runs WHERE id = ?1",
        [run_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn migration_adds_the_answered_by_provider_column_and_its_run_tally() {
    let dir = TempDir::new().unwrap();
    let _storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let conn = raw(dir.path());

    assert!(
        table_columns(&conn, "cases").contains(&"answered_by_provider_id".to_string()),
        "cases missing answered_by_provider_id"
    );
    assert!(
        table_columns(&conn, "runs").contains(&"fallback_count".to_string()),
        "runs missing fallback_count"
    );
}

#[tokio::test]
async fn ingest_tallies_the_runs_fallback_answers() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fc"), None)
        .await
        .unwrap();
    // A run that never fell back must tally an honest 0, not NULL: 0 is what
    // licenses the case backfill's no-decode fast path.
    storage
        .ingest_run(two_case_run("run-fc-none"), None)
        .await
        .unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(fallback_count(&conn, "run-fc"), Some(1));
    assert_eq!(fallback_count(&conn, "run-fc-none"), Some(0));
}

/// A case whose fallback answer was *skipped* still counts here, unlike the
/// client's graded-only `summary.fallback_cases`. The tally answers "can a case
/// blob in this run hold an answerer" — and this one does, so reading the
/// summary instead would strand a real attribution behind the fast path.
#[tokio::test]
async fn the_fallback_tally_counts_skipped_answers_too() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(
            make_run(
                "run-fc-skip",
                Some("proj"),
                Some("suite"),
                vec![],
                Some("main"),
                0,
                &[CaseSpec::new("primary", "t1", CaseStatus::Skip)
                    .output(Some(""))
                    .empty_reason("refusal")
                    .answered_by("reserve")],
            ),
            None,
        )
        .await
        .unwrap();
    drop(storage);

    assert_eq!(fallback_count(&raw(dir.path()), "run-fc-skip"), Some(1));
}

/// The migration-17/18 upgrade shape: every case row's answerer is NULL and
/// every run's tally is NULL. The run tally is rebuilt from the run blob (one
/// decode per *run*), and the case rows of runs that never fell back are then
/// stamped in a single UPDATE without touching a case blob.
#[tokio::test]
async fn backfill_rebuilds_the_fallback_tally_from_the_run_blob() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fc-bf"), None)
        .await
        .unwrap();
    storage
        .ingest_run(two_case_run("run-fc-bf-none"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET answered_by_provider_id = NULL;
             UPDATE runs SET fallback_count = NULL;",
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(fallback_count(&conn, "run-fc-bf"), Some(1));
    assert_eq!(fallback_count(&conn, "run-fc-bf-none"), Some(0));
    assert_eq!(
        answered_by(&conn, "run-fc-bf"),
        vec![Some("reserve".to_string()), Some(String::new())],
        "the run that did fall back is decoded case by case, so its real \
         answerer survives"
    );
    assert_eq!(
        answered_by(&conn, "run-fc-bf-none"),
        vec![Some(String::new()), Some(String::new())],
    );
}

/// The fast path is proven by lying to it: the run's case blob really does name
/// `reserve`, but the tally says the run never fell back, so a pass that reads
/// blobs would restore `reserve` and one that trusts the tally stamps `''`.
/// Seeing `''` is the only way to observe that no case blob was decoded.
///
/// The lie is also the reason the tally may never be guessed: this is exactly
/// the data loss a blanket `''` stamp would cause on every run uploaded between
/// the client gaining `answered_by_provider_id` and the server gaining its
/// column.
#[tokio::test]
async fn the_answerer_fast_path_trusts_the_tally_instead_of_decoding() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fc-fast"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET answered_by_provider_id = NULL;
             UPDATE runs SET fallback_count = 0;",
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    assert_eq!(
        answered_by(&raw(dir.path()), "run-fc-fast"),
        vec![Some(String::new()), Some(String::new())],
        "a `reserve` here means the case blobs were decoded, i.e. the \
         single-UPDATE fast path did not run"
    );
}

/// The mirror of the test above, and the reason the fast path is keyed on `= 0`
/// rather than "not NULL": an undecodable run blob leaves the -1 sentinel,
/// which vouches for nothing, so those case rows must still be read.
#[tokio::test]
async fn an_unreadable_run_tally_sends_its_cases_down_the_decode_path() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fc-sentinel"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET answered_by_provider_id = NULL;
             UPDATE runs SET fallback_count = -1;",
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    assert_eq!(
        answered_by(&raw(dir.path()), "run-fc-sentinel"),
        vec![Some("reserve".to_string()), Some(String::new())],
        "-1 is 'unknown', which must never be read as 'no handoff'"
    );
}

#[tokio::test]
async fn ingest_promotes_the_answering_provider_and_stamps_the_rest() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fallback"), None)
        .await
        .unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        answered_by(&conn, "run-fallback"),
        vec![Some("reserve".to_string()), Some(String::new())],
        "ingest must stamp '' when the configured provider answered, never \
         NULL: NULL is reserved for 'not yet backfilled' and would make the \
         backfill predicate re-select fresh rows on every open"
    );

    // The cell identity is untouched: the column and every case_key join stay
    // keyed on the configured provider.
    let cells = cell_rows(&conn, "run-fallback");
    assert_eq!(cells[0].provider_id, "primary");
    assert_eq!(cells[1].provider_id, "primary");
}

#[tokio::test]
async fn backfill_populates_answered_by_provider_from_blobs() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-bf-fallback"), None)
        .await
        .unwrap();
    drop(storage);

    // Simulate rows written before migration 17.
    {
        let conn = raw(dir.path());
        conn.execute_batch("UPDATE cases SET answered_by_provider_id = NULL;")
            .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        answered_by(&conn, "run-bf-fallback"),
        vec![Some("reserve".to_string()), Some(String::new())],
        "the answerer comes back out of the case blob; a case whose blob has no \
         such field is stamped '' rather than left NULL"
    );

    // The anti-spin property: every row is stamped, so a second open selects
    // nothing at all.
    let unstamped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cases WHERE answered_by_provider_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unstamped, 0, "backfill must stamp every case row");
}

#[tokio::test]
async fn backfill_stamps_the_answered_by_sentinel_for_a_corrupt_blob() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(fallback_run("run-fallback-corrupt"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch("UPDATE cases SET answered_by_provider_id = NULL;")
            .unwrap();
        conn.execute(
            "UPDATE cases SET detail = ?1 WHERE run_id='run-fallback-corrupt' AND idx=0",
            rusqlite::params![vec![0xde_u8, 0xad, 0xbe, 0xef]],
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        answered_by(&conn, "run-fallback-corrupt"),
        vec![Some(String::new()), Some(String::new())],
        "an undecodable case blob gets the empty-string sentinel, which ends \
         the rescan even though the answerer it held is now unrecoverable"
    );
}
