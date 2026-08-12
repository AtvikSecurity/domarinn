//! One-shot data migration from a SQLite deployment into an empty Postgres
//! database (`domarinn migrate-db`).
//!
//! The source is brought to the **latest** SQLite schema first — the same
//! migration + backfill calls `Storage::open_blocking` makes — so the copy
//! only ever deals with one shape per table. Each table's column list is then
//! derived at runtime as the intersection of SQLite's `PRAGMA table_info` and
//! Postgres's `information_schema.columns`: that automatically excludes the
//! PG-only columns (`cache_entries.id`, the generated `tsv` mirrors) and stays
//! correct when a future migration adds a column to both sides.
//!
//! The copy *order* is a hardcoded list, because FK dependency order cannot be
//! derived as cheaply as the columns can — and the list doubles as a tripwire:
//! it must exactly cover the discovered SQLite table set, so a future
//! migration that adds a table fails this tool loudly instead of silently not
//! copying it.
//!
//! Everything runs inside one `spawn_blocking`: the sync `postgres::Client`
//! embeds its own runtime and may be neither created nor dropped on a tokio
//! worker thread (see `Storage::open_postgres` and `DbInner::drop`).

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;

use super::exec::{Conn, Row, Value};
use super::{backfill, open_conn, pg, schema};

/// FK dependency order for the runs database. Parents strictly precede
/// children: everything under `runs` before the FTS mirrors (whose PG forms
/// reference `runs`), `users` before its three dependents and the grants.
const RUNS_TABLES: &[&str] = &[
    "runs",
    "run_tags",
    "run_blobs",
    "cases",
    "case_tags",
    "baselines",
    "runs_fts",
    "cases_fts",
    "users",
    "sessions",
    "api_keys",
    "user_identities",
    "login_transactions",
    "saml_consumed_assertions",
    "run_set_restrictions",
    "run_set_grants",
];

/// The cache database. `cache_counters` is updated, not inserted (Postgres
/// migration v1 seeds the singleton row); `cache_entries_fts` is re-keyed
/// from SQLite rowids to Postgres ids, so it must follow `cache_entries`.
const CACHE_TABLES: &[&str] = &["cache_entries", "cache_counters", "cache_entries_fts"];

/// Cap on parameter binds per INSERT batch. Postgres's protocol limit is
/// 65535; staying well under it keeps statements comfortably sized while
/// still amortizing the round trip.
const MAX_BINDS: usize = 20_000;

/// What was copied, in copy order: table name → rows.
#[derive(Debug)]
pub struct MigrateReport {
    pub tables: Vec<(String, u64)>,
}

/// Copy a SQLite deployment under `data_dir` into the (empty) database at
/// `database_url`. Refuses a target that already holds data.
pub async fn migrate_to_postgres(
    data_dir: PathBuf,
    database_url: String,
) -> anyhow::Result<MigrateReport> {
    tokio::task::spawn_blocking(move || migrate_blocking(&data_dir, &database_url))
        .await
        .context("migration task panicked")?
}

fn migrate_blocking(data_dir: &Path, database_url: &str) -> anyhow::Result<MigrateReport> {
    let runs_path = data_dir.join("domarinn.db");
    if !runs_path.exists() {
        anyhow::bail!(
            "no SQLite database at {} — is --data-dir pointing at the server's data directory?",
            runs_path.display()
        );
    }

    // Bring the source to the latest schema with the exact calls the server
    // makes on startup, so the column intersection below sees the final shape
    // and every backfill sentinel is already stamped.
    let mut runs_src = open_conn(&runs_path)?;
    schema::migrate_runs(&mut runs_src).context("applying runs migrations to the source")?;
    backfill::run(&mut Conn::Sqlite(&runs_src)).context("backfilling the source runs database")?;
    let cache_path = data_dir.join("cache.db");
    let mut cache_src = open_conn(&cache_path)?;
    schema::cache_migrations()
        .to_latest(&mut cache_src)
        .context("applying cache migrations to the source")?;

    let connector = pg::PgConnector::new(database_url)?;
    let mut dst = connector.connect()?;
    pg::migrate(&mut dst).context("applying postgres migrations")?;

    // Refuse a non-empty target outright: merging into live data cannot be
    // verified by the count check below, so it is not offered at all.
    for table in ["runs", "users", "cache_entries"] {
        let n: i64 = dst
            .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])?
            .get(0);
        if n > 0 {
            anyhow::bail!(
                "target database already has data ({table}: {n} rows); \
                 point --database-url at an empty database"
            );
        }
    }

    let mut report = MigrateReport { tables: Vec::new() };
    copy_database(&runs_src, &mut dst, RUNS_TABLES, &mut report)?;
    copy_database(&cache_src, &mut dst, CACHE_TABLES, &mut report)?;
    Ok(report)
}

fn copy_database(
    src: &rusqlite::Connection,
    dst: &mut postgres::Client,
    order: &[&str],
    report: &mut MigrateReport,
) -> anyhow::Result<()> {
    // The tripwire: the const list must exactly cover what the source
    // actually contains, or a table added by a future migration would be
    // silently left behind.
    let discovered = sqlite_tables(src)?;
    let listed: BTreeSet<String> = order.iter().map(|t| (*t).to_string()).collect();
    if discovered != listed {
        anyhow::bail!(
            "table list out of date: source has {:?}, migrate-db copies {:?} — \
             update the copy-order list in storage/migratedb.rs",
            discovered,
            listed
        );
    }

    for table in order {
        let copied = match *table {
            "cache_counters" => copy_cache_counters(src, dst)?,
            "cache_entries_fts" => copy_cache_fts(src, dst)?,
            _ => copy_table(src, dst, table)?,
        };
        verify_counts(src, dst, table).with_context(|| format!("verifying `{table}`"))?;
        report.tables.push(((*table).to_string(), copied));
    }
    Ok(())
}

/// Tables in the SQLite source, minus FTS5 shadow tables (`runs_fts_data`, …)
/// — those are internals of SQLite's FTS engine, not data to copy. Same
/// filtering as the schema drift guard.
fn sqlite_tables(conn: &rusqlite::Connection) -> anyhow::Result<BTreeSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_schema
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names: Vec<String> = stmt
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let fts: Vec<String> = names
        .iter()
        .filter(|n| n.ends_with("_fts"))
        .cloned()
        .collect();
    Ok(names
        .into_iter()
        .filter(|n| !fts.iter().any(|f| n.starts_with(&format!("{f}_"))))
        .collect())
}

/// A table's copyable columns: SQLite's declared order, kept only where the
/// Postgres side has the same name.
fn shared_columns(
    src: &rusqlite::Connection,
    dst: &mut postgres::Client,
    table: &str,
) -> anyhow::Result<Vec<String>> {
    let mut stmt = src.prepare(&format!("PRAGMA table_info({table})"))?;
    let sqlite_cols: Vec<String> = stmt
        .query_map([], |r| r.get(1))?
        .collect::<Result<_, _>>()?;
    let pg_cols: BTreeSet<String> = dst
        .query(
            "SELECT column_name FROM information_schema.columns
             WHERE table_schema = 'public' AND table_name = $1",
            &[&table],
        )?
        .into_iter()
        .map(|row| row.get(0))
        .collect();
    if pg_cols.is_empty() {
        anyhow::bail!("table `{table}` does not exist on the postgres side");
    }
    let cols: Vec<String> = sqlite_cols
        .into_iter()
        .filter(|c| pg_cols.contains(c))
        .collect();
    if cols.is_empty() {
        anyhow::bail!("no shared columns for table `{table}`");
    }
    Ok(cols)
}

/// Generic row copy: SELECT every shared column from SQLite as [`Value`]s and
/// bind them straight into batched multi-row INSERTs — the `Value` type
/// implements both drivers' parameter traits, so no per-table code exists.
fn copy_table(
    src: &rusqlite::Connection,
    dst: &mut postgres::Client,
    table: &str,
) -> anyhow::Result<u64> {
    let cols = shared_columns(src, dst, table)?;
    let ncols = cols.len();
    let batch_rows = (MAX_BINDS / ncols).max(1);

    let mut stmt = src.prepare(&format!("SELECT {} FROM {table}", cols.join(", ")))?;
    let mut rows = stmt.query([])?;
    let mut buf: Vec<Value> = Vec::with_capacity(batch_rows * ncols);
    let mut copied = 0u64;
    while let Some(row) = rows.next()? {
        let row = Row::Sqlite(row);
        for i in 0..ncols {
            buf.push(row.get::<Value>(i)?);
        }
        if buf.len() == batch_rows * ncols {
            copied += insert_batch(dst, table, &cols, &buf)?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        copied += insert_batch(dst, table, &cols, &buf)?;
    }
    Ok(copied)
}

/// One multi-row `INSERT INTO t (cols) VALUES ($1,…),(…)`. `values` is the
/// flat row-major buffer; its length is always a multiple of `cols.len()`.
fn insert_batch(
    dst: &mut postgres::Client,
    table: &str,
    cols: &[String],
    values: &[Value],
) -> anyhow::Result<u64> {
    let ncols = cols.len();
    let nrows = values.len() / ncols;
    let mut sql = format!("INSERT INTO {table} ({}) VALUES ", cols.join(", "));
    for r in 0..nrows {
        if r > 0 {
            sql.push(',');
        }
        sql.push('(');
        for c in 0..ncols {
            if c > 0 {
                sql.push(',');
            }
            write!(sql, "${}", r * ncols + c + 1).expect("write to String");
        }
        sql.push(')');
    }
    let params: Vec<&(dyn postgres::types::ToSql + Sync)> = values
        .iter()
        .map(|v| v as &(dyn postgres::types::ToSql + Sync))
        .collect();
    dst.execute(&sql, &params)
        .with_context(|| format!("inserting into `{table}`"))
}

/// The counters singleton: Postgres migration v1 already seeded the row, so
/// the SQLite hits/misses land as an UPDATE rather than an INSERT.
fn copy_cache_counters(
    src: &rusqlite::Connection,
    dst: &mut postgres::Client,
) -> anyhow::Result<u64> {
    let (hits, misses): (i64, i64) = src.query_row(
        "SELECT hits, misses FROM cache_counters WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    dst.execute(
        "UPDATE cache_counters SET hits = $1, misses = $2 WHERE id = 1",
        &[&hits, &misses],
    )?;
    Ok(1)
}

/// The cache FTS mirror is the one table whose key changes representation:
/// SQLite rows are keyed by `cache_entries.rowid`, Postgres rows by the
/// identity `id` minted when `cache_entries` was copied above. Bridge through
/// the stable `key`: resolve each source row to its key via the rowid join,
/// then map key → new id from the target.
fn copy_cache_fts(src: &rusqlite::Connection, dst: &mut postgres::Client) -> anyhow::Result<u64> {
    let mut stmt = src.prepare(
        "SELECT ce.key, f.request, f.output
         FROM cache_entries_fts f JOIN cache_entries ce ON ce.rowid = f.rowid",
    )?;
    let mut rows = stmt.query([])?;

    let key_to_id: HashMap<String, i64> = dst
        .query("SELECT key, id FROM cache_entries", &[])?
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();

    let cols = [
        "id".to_string(),
        "request".to_string(),
        "output".to_string(),
    ];
    let batch_rows = (MAX_BINDS / cols.len()).max(1);
    let mut buf: Vec<Value> = Vec::with_capacity(batch_rows * cols.len());
    let mut copied = 0u64;
    while let Some(row) = rows.next()? {
        let row = Row::Sqlite(row);
        let key: String = row.get(0)?;
        let id = *key_to_id
            .get(&key)
            .with_context(|| format!("cache entry `{key}` has an FTS row but was not copied"))?;
        buf.push(Value::Int(id));
        buf.push(row.get::<Value>(1)?);
        buf.push(row.get::<Value>(2)?);
        if buf.len() == batch_rows * cols.len() {
            copied += insert_batch(dst, "cache_entries_fts", &cols, &buf)?;
            buf.clear();
        }
    }
    if !buf.is_empty() {
        copied += insert_batch(dst, "cache_entries_fts", &cols, &buf)?;
    }
    Ok(copied)
}

/// Row counts must agree on both sides before a table is reported as copied.
/// For the cache FTS mirror the source count is the rowid join's — an FTS row
/// whose entry vanished has no key to carry it over.
fn verify_counts(
    src: &rusqlite::Connection,
    dst: &mut postgres::Client,
    table: &str,
) -> anyhow::Result<()> {
    let src_sql = match table {
        "cache_entries_fts" => "SELECT COUNT(*) FROM cache_entries_fts f \
             JOIN cache_entries ce ON ce.rowid = f.rowid"
            .to_string(),
        _ => format!("SELECT COUNT(*) FROM {table}"),
    };
    let src_count: i64 = src.query_row(&src_sql, [], |r| r.get(0))?;
    let dst_count: i64 = dst
        .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])?
        .get(0);
    if src_count != dst_count {
        anyhow::bail!("row count mismatch on `{table}`: sqlite {src_count}, postgres {dst_count}");
    }
    Ok(())
}
