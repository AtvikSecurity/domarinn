//! Hybrid SQLite storage for runs and the shared cache.
//!
//! Two database files live under the data dir:
//! * `measurellm.db` — durable run history. Each run is stored twice: as a
//!   zstd-compressed blob of the original document (for lossless export) and as
//!   normalized rows (`runs`, `cases`, tags) for indexed filtering.
//! * `cache.db` — the content-addressed provider cache. Disposable.
//!
//! Concurrency: each DB has one writer connection behind a [`tokio::sync::Mutex`]
//! and a small pool of read connections. Every SQL call runs inside
//! [`tokio::task::spawn_blocking`]. Connections open in WAL mode and writes use
//! `BEGIN IMMEDIATE`.
//!
//! This module is split into focused submodules by concern:
//! * [`schema`] — migration SQL,
//! * [`runs`] — ingest + run list/detail/export,
//! * [`cases`] — case list (lean) + case detail,
//! * [`compare`] — the run/run diff,
//! * [`projects`] — projects, suites, and baselines,
//! * [`cache`] — the content-addressed cache table, stats, and pruning.
//!
//! [`Storage`] is defined here; each submodule attaches its own `impl Storage`
//! block. The public surface (`Storage`, [`RunListFilter`], [`CaseListFilter`],
//! [`IngestOutcome`], [`CachePutOutcome`], and the cursor/time helpers) is
//! re-exported from this module so callers see a flat `crate::storage::*`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use anyhow::Context;
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as TokioMutex;

mod cache;
mod cases;
mod compare;
mod projects;
mod runs;
mod schema;

pub use cases::CaseListFilter;
pub use runs::{RunListFilter, RunListPage};

const MAX_READERS: usize = 4;

/// Outcome of ingesting a run (drives the HTTP status code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// A new run was stored.
    Created,
    /// The same run id + identical content already existed (idempotent).
    Existing,
    /// The same run id exists but with different content.
    Conflict,
}

/// Outcome of a cache PUT (first-write-wins).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CachePutOutcome {
    Created,
    Exists,
}

// ---------------------------------------------------------------------------
// Connection helpers
// ---------------------------------------------------------------------------

fn open_conn(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening sqlite db at {}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;\
         PRAGMA busy_timeout=5000;\
         PRAGMA synchronous=NORMAL;\
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(conn)
}

/// One database file with a writer + a lazily-grown pool of read connections.
struct Db {
    inner: Arc<DbInner>,
}

struct DbInner {
    writer: TokioMutex<Connection>,
    readers: StdMutex<Vec<Connection>>,
    path: PathBuf,
}

impl Db {
    fn new(writer: Connection, path: PathBuf) -> Self {
        Db {
            inner: Arc::new(DbInner {
                writer: TokioMutex::new(writer),
                readers: StdMutex::new(Vec::new()),
                path,
            }),
        }
    }

    /// Run a closure with the exclusive writer connection.
    async fn write<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&mut Connection) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut guard = inner.writer.blocking_lock();
            f(&mut guard)
        })
        .await
        .context("write task panicked")?
    }

    /// Run a closure with a pooled read connection.
    async fn read<F, T>(&self, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Connection) -> anyhow::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.acquire_reader()?;
            let result = f(&conn);
            inner.release_reader(conn);
            result
        })
        .await
        .context("read task panicked")?
    }
}

impl DbInner {
    fn acquire_reader(&self) -> anyhow::Result<Connection> {
        if let Some(conn) = self.readers.lock().unwrap().pop() {
            return Ok(conn);
        }
        open_conn(&self.path)
    }

    fn release_reader(&self, conn: Connection) {
        let mut pool = self.readers.lock().unwrap();
        if pool.len() < MAX_READERS {
            pool.push(conn);
        }
    }
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

/// The storage handle shared across the app (cheap to clone).
///
/// The methods on this type are implemented across the submodules of this
/// module, grouped by concern.
#[derive(Clone)]
pub struct Storage {
    runs: Arc<Db>,
    cache: Arc<Db>,
}

impl Storage {
    /// Open (creating if needed) both databases and run migrations.
    pub async fn open(dir: PathBuf) -> anyhow::Result<Storage> {
        tokio::task::spawn_blocking(move || Storage::open_blocking(&dir))
            .await
            .context("storage open task panicked")?
    }

    fn open_blocking(dir: &Path) -> anyhow::Result<Storage> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating data dir {}", dir.display()))?;

        let runs_path = dir.join("measurellm.db");
        let mut runs_writer = open_conn(&runs_path)?;
        schema::runs_migrations()
            .to_latest(&mut runs_writer)
            .context("applying runs migrations")?;

        let cache_path = dir.join("cache.db");
        let mut cache_writer = open_conn(&cache_path)?;
        schema::cache_migrations()
            .to_latest(&mut cache_writer)
            .context("applying cache migrations")?;

        Ok(Storage {
            runs: Arc::new(Db::new(runs_writer, runs_path)),
            cache: Arc::new(Db::new(cache_writer, cache_path)),
        })
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (used across the storage submodules)
// ---------------------------------------------------------------------------

pub(super) fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

pub(super) fn ms_to_rfc3339(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

pub(super) fn to_microusd(cost: Option<f64>) -> Option<i64> {
    cost.map(|c| (c * 1_000_000.0).round() as i64)
}

pub(super) fn from_microusd(micro: Option<i64>) -> Option<f64> {
    micro.map(|m| m as f64 / 1_000_000.0)
}

pub(super) fn compress(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(zstd::encode_all(bytes, 3)?)
}

pub(super) fn decompress(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(zstd::decode_all(bytes)?)
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// sha256 over the canonical JSON encoding of the run (stable regardless of map
/// ordering), used for idempotency.
pub(super) fn content_hash(value: &serde_json::Value) -> String {
    let canonical = measurellm_core::cache::canonical_json(value);
    format!("sha256:{}", sha256_hex(canonical.as_bytes()))
}

/// Cursor encodes `created_at_ms:run_id`.
pub fn encode_cursor(created_at: i64, id: &str) -> String {
    format!("{created_at}:{id}")
}

/// Parse a run-list cursor of the form `created_at_ms:run_id`.
pub fn decode_cursor(cursor: &str) -> Option<(i64, String)> {
    let (ms, id) = cursor.split_once(':')?;
    Some((ms.parse().ok()?, id.to_string()))
}

/// Parse a `since`/`until` query value: either epoch-ms or an RFC3339 timestamp.
pub fn parse_time_ms(raw: &str) -> Option<i64> {
    if let Ok(ms) = raw.parse::<i64>() {
        return Some(ms);
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis())
}
