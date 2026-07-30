//! Projects, suites (with a recent pass-rate series), and baseline management.

use rusqlite::{params, Connection, TransactionBehavior};

use domarinn_core::ids::RunId;

use super::{ms_to_rfc3339, now_ms, Storage};
use crate::dto::projects::{
    ProjectListItem, ProjectsResponse, SuitePoint, SuiteSummary, SuitesResponse,
};
use crate::runsets::{visibility_predicate, RunVisibility};

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

    /// Point `(project, suite)`'s baseline at `run_id`.
    ///
    /// `false` — a 404 at the handler — when the run does not exist, is not
    /// visible to this caller, or is not a run *of this suite*. All three
    /// collapse into one answer on purpose:
    ///
    /// * Visibility, because a baseline is a read of the named run as much as a
    ///   write to the suite. Without it, anyone who may write into any
    ///   unrestricted suite could probe arbitrary run ids and read 200-vs-404
    ///   to confirm a restricted run exists.
    /// * Membership, because it is what makes that probe pointless rather than
    ///   merely awkward, and because a baseline drawn from another suite is
    ///   meaningless anyway — every reader of this suite would be handed an id
    ///   that does not belong to it. `IS` rather than `=` so the comparison is
    ///   NULL-safe against a run that has no project or suite.
    pub async fn set_baseline(
        &self,
        project: String,
        suite: String,
        run_id: RunId,
        vis: RunVisibility,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let mut args: Vec<rusqlite::types::Value> = vec![
                    run_id.as_str().to_string().into(),
                    project.clone().into(),
                    suite.clone().into(),
                ];
                let visible = visibility_predicate("runs", &vis, &mut args);
                let sql = format!(
                    "SELECT 1 FROM runs
                      WHERE id = ?1 AND project IS ?2 AND suite IS ?3 AND {visible}"
                );
                let exists = tx
                    .query_row(&sql, rusqlite::params_from_iter(args.iter()), |_| Ok(()))
                    .is_ok();
                if !exists {
                    return Ok(false);
                }
                tx.execute(
                    "INSERT INTO baselines (project, suite, run_id, set_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(project, suite) DO UPDATE SET run_id = excluded.run_id, set_at = excluded.set_at",
                    params![project, suite, run_id.as_str(), now_ms()],
                )?;
                tx.commit()?;
                Ok(true)
            })
            .await
    }

    pub async fn delete_baseline(&self, project: String, suite: String) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let n = conn.execute(
                    "DELETE FROM baselines WHERE project = ?1 AND suite = ?2",
                    params![project, suite],
                )?;
                Ok(n > 0)
            })
            .await
    }
}

fn list_projects(conn: &Connection, vis: &RunVisibility) -> anyhow::Result<ProjectsResponse> {
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
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
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
        Ok(ProjectListItem {
            project: row.get::<_, String>(0)?,
            run_count: row.get::<_, i64>(1)?,
            suite_count: row.get::<_, i64>(2)?,
            last_run_at: ms_to_rfc3339(row.get::<_, i64>(3)?),
        })
    })?;
    let projects: Vec<ProjectListItem> = rows.collect::<Result<_, _>>()?;
    Ok(ProjectsResponse { projects })
}

/// The suite's baseline run id, filtered by visibility.
///
/// `set_baseline` now refuses to record a run that is not of this suite, so on
/// a database written by this version the join can only ever confirm what the
/// caller already sees. It is unconditional anyway because `baselines` has no
/// constraint tying a row to its run's `(project, suite)` and earlier versions
/// never checked — an upgraded database can hold a row pointing outside the
/// set, and that row must not publish an invisible run's id on a visible suite.
pub(super) fn read_baseline(
    conn: &Connection,
    project: &str,
    suite: &str,
    vis: &RunVisibility,
) -> anyhow::Result<Option<RunId>> {
    let mut args: Vec<rusqlite::types::Value> =
        vec![project.to_string().into(), suite.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT b.run_id FROM baselines b JOIN runs ON runs.id = b.run_id
          WHERE b.project = ?1 AND b.suite = ?2 AND {visible}"
    );
    Ok(conn
        .query_row(&sql, rusqlite::params_from_iter(args.iter()), |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .map(RunId::new))
}

fn list_suites(
    conn: &Connection,
    project: &str,
    vis: &RunVisibility,
) -> anyhow::Result<SuitesResponse> {
    let mut names_args: Vec<rusqlite::types::Value> = vec![project.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut names_args);
    let names_sql = format!(
        "SELECT DISTINCT suite FROM runs
          WHERE project = ?1 AND suite IS NOT NULL AND {visible}"
    );
    let mut stmt = conn.prepare(&names_sql)?;
    let suite_names: Vec<String> = stmt
        .query_map(rusqlite::params_from_iter(names_args.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<_, _>>()?;

    let mut suites = Vec::new();
    for suite in suite_names {
        let mut series_args: Vec<rusqlite::types::Value> =
            vec![project.to_string().into(), suite.clone().into()];
        let visible = visibility_predicate("runs", vis, &mut series_args);
        let series_sql = format!(
            "SELECT id, created_at, case_count, pass_count
             FROM runs WHERE project = ?1 AND suite = ?2 AND {visible}
             ORDER BY created_at DESC LIMIT 20"
        );
        let mut series_stmt = conn.prepare(&series_sql)?;
        let series: Vec<SuitePoint> = series_stmt
            .query_map(rusqlite::params_from_iter(series_args.iter()), |row| {
                let case_count: i64 = row.get(2)?;
                let pass_count: i64 = row.get(3)?;
                let pass_rate = if case_count > 0 {
                    pass_count as f64 / case_count as f64
                } else {
                    0.0
                };
                Ok(SuitePoint {
                    run_id: RunId::new(row.get::<_, String>(0)?),
                    created_at: ms_to_rfc3339(row.get::<_, i64>(1)?),
                    total: case_count,
                    passed: pass_count,
                    pass_rate,
                })
            })?
            .collect::<Result<_, _>>()?;

        let baseline_run_id = read_baseline(conn, project, &suite, vis)?;

        let last_run_at = series.first().map(|s| s.created_at.clone());

        suites.push(SuiteSummary {
            suite,
            run_count: series.len() as i64,
            last_run_at,
            baseline_run_id,
            series,
        });
    }

    Ok(SuitesResponse {
        project: project.to_string(),
        suites,
    })
}
