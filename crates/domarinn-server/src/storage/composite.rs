//! Assembling a branch's composite baseline from stored run blobs.
//!
//! The server stores every run's complete document as a compressed blob;
//! rebuilding one from `cases` rows would lose the detail (asserts, previews,
//! config snapshot) a comparison renders. So a branch baseline hydrates the
//! newest [`BRANCH_LOOKBACK`](domarinn_core::composite::BRANCH_LOOKBACK) blobs
//! on the branch and merges them with the same
//! [`merge_branch_runs`](domarinn_core::composite::merge_branch_runs) the CLI
//! uses on its local store, so the two can never disagree about what a branch
//! baseline contains.

use domarinn_core::composite::{merge_branch_runs, BRANCH_LOOKBACK};
use domarinn_core::ids::RunId;
use domarinn_core::result::RunResult;

use super::exec::{Conn, Queryable, Value};
use super::{decompress, Storage};
use crate::runsets::{visibility_predicate, RunVisibility};

impl Storage {
    /// The composite baseline for `(project, suite)` on `branch`, or `None`
    /// when no visible run on that branch contributed a case.
    ///
    /// `exclude` drops the run being gated from the merge — a head run that
    /// uploaded before comparing must not become its own baseline.
    ///
    /// Deliberately *no* cached filter: a fully-cached run's verdicts are
    /// re-derived from cached requests and are as real as any other run's. The
    /// runs-list hiding of fully-cached runs is a browsing affordance, not a
    /// validity judgment.
    pub async fn branch_baseline_export(
        &self,
        project: String,
        suite: String,
        branch: String,
        exclude: Option<RunId>,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<RunResult>> {
        self.runs
            .read(move |conn| {
                let runs = branch_runs_newest_first(
                    conn,
                    &project,
                    &suite,
                    &branch,
                    exclude.as_ref(),
                    &vis,
                )?;
                Ok(merge_branch_runs(&branch, runs))
            })
            .await
    }
}

/// The newest [`BRANCH_LOOKBACK`] visible run documents of `(project, suite)`
/// on `branch`, hydrated from their stored blobs, newest first.
///
/// An undecodable blob is skipped rather than failing the whole gate: one
/// corrupt historical run must not make a suite's baseline unresolvable.
fn branch_runs_newest_first(
    conn: &mut Conn<'_>,
    project: &str,
    suite: &str,
    branch: &str,
    exclude: Option<&RunId>,
    vis: &RunVisibility,
) -> anyhow::Result<Vec<RunResult>> {
    let mut args: Vec<Value> = vec![
        project.to_string().into(),
        suite.to_string().into(),
        branch.to_string().into(),
    ];
    let visible = visibility_predicate("r", vis, &mut args);
    let exclude_clause = match exclude {
        Some(id) => {
            args.push(id.as_str().to_string().into());
            format!("AND r.id <> ?{}", args.len())
        }
        None => String::new(),
    };
    args.push((BRANCH_LOOKBACK as i64).into());
    let limit_pos = args.len();
    // Served by idx_runs_proj_suite_branch_created; the blob join is one row
    // per selected run.
    let sql = format!(
        "SELECT b.body FROM runs r JOIN run_blobs b ON b.run_id = r.id
          WHERE r.project = ?1 AND r.suite = ?2 AND r.git_branch = ?3
            AND {visible} {exclude_clause}
          ORDER BY r.created_at DESC, r.id DESC
          LIMIT ?{limit_pos}"
    );
    let blobs: Vec<Vec<u8>> = conn.query_map(&sql, &args, |row| row.get(0))?;
    Ok(blobs
        .iter()
        .filter_map(|blob| {
            let bytes = decompress(blob).ok()?;
            serde_json::from_slice::<RunResult>(&bytes).ok()
        })
        .collect())
}
