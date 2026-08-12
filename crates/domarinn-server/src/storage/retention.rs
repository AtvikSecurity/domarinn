//! Age-based run retention.
//!
//! Nothing ever deleted a run. `spawn_cache_retention` prunes `cache.db`, but
//! `runs`, `cases`, `run_blobs`, the tag tables and both FTS indexes grew
//! without bound — and with CI pushing on every pull request, that is the thing
//! that bites first.
//!
//! **Off by default.** Eval history is expensive to produce and impossible to
//! recreate; a harness that silently starts deleting it on upgrade would be a
//! bad trade for disk. Set `DOMARINN_RUN_MAX_AGE_DAYS` to opt in.
//!
//! Two runs are never evicted regardless of age, because deleting either one
//! breaks something a user can see:
//!
//! - **A pinned baseline.** It is what `--against server:baseline` resolves and
//!   what the compare view diffs against; evicting it turns a working CI gate
//!   into an unresolvable-baseline error.
//! - **The newest run of each `(project, suite, branch)`.** It is what the runs
//!   list and the status surface show. A suite that simply has not run in a
//!   while must go stale, not vanish.

use super::exec::{params, Conn, Queryable};
use super::Storage;

/// What a sweep removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Swept {
    pub deleted: usize,
    /// Runs old enough to evict that were kept because they are a pinned
    /// baseline or the newest of their branch. Reported rather than silently
    /// skipped: an operator who set a 7-day window and still sees old runs
    /// should be able to tell "the sweeper is broken" from "those are pinned".
    pub kept: usize,
}

impl Storage {
    /// Delete runs finished more than `max_age_days` ago, except those that are
    /// still load-bearing. See the module docs.
    pub async fn sweep_runs(&self, max_age_days: u64) -> anyhow::Result<Swept> {
        self.runs.write(move |conn| sweep(conn, max_age_days)).await
    }
}

/// Runs that must survive any sweep: every pinned baseline, plus the newest run
/// of each `(project, suite, git_branch)` group.
///
/// `IS NOT DISTINCT FROM` rather than `=` in the correlated subquery is
/// load-bearing: `project`, `suite` and `git_branch` are all nullable, and
/// `NULL = NULL` is NULL in SQL, so a run with no project would never match its
/// own group and every such run would look evictable.
const PROTECTED: &str = "
    id IN (SELECT run_id FROM baselines)
    OR id IN (
        SELECT r.id FROM runs r
        WHERE r.created_at = (
            SELECT MAX(r2.created_at) FROM runs r2
            WHERE r2.project IS NOT DISTINCT FROM r.project
              AND r2.suite IS NOT DISTINCT FROM r.suite
              AND r2.git_branch IS NOT DISTINCT FROM r.git_branch
        )
    )";

fn sweep(conn: &mut Conn<'_>, max_age_days: u64) -> anyhow::Result<Swept> {
    // Saturating, not `as i64 * 86_400_000`: `DOMARINN_RUN_MAX_AGE_DAYS` is
    // parsed as an unbounded u64, and a value past ~1.07e11 wraps the cast
    // negative, putting the cutoff in the *future* so the first sweep deletes
    // every run that is not explicitly protected. Release builds wrap silently.
    let window_ms = (max_age_days as i128) * 86_400_000i128;
    let cutoff = (super::now_ms() as i128 - window_ms).max(i64::MIN as i128) as i64;
    let mut tx = conn.immediate_tx()?;

    let expired: i64 = tx.query_row(
        "SELECT COUNT(*) FROM runs WHERE created_at < ?1",
        &params![cutoff],
        |row| row.get(0),
    )?;

    let victims: Vec<String> = {
        let sql = format!("SELECT id FROM runs WHERE created_at < ?1 AND NOT ({PROTECTED})");
        tx.query_map(&sql, &params![cutoff], |row| row.get::<String>(0))?
    };

    for id in &victims {
        // `runs` cascades to cases/blobs/tags via foreign keys, but the FTS
        // virtual tables have none — the same reason `delete_run` deletes them
        // explicitly.
        tx.execute("DELETE FROM runs WHERE id = ?1", &params![id])?;
        tx.execute("DELETE FROM runs_fts WHERE run_id = ?1", &params![id])?;
        tx.execute("DELETE FROM cases_fts WHERE run_id = ?1", &params![id])?;
    }
    tx.commit()?;

    Ok(Swept {
        deleted: victims.len(),
        kept: (expired as usize).saturating_sub(victims.len()),
    })
}
