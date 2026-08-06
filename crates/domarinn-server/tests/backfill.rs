//! Storage-level tests for migration 3: the queryable case cell/score/
//! stop_reason columns, the runs `config_digest` column, ingest writes, and the
//! blob backfill that populates them for pre-migration rows.
//!
//! These reach past the HTTP surface (no DTO exposes the new columns yet) and
//! assert directly against the SQLite file with a raw `rusqlite` connection.

mod common;

use std::path::Path;

use common::*;
use domarinn_core::ids::RunId;
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

fn index_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
        .unwrap();
    stmt.query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The migration-3 cell columns for one `cases` row.
#[derive(Debug, PartialEq)]
struct CellRow {
    provider_id: String,
    prompt_id: Option<String>,
    test_id: String,
    repeat_idx: i64,
    score: f64,
    stop_reason: Option<String>,
}

fn cell_rows(conn: &Connection, run_id: &str) -> Vec<CellRow> {
    let mut stmt = conn
        .prepare(
            "SELECT provider_id, prompt_id, test_id, repeat_idx, score, stop_reason
             FROM cases WHERE run_id = ?1 ORDER BY idx",
        )
        .unwrap();
    stmt.query_map([run_id], |r| {
        Ok(CellRow {
            provider_id: r.get(0)?,
            prompt_id: r.get(1)?,
            test_id: r.get(2)?,
            repeat_idx: r.get(3)?,
            score: r.get(4)?,
            stop_reason: r.get(5)?,
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

#[tokio::test]
async fn migration_adds_cell_columns_score_stop_reason_and_indexes() {
    let dir = TempDir::new().unwrap();
    let _storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let conn = raw(dir.path());

    let cases = table_columns(&conn, "cases");
    for col in [
        "provider_id",
        "prompt_id",
        "test_id",
        "repeat_idx",
        "score",
        "stop_reason",
    ] {
        assert!(cases.contains(&col.to_string()), "cases missing {col}");
    }
    assert!(
        table_columns(&conn, "runs").contains(&"config_digest".to_string()),
        "runs missing config_digest"
    );

    let idx = index_names(&conn);
    for name in [
        "idx_cases_run_provider",
        "idx_cases_run_test",
        "idx_cases_key",
        "idx_runs_digest",
    ] {
        assert!(idx.contains(&name.to_string()), "missing index {name}");
    }
}

#[tokio::test]
async fn ingest_populates_cell_columns_and_stop_reason() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();

    let mut run = two_case_run("run-ingest");
    // stop_reason present on the first case, absent (NULL) on the second.
    run.cases[0].stop_reason = Some("length".to_string());
    storage.ingest_run(run, None).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    let rows = cell_rows(&conn, "run-ingest");

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        CellRow {
            provider_id: "openai".to_string(),
            prompt_id: None,
            test_id: "t1".to_string(),
            repeat_idx: 0,
            score: 1.0,
            stop_reason: Some("length".to_string()),
        }
    );
    assert_eq!(
        rows[1],
        CellRow {
            provider_id: "anthropic".to_string(),
            prompt_id: None,
            test_id: "t2".to_string(),
            repeat_idx: 0,
            score: 0.0,
            stop_reason: None,
        }
    );

    let digest: String = conn
        .query_row(
            "SELECT config_digest FROM runs WHERE id = 'run-ingest'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(digest, "sha256:deadbeef");
}

#[tokio::test]
async fn backfill_repopulates_columns_nulled_after_ingest() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(two_case_run("run-bf"), None)
        .await
        .unwrap();
    drop(storage);

    // Simulate rows written before migration 3: all new columns NULL.
    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET provider_id=NULL, prompt_id=NULL, test_id=NULL,
                 repeat_idx=NULL, score=NULL, stop_reason=NULL;
             UPDATE runs SET config_digest=NULL;",
        )
        .unwrap();
    }

    // Reopen: open_blocking runs the backfill after migrations.
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    let null_cases: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cases WHERE provider_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(null_cases, 0, "every case should be backfilled");

    let rows = cell_rows(&conn, "run-bf");
    assert_eq!(
        rows[0],
        CellRow {
            provider_id: "openai".to_string(),
            prompt_id: None,
            test_id: "t1".to_string(),
            repeat_idx: 0,
            score: 1.0,
            stop_reason: None,
        }
    );
    assert_eq!(
        rows[1],
        CellRow {
            provider_id: "anthropic".to_string(),
            prompt_id: None,
            test_id: "t2".to_string(),
            repeat_idx: 0,
            score: 0.0,
            stop_reason: None,
        }
    );

    let digest: String = conn
        .query_row(
            "SELECT config_digest FROM runs WHERE id='run-bf'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(digest, "sha256:deadbeef");
}

#[tokio::test]
async fn migration_adds_the_case_error_column() {
    let dir = TempDir::new().unwrap();
    let _storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let conn = raw(dir.path());
    assert!(
        table_columns(&conn, "cases").contains(&"error".to_string()),
        "cases missing error"
    );
}

#[tokio::test]
async fn ingest_promotes_the_case_error_out_of_the_blob() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(
            make_run(
                "run-err",
                Some("proj"),
                Some("suite"),
                vec![],
                Some("main"),
                0,
                &[
                    CaseSpec::new("openai", "t1", CaseStatus::Error)
                        .error("provider returned 502 after 3 retries"),
                    CaseSpec::new("openai", "t2", CaseStatus::Pass),
                ],
            ),
            None,
        )
        .await
        .unwrap();

    let cases = storage
        .list_cases(default_case_filter(RunId::new("run-err")))
        .await
        .unwrap();

    let errored = cases
        .cases
        .iter()
        .find(|c| c.status == CaseStatus::Error)
        .expect("the errored case");
    assert_eq!(
        errored.error.as_deref(),
        Some("provider returned 502 after 3 retries"),
        "an errored case must carry its reason: it has no output, so the grid \
         has nothing else to show"
    );

    let passed = cases
        .cases
        .iter()
        .find(|c| c.status == CaseStatus::Pass)
        .expect("the passing case");
    assert_eq!(
        passed.error, None,
        "the '' sentinel written for 'no error' must read back as None"
    );
}

#[tokio::test]
async fn backfill_repopulates_the_case_error_for_pre_migration_rows() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(
            make_run(
                "run-bf-err",
                Some("proj"),
                Some("suite"),
                vec![],
                Some("main"),
                0,
                &[
                    CaseSpec::new("openai", "t1", CaseStatus::Error).error("boom"),
                    CaseSpec::new("openai", "t2", CaseStatus::Pass),
                ],
            ),
            None,
        )
        .await
        .unwrap();
    drop(storage);

    // Simulate rows written before migration 7.
    {
        let conn = raw(dir.path());
        conn.execute_batch("UPDATE cases SET error = NULL;")
            .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let cases = storage
        .list_cases(default_case_filter(RunId::new("run-bf-err")))
        .await
        .unwrap();
    assert_eq!(
        cases
            .cases
            .iter()
            .find(|c| c.status == CaseStatus::Error)
            .and_then(|c| c.error.as_deref()),
        Some("boom"),
    );
    drop(storage);

    // Every row is now stamped, so a second open selects nothing: a case with
    // no error must land as '' rather than NULL or the backfill would rescan
    // the whole table on every startup.
    let conn = raw(dir.path());
    let unstamped: i64 = conn
        .query_row("SELECT COUNT(*) FROM cases WHERE error IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unstamped, 0, "backfill must stamp every row, error or not");
}

/// A two-case run where the first case's output came back empty for `reason`.
fn empty_reason_run(id: &str, reason: &'static str) -> domarinn_core::result::RunResult {
    make_run(
        id,
        Some("proj"),
        Some("suite"),
        vec![],
        Some("main"),
        0,
        &[
            CaseSpec::new("openai", "t1", CaseStatus::Fail)
                .output(Some(""))
                .empty_reason(reason),
            CaseSpec::new("openai", "t2", CaseStatus::Pass),
        ],
    )
}

/// `(empty_reason, ...)` for a run's cases, in `idx` order.
fn empty_reasons(conn: &Connection, run_id: &str) -> Vec<Option<String>> {
    let mut stmt = conn
        .prepare("SELECT empty_reason FROM cases WHERE run_id = ?1 ORDER BY idx")
        .unwrap();
    stmt.query_map([run_id], |r| r.get::<_, Option<String>>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn empty_count(conn: &Connection, run_id: &str) -> Option<i64> {
    conn.query_row(
        "SELECT empty_count FROM runs WHERE id = ?1",
        [run_id],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn migration_adds_the_empty_columns_and_index() {
    let dir = TempDir::new().unwrap();
    let _storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let conn = raw(dir.path());

    assert!(
        table_columns(&conn, "cases").contains(&"empty_reason".to_string()),
        "cases missing empty_reason"
    );
    assert!(
        table_columns(&conn, "runs").contains(&"empty_count".to_string()),
        "runs missing empty_count"
    );
    assert!(
        index_names(&conn).contains(&"idx_cases_run_empty_reason".to_string()),
        "missing index idx_cases_run_empty_reason"
    );
}

#[tokio::test]
async fn ingest_promotes_the_empty_reason_and_counts_it_on_the_run() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(empty_reason_run("run-empty", "refusal"), None)
        .await
        .unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        empty_reasons(&conn, "run-empty"),
        vec![Some("refusal".to_string()), Some(String::new())],
        "ingest must stamp '' for a case with no reason, never NULL: NULL is \
         reserved for 'not yet backfilled' and would make the backfill \
         predicate re-select fresh rows on every open"
    );
    assert_eq!(empty_count(&conn, "run-empty"), Some(1));
}

#[tokio::test]
async fn backfill_populates_empty_reason_and_empty_count_from_blobs() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(empty_reason_run("run-bf-empty", "refusal"), None)
        .await
        .unwrap();
    drop(storage);

    // Simulate rows written before migration 15.
    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET empty_reason = NULL;
             UPDATE runs SET empty_count = NULL;",
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        empty_reasons(&conn, "run-bf-empty"),
        vec![Some("refusal".to_string()), Some(String::new())],
        "the reason comes back out of the case blob; a case without one is \
         stamped '' rather than left NULL"
    );
    assert_eq!(empty_count(&conn, "run-bf-empty"), Some(1));

    // The anti-spin property: every row is stamped, so a second open selects
    // nothing at all.
    let unstamped: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cases WHERE empty_reason IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unstamped, 0, "backfill must stamp every case row");
    let unstamped_runs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM runs WHERE empty_count IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unstamped_runs, 0, "backfill must stamp every run row");
}

/// The blob-decoding path has to draw the "is this empty" line in the same
/// place ingest does: a blob whose case carries a present-but-blank
/// `empty_reason` backfills to the `''` "known: not empty" sentinel, which the
/// detail tally and case grid both exclude — so the run count must exclude it
/// too, or the list would claim an empty case the detail cannot show.
#[tokio::test]
async fn backfill_does_not_count_a_blank_reason_as_empty() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(empty_reason_run("run-bf-blank", ""), None)
        .await
        .unwrap();
    drop(storage);

    // Simulate rows written before migration 15, so the backfill re-derives
    // both columns from the blob rather than reading what ingest computed.
    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET empty_reason = NULL;
             UPDATE runs SET empty_count = NULL;",
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        empty_reasons(&conn, "run-bf-blank"),
        vec![Some(String::new()), Some(String::new())],
        "a blank reason is indistinguishable from no reason once stored"
    );
    assert_eq!(
        empty_count(&conn, "run-bf-blank"),
        Some(0),
        "no case is selectable as empty, so the run tally must be 0"
    );
}

/// The migration-15 upgrade shape — a run missing *only* `empty_count` — is
/// every run in the store on the first open after upgrading, so it must be
/// filled by counting the (already-backfilled) case rows, not by decoding the
/// run blob. Proven by corrupting the run blob: the blob path would stamp the
/// `-1` sentinel, so getting the real count back means the blob was never read.
#[tokio::test]
async fn empty_count_backfill_counts_case_rows_instead_of_decoding_the_run_blob() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(empty_reason_run("run-bf-fast", "refusal"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        // Only the migration-15 run column is missing; the case rows are
        // nulled too, so the count must come from what `backfill_cases` has
        // just re-stamped, in order — not from what ingest left behind.
        conn.execute_batch(
            "UPDATE cases SET empty_reason = NULL;
             UPDATE runs SET empty_count = NULL;",
        )
        .unwrap();
        conn.execute(
            "UPDATE run_blobs SET body = ?1 WHERE run_id='run-bf-fast'",
            rusqlite::params![vec![0xde_u8, 0xad, 0xbe, 0xef]],
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        empty_count(&conn, "run-bf-fast"),
        Some(1),
        "an upgrade-shaped run must get its count from the case rows; -1 here \
         means the blob path ran (and hit the corrupt blob) instead"
    );
}

#[tokio::test]
async fn backfill_stamps_empty_sentinels_for_corrupt_blobs_and_stops_rescanning() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(empty_reason_run("run-empty-corrupt", "refusal"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET empty_reason = NULL;
             UPDATE runs SET empty_count = NULL;",
        )
        .unwrap();
        conn.execute(
            "UPDATE cases SET detail = ?1 WHERE run_id='run-empty-corrupt' AND idx=0",
            rusqlite::params![vec![0xde_u8, 0xad, 0xbe, 0xef]],
        )
        .unwrap();
        conn.execute(
            "UPDATE run_blobs SET body = ?1 WHERE run_id='run-empty-corrupt'",
            rusqlite::params![vec![0xde_u8, 0xad, 0xbe, 0xef]],
        )
        .unwrap();
    }

    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    assert_eq!(
        empty_reasons(&conn, "run-empty-corrupt")[0],
        Some(String::new()),
        "an undecodable case blob gets the empty-string sentinel"
    );
    assert_eq!(
        empty_count(&conn, "run-empty-corrupt"),
        Some(-1),
        "an undecodable run blob gets the -1 sentinel"
    );

    // Both sentinels are non-NULL, so a second open selects zero rows.
    let unstamped: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM cases WHERE empty_reason IS NULL)
                  + (SELECT COUNT(*) FROM runs WHERE empty_count IS NULL)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unstamped, 0, "sentinels must end the rescan");
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

#[tokio::test]
async fn migration_adds_the_answered_by_provider_column() {
    let dir = TempDir::new().unwrap();
    let _storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    let conn = raw(dir.path());

    assert!(
        table_columns(&conn, "cases").contains(&"answered_by_provider_id".to_string()),
        "cases missing answered_by_provider_id"
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

#[tokio::test]
async fn backfill_marks_corrupt_case_blob_with_sentinel_and_completes() {
    let dir = TempDir::new().unwrap();
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    storage
        .ingest_run(two_case_run("run-corrupt"), None)
        .await
        .unwrap();
    drop(storage);

    {
        let conn = raw(dir.path());
        conn.execute_batch(
            "UPDATE cases SET provider_id=NULL, prompt_id=NULL, test_id=NULL,
                 repeat_idx=NULL, score=NULL, stop_reason=NULL;",
        )
        .unwrap();
        // Corrupt just the first case's detail blob (undecompressible bytes).
        conn.execute(
            "UPDATE cases SET detail = ?1 WHERE run_id='run-corrupt' AND idx=0",
            rusqlite::params![vec![0xde_u8, 0xad, 0xbe, 0xef]],
        )
        .unwrap();
    }

    // Must complete without hanging or erroring.
    let storage = Storage::open(dir.path().to_path_buf()).await.unwrap();
    drop(storage);

    let conn = raw(dir.path());
    let corrupt: String = conn
        .query_row(
            "SELECT provider_id FROM cases WHERE run_id='run-corrupt' AND idx=0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(corrupt, "", "corrupt row gets the empty-string sentinel");

    let good: String = conn
        .query_row(
            "SELECT provider_id FROM cases WHERE run_id='run-corrupt' AND idx=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(good, "anthropic", "intact row still backfills correctly");

    // Sentinel is non-NULL, so a further reopen does not rescan it forever.
    let null_left: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM cases WHERE provider_id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(null_left, 0);
}
