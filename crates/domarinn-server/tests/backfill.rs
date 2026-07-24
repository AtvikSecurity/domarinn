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
