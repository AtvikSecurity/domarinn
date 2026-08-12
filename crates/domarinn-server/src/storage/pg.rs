//! Postgres connection plumbing and the migration runner.
//!
//! The driver stack is pure Rust end to end: the sync `postgres` facade over
//! tokio-postgres (each client embeds its own small runtime, which drops
//! cleanly into `Db`'s `spawn_blocking` closure model), TLS via rustls
//! pinned to the `ring` provider with bundled webpki roots. No libpq, so the
//! musl-static and distroless `ldd` release gates never see a new `.so`.
//!
//! Migrations run under `pg_advisory_lock`: several server replicas pointed
//! at one database — the deployment Postgres exists for — can race at
//! startup, and the lock serializes them where SQLite got the same guarantee
//! from the single-writer file lock.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use tokio_postgres_rustls::MakeRustlsConnect;

use super::{now_ms, schema_pg};

/// One arbitrary-but-stable key for the whole migration batch. Advisory
/// locks are database-scoped, so this only ever contends with other domarinn
/// servers on the same database — which is the point.
const MIGRATION_LOCK_KEY: i64 = 0x646f_6d61_7269_6e6e; // "domarinn"

/// Everything needed to open another connection to the configured database.
/// Cheap to clone; the reader pool uses it to grow lazily, exactly like the
/// SQLite side reopens its database file.
#[derive(Clone)]
pub(super) struct PgConnector {
    config: postgres::Config,
    tls: Arc<rustls::ClientConfig>,
}

impl PgConnector {
    pub fn new(url: &str) -> anyhow::Result<PgConnector> {
        let config =
            postgres::Config::from_str(url).context("parsing the postgres connection URL")?;
        // Ring, explicitly — rustls's default provider is aws-lc-rs, which
        // the container build forbids (see the Dockerfile header).
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .context("building rustls client config")?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(PgConnector {
            config,
            tls: Arc::new(tls),
        })
    }

    /// Open a new connection. Whether TLS is actually negotiated follows the
    /// URL's `sslmode` (`disable`/`prefer`/`require`), handled by the driver.
    pub fn connect(&self) -> anyhow::Result<postgres::Client> {
        let tls = MakeRustlsConnect::new((*self.tls).clone());
        self.config
            .connect(tls)
            .context("connecting to postgres")
            .map_err(Into::into)
    }
}

/// Apply both schema ledgers, serialized across replicas.
///
/// Two ledgers — `schema_migrations` (runs) and `cache_schema_migrations` —
/// mirror the two-database split on SQLite, so either table set can gain a
/// migration without renumbering the other.
pub(super) fn migrate(client: &mut postgres::Client) -> anyhow::Result<()> {
    client
        .query("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_KEY])
        .context("taking the migration advisory lock")?;
    let result =
        apply_ledger(client, "schema_migrations", schema_pg::RUNS_MIGRATIONS).and_then(|()| {
            apply_ledger(
                client,
                "cache_schema_migrations",
                schema_pg::CACHE_MIGRATIONS,
            )
        });
    // Best-effort: the lock is session-scoped and the session closes soon
    // after a failure anyway.
    let _ = client.query("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_KEY]);
    result
}

fn apply_ledger(
    client: &mut postgres::Client,
    table: &str,
    migrations: &[(i64, &str)],
) -> anyhow::Result<()> {
    client.batch_execute(&format!(
        "CREATE TABLE IF NOT EXISTS {table} \
         (version BIGINT PRIMARY KEY, applied_at BIGINT NOT NULL)"
    ))?;
    let current: i64 = client
        .query_one(
            &format!("SELECT COALESCE(MAX(version), 0) FROM {table}"),
            &[],
        )?
        .get(0);
    for (version, sql) in migrations {
        if *version <= current {
            continue;
        }
        let mut tx = client.transaction()?;
        tx.batch_execute(sql)
            .with_context(|| format!("applying {table} migration {version}"))?;
        tx.execute(
            &format!("INSERT INTO {table} (version, applied_at) VALUES ($1, $2)"),
            &[version, &now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(())
}
