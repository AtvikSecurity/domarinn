//! Projects, suites (with a recent pass-rate series), and baseline management.

use domarinn_core::ids::RunId;

use super::exec::{params, Conn, Queryable, Value};
use super::{ms_to_rfc3339, now_ms, Storage};
use crate::dto::projects::{
    ProjectListItem, ProjectsResponse, SuitePoint, SuiteSummary, SuitesResponse,
};
use crate::runsets::{visibility_predicate, RunVisibility};

/// What a suite's baseline is pinned to.
///
/// Exactly one of the two, enforced by the `baselines` CHECK (migration 20): a
/// fixed run, or a branch whose newest runs are merged into a composite at
/// resolution time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselinePin {
    Run(RunId),
    Branch(String),
}

impl Storage {
    /// The catalog of projects, counted over the runs this caller may see. A
    /// project whose every run is invisible disappears entirely rather than
    /// listing with a zero count.
    pub async fn list_projects(&self, vis: RunVisibility) -> anyhow::Result<ProjectsResponse> {
        self.runs.read(move |conn| list_projects(conn, &vis)).await
    }

    pub async fn list_suites(
        &self,
        project: String,
        vis: RunVisibility,
    ) -> anyhow::Result<SuitesResponse> {
        self.runs
            .read(move |conn| list_suites(conn, &project, &vis))
            .await
    }

    /// Point `(project, suite)`'s baseline at `pin` — a fixed run or a branch.
    ///
    /// For a run pin, `false` — a 404 at the handler — when the run does not
    /// exist, is not visible to this caller, or is not a run *of this suite*.
    /// All three collapse into one answer on purpose:
    ///
    /// * Visibility, because a baseline is a read of the named run as much as a
    ///   write to the suite. Without it, anyone who may write into any
    ///   unrestricted suite could probe arbitrary run ids and read 200-vs-404
    ///   to confirm a restricted run exists.
    /// * Membership, because it is what makes that probe pointless rather than
    ///   merely awkward, and because a baseline drawn from another suite is
    ///   meaningless anyway — every reader of this suite would be handed an id
    ///   that does not belong to it. `IS NOT DISTINCT FROM` rather than `=` so
    ///   the comparison is NULL-safe against a run that has no project or suite.
    ///
    /// A branch pin requires no run to exist yet — pinning `main` before the
    /// first upload is the natural CI bootstrap, and resolution honestly reads
    /// "no runs on that branch" as an absent baseline until one lands. There is
    /// nothing to probe: the pin names a branch, not a run.
    pub async fn set_baseline(
        &self,
        project: String,
        suite: String,
        pin: BaselinePin,
        vis: RunVisibility,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let mut tx = conn.immediate_tx()?;
                match &pin {
                    BaselinePin::Run(run_id) => {
                        let mut args: Vec<Value> = vec![
                            run_id.as_str().to_string().into(),
                            project.clone().into(),
                            suite.clone().into(),
                        ];
                        let visible = visibility_predicate("runs", &vis, &mut args);
                        let sql = format!(
                            "SELECT 1 FROM runs
                              WHERE id = ?1 AND project IS NOT DISTINCT FROM ?2
                                AND suite IS NOT DISTINCT FROM ?3 AND {visible}"
                        );
                        // `.query_row_opt(...)?`, never `.is_ok()` — see
                        // `project_has_visible_runs` in [`super::sets`]. A
                        // swallowed error here reports "no such run" for a
                        // broken database and silently leaves the baseline
                        // unpinned.
                        let exists = tx.query_row_opt(&sql, &args, |_| Ok(()))?.is_some();
                        if !exists {
                            return Ok(false);
                        }
                        tx.execute(
                            "INSERT INTO baselines (project, suite, run_id, branch, set_at)
                             VALUES (?1, ?2, ?3, NULL, ?4)
                             ON CONFLICT (project, suite) DO UPDATE
                                SET run_id = excluded.run_id, branch = NULL, set_at = excluded.set_at",
                            &params![project, suite, run_id.as_str(), now_ms()],
                        )?;
                    }
                    BaselinePin::Branch(branch) => {
                        tx.execute(
                            "INSERT INTO baselines (project, suite, run_id, branch, set_at)
                             VALUES (?1, ?2, NULL, ?3, ?4)
                             ON CONFLICT (project, suite) DO UPDATE
                                SET branch = excluded.branch, run_id = NULL, set_at = excluded.set_at",
                            &params![project, suite, branch.as_str(), now_ms()],
                        )?;
                    }
                }
                tx.commit()?;
                Ok(true)
            })
            .await
    }

    /// The suite's pin, visibility-filtered — `None` reads as unpinned.
    pub async fn baseline_pin(
        &self,
        project: String,
        suite: String,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<BaselinePin>> {
        self.runs
            .read(move |conn| read_baseline_pin(conn, &project, &suite, &vis))
            .await
    }

    /// The pin plus when it was set (epoch-ms) — `GET .../baseline` metadata.
    pub async fn baseline_meta(
        &self,
        project: String,
        suite: String,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<(BaselinePin, i64)>> {
        self.runs
            .read(move |conn| {
                Ok(read_baseline_row(conn, &project, &suite, &vis)?
                    .and_then(|(run_id, branch, set_at)| Some((pin_of(run_id, branch)?, set_at))))
            })
            .await
    }

    pub async fn delete_baseline(&self, project: String, suite: String) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "DELETE FROM baselines WHERE project = ?1 AND suite = ?2",
                    &params![project, suite],
                )?;
                Ok(n > 0)
            })
            .await
    }
}

fn list_projects(conn: &mut Conn<'_>, vis: &RunVisibility) -> anyhow::Result<ProjectsResponse> {
    let mut args: Vec<Value> = Vec::new();
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT project,
                COUNT(*) AS run_count,
                COUNT(DISTINCT suite) AS suite_count,
                MAX(created_at) AS last_run_at
         FROM runs
         WHERE project IS NOT NULL AND {visible}
         GROUP BY project
         ORDER BY last_run_at DESC"
    );
    let projects: Vec<ProjectListItem> = conn.query_map(&sql, &args, |row| {
        Ok(ProjectListItem {
            project: row.get::<String>(0)?,
            run_count: row.get::<i64>(1)?,
            suite_count: row.get::<i64>(2)?,
            last_run_at: ms_to_rfc3339(row.get::<i64>(3)?),
        })
    })?;
    Ok(ProjectsResponse { projects })
}

/// The suite's baseline pin — a run id or a branch — filtered by visibility.
///
/// For a run pin, `set_baseline` now refuses to record a run that is not of
/// this suite, so on a database written by this version the join can only ever
/// confirm what the caller already sees. It is unconditional anyway because
/// `baselines` has no constraint tying a row to its run's `(project, suite)`
/// and earlier versions never checked — an upgraded database can hold a row
/// pointing outside the set, and that row must not publish an invisible run's
/// id on a visible suite. The LEFT JOIN keeps that guard for run pins while
/// letting a branch pin — whose `run_id` is NULL, so there is no run to read —
/// through: a branch name is visible to whoever the enclosing query already
/// showed the suite.
pub(super) fn read_baseline_pin(
    conn: &mut Conn<'_>,
    project: &str,
    suite: &str,
    vis: &RunVisibility,
) -> anyhow::Result<Option<BaselinePin>> {
    Ok(read_baseline_row(conn, project, suite, vis)?
        .and_then(|(run_id, branch, _set_at)| pin_of(run_id, branch)))
}

/// The raw `baselines` row, visibility-filtered as documented on
/// [`read_baseline_pin`].
fn read_baseline_row(
    conn: &mut Conn<'_>,
    project: &str,
    suite: &str,
    vis: &RunVisibility,
) -> anyhow::Result<Option<(Option<String>, Option<String>, i64)>> {
    let mut args: Vec<Value> = vec![project.to_string().into(), suite.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT b.run_id, b.branch, b.set_at FROM baselines b
          LEFT JOIN runs ON runs.id = b.run_id
          WHERE b.project = ?1 AND b.suite = ?2
            AND (b.run_id IS NULL OR {visible})"
    );
    // `.query_row_opt(...)?`, never `.ok()` — see `project_has_visible_runs` in
    // [`super::sets`]. A suite reading as unpinned because the query failed is
    // indistinguishable from one that was never pinned.
    conn.query_row_opt(&sql, &args, |row| {
        Ok((
            row.get::<Option<String>>(0)?,
            row.get::<Option<String>>(1)?,
            row.get::<i64>(2)?,
        ))
    })
}

/// The typed pin a row's `(run_id, branch)` pair spells. A row with neither is
/// unreachable under the CHECK but must read as unpinned, not panic a listing.
fn pin_of(run_id: Option<String>, branch: Option<String>) -> Option<BaselinePin> {
    match (run_id, branch) {
        (Some(run_id), _) => Some(BaselinePin::Run(RunId::new(run_id))),
        (None, Some(branch)) => Some(BaselinePin::Branch(branch)),
        (None, None) => None,
    }
}

/// The two DTO columns a pin fans out into: `(baseline_run_id,
/// baseline_branch)`. Every listing carries both — a run pin fills the first, a
/// branch pin the second — so the split lives in one place.
pub(super) fn split_pin(pin: Option<BaselinePin>) -> (Option<RunId>, Option<String>) {
    match pin {
        Some(BaselinePin::Run(id)) => (Some(id), None),
        Some(BaselinePin::Branch(branch)) => (None, Some(branch)),
        None => (None, None),
    }
}

fn list_suites(
    conn: &mut Conn<'_>,
    project: &str,
    vis: &RunVisibility,
) -> anyhow::Result<SuitesResponse> {
    let mut names_args: Vec<Value> = vec![project.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut names_args);
    let names_sql = format!(
        "SELECT DISTINCT suite FROM runs
          WHERE project = ?1 AND suite IS NOT NULL AND {visible}"
    );
    let suite_names: Vec<String> =
        conn.query_map(&names_sql, &names_args, |row| row.get::<String>(0))?;

    let mut suites = Vec::new();
    for suite in suite_names {
        let mut series_args: Vec<Value> = vec![project.to_string().into(), suite.clone().into()];
        let visible = visibility_predicate("runs", vis, &mut series_args);
        let series_sql = format!(
            "SELECT id, created_at, case_count, pass_count
             FROM runs WHERE project = ?1 AND suite = ?2 AND {visible}
             ORDER BY created_at DESC LIMIT 20"
        );
        let series: Vec<SuitePoint> = conn.query_map(&series_sql, &series_args, |row| {
            let case_count: i64 = row.get(2)?;
            let pass_count: i64 = row.get(3)?;
            let pass_rate = if case_count > 0 {
                pass_count as f64 / case_count as f64
            } else {
                0.0
            };
            Ok(SuitePoint {
                run_id: RunId::new(row.get::<String>(0)?),
                created_at: ms_to_rfc3339(row.get::<i64>(1)?),
                total: case_count,
                passed: pass_count,
                pass_rate,
            })
        })?;

        let (baseline_run_id, baseline_branch) =
            split_pin(read_baseline_pin(conn, project, &suite, vis)?);

        let last_run_at = series.first().map(|s| s.created_at.clone());

        suites.push(SuiteSummary {
            suite,
            run_count: series.len() as i64,
            last_run_at,
            baseline_run_id,
            baseline_branch,
            series,
        });
    }

    Ok(SuitesResponse {
        project: project.to_string(),
        suites,
    })
}
