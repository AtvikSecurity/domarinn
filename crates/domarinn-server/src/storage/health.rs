//! Readiness probe: a trivial round trip through each logical database.

use super::exec::Queryable;
use super::Storage;

impl Storage {
    /// Prove both databases answer a query: `SELECT 1` through a pooled read
    /// connection on the runs handle and then the cache handle.
    ///
    /// Behind `GET /api/v1/ready`. Read connections, not the writers, so a
    /// probe never queues behind an ingest holding the writer mutex — a busy
    /// server is still a ready one. What it catches is the storage being gone:
    /// a Postgres that went away (a pooled reader reconnects and fails), or a
    /// SQLite data dir that was unmounted or lost its permissions.
    pub async fn ping(&self) -> anyhow::Result<()> {
        for db in [&self.runs, &self.cache] {
            db.read(|conn| conn.query_row("SELECT 1", &[], |_| Ok(())))
                .await?;
        }
        Ok(())
    }
}
