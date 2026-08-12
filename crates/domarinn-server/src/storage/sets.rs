//! Aggregates for the run-set browser (`/api/v1/sets*`).
//!
//! Separate from [`super::runs`] (which is at the per-file line ratchet) and
//! from [`super::projects`] (whose DTOs are a frozen legacy wire shape that
//! older CLIs depend on). Nothing here is shared with those endpoints on
//! purpose: the sets browser is free to change its shape, `/api/v1/projects`
//! is not.
//!
//! Every query is filtered by [`visibility_predicate`], so a project or suite
//! whose runs are all invisible disappears rather than listing with a zero
//! count — the same rule [`super::projects::list_projects`] follows.

use std::collections::HashMap;

use super::exec::{Conn, Queryable, Row, Value};
use super::projects::read_baseline;
use super::runsets::{covering_level, restricted};
use super::Storage;
use crate::dto::sets::{
    ProjectSetDetailResponse, ProjectSetView, SetsResponse, SuiteSetDetailResponse, SuiteSetView,
};
use crate::runsets::{visibility_predicate, GrantLevel, RunVisibility};

/// How many recent runs a suite's sparkline covers. Matches the legacy suite
/// series window, so the two surfaces tell the same story about "recent".
const SPARKLINE_RUNS: usize = 20;

/// How many per-suite pass rates a project's row carries. A project with more
/// suites than this shows the first 20 in name order rather than an unbounded
/// array — the row is a glance, not a report.
const PROJECT_SPARK_CAP: usize = 20;

/// The per-run aggregate columns, in the order every reader below unpacks them.
const AGGREGATES: &str = "COUNT(*), MAX(created_at), \
     CAST(COALESCE(SUM(pass_count), 0) AS BIGINT), CAST(COALESCE(SUM(fail_count), 0) AS BIGINT), \
     CAST(COALESCE(SUM(error_count), 0) AS BIGINT), CAST(COALESCE(SUM(case_count), 0) AS BIGINT), \
     CAST(COALESCE(SUM(CASE WHEN empty_count > 0 THEN empty_count ELSE 0 END), 0) AS BIGINT)";

/// A run's pass rate, as SQL. A run with no cases is 0.0 rather than NULL: the
/// sparkline is a `number[]`, and "no cases" is not a gap in the series.
const PASS_RATE: &str =
    "CASE WHEN case_count > 0 THEN CAST(pass_count AS REAL) / case_count ELSE 0.0 END";

/// `COUNT(*), MAX(created_at), SUM(pass), SUM(fail), SUM(error), SUM(cases),
/// SUM(empty)`, read from `row` starting at `offset`.
struct Aggregates {
    run_count: i64,
    last_run_at: Option<i64>,
    pass_count: i64,
    fail_count: i64,
    error_count: i64,
    case_count: i64,
    /// The `CASE` in [`AGGREGATES`] is what makes `runs.empty_count`'s
    /// tri-state safe to sum: a NULL (pre-backfill) or `-1` (undecodable blob)
    /// run adds nothing instead of poisoning the total or, worse, subtracting
    /// one from it. Zero here means "no run had anything to report", which the
    /// DTO omits — see [`crate::dto::sets`]'s header.
    empty_count: i64,
}

impl Aggregates {
    fn read(row: &Row<'_>, offset: usize) -> anyhow::Result<Aggregates> {
        Ok(Aggregates {
            run_count: row.get(offset)?,
            last_run_at: row.get(offset + 1)?,
            pass_count: row.get(offset + 2)?,
            fail_count: row.get(offset + 3)?,
            error_count: row.get(offset + 4)?,
            case_count: row.get(offset + 5)?,
            empty_count: row.get(offset + 6)?,
        })
    }

    /// The set's empty tally as the wire carries it: absent when no visible run
    /// reported one.
    fn reportable_empty_count(&self) -> Option<u64> {
        (self.empty_count > 0).then_some(self.empty_count as u64)
    }
}

impl Storage {
    /// Every project with at least one visible run, name-ascending.
    pub async fn list_run_sets(&self, vis: RunVisibility) -> anyhow::Result<SetsResponse> {
        self.runs.read(move |conn| list_run_sets(conn, &vis)).await
    }

    /// One project's suites. `None` — a 404 at the handler — when the project
    /// has no visible runs at all, so an invisible project is indistinguishable
    /// from one that never existed.
    pub async fn run_set_project(
        &self,
        project: String,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<ProjectSetDetailResponse>> {
        self.runs
            .read(move |conn| run_set_project(conn, &project, &vis))
            .await
    }

    /// One suite, addressed directly. `None` under the same rule as
    /// [`Storage::run_set_project`].
    pub async fn run_set_suite(
        &self,
        project: String,
        suite: String,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<SuiteSetDetailResponse>> {
        self.runs
            .read(move |conn| run_set_suite(conn, &project, &suite, &vis))
            .await
    }
}

/// A bound `TEXT` parameter, or SQL NULL when the caller left it open.
fn optional_text(value: Option<&str>) -> Value {
    match value {
        Some(text) => Value::Text(text.to_string()),
        None => Value::Null,
    }
}

/// The level to publish to this caller for `(project, suite)`.
///
/// `None` for the classes that do not ride grants at all: an admin is never
/// filtered by one, and anonymous/static-token callers can never hold one.
fn my_level(
    conn: &mut Conn<'_>,
    vis: &RunVisibility,
    project: &str,
    suite: Option<&str>,
) -> anyhow::Result<Option<GrantLevel>> {
    match vis {
        RunVisibility::User(user_id) => covering_level(conn, Some(project), suite, user_id),
        RunVisibility::Full | RunVisibility::Public => Ok(None),
    }
}

fn list_run_sets(conn: &mut Conn<'_>, vis: &RunVisibility) -> anyhow::Result<SetsResponse> {
    let mut args: Vec<Value> = Vec::new();
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT project, COUNT(DISTINCT suite), {AGGREGATES}
           FROM runs
          WHERE project IS NOT NULL AND {visible}
          GROUP BY project
          ORDER BY project ASC"
    );
    let rows: Vec<(String, i64, Aggregates)> = conn.query_map(&sql, &args, |row| {
        Ok((
            row.get::<String>(0)?,
            row.get::<i64>(1)?,
            Aggregates::read(row, 2)?,
        ))
    })?;

    let mut rates = latest_pass_rates(conn, vis)?;
    let mut projects = Vec::with_capacity(rows.len());
    for (project, suite_count, agg) in rows {
        let mut recent_pass_rates = rates.remove(&project).unwrap_or_default();
        recent_pass_rates.truncate(PROJECT_SPARK_CAP);
        projects.push(ProjectSetView {
            suite_count,
            run_count: agg.run_count,
            last_run_at: agg.last_run_at,
            pass_count: agg.pass_count,
            fail_count: agg.fail_count,
            error_count: agg.error_count,
            case_count: agg.case_count,
            empty_count: agg.reportable_empty_count(),
            recent_pass_rates,
            restricted: restricted(conn, Some(&project), None)?,
            my_level: my_level(conn, vis, &project, None)?,
            project,
        });
    }
    Ok(SetsResponse { projects })
}

/// Each visible `(project, suite)`'s newest run's pass rate, grouped by project
/// with the suites in name order.
///
/// One window-function pass over the whole visible corpus rather than a query
/// per project: the listing is unpaginated, so N+1 here would be N+1 over every
/// project on the instance.
fn latest_pass_rates(
    conn: &mut Conn<'_>,
    vis: &RunVisibility,
) -> anyhow::Result<HashMap<String, Vec<f64>>> {
    let mut args: Vec<Value> = Vec::new();
    let visible = visibility_predicate("runs", vis, &mut args);
    // `id` breaks ties so two runs sharing a `created_at` still pick one
    // deterministically.
    let sql = format!(
        "SELECT project, pass_rate FROM (
             SELECT project, suite, {PASS_RATE} AS pass_rate,
                    ROW_NUMBER() OVER (PARTITION BY project, suite
                                       ORDER BY created_at DESC, id DESC) AS rn
               FROM runs
              WHERE project IS NOT NULL AND suite IS NOT NULL AND {visible}
         ) AS ranked
          WHERE rn = 1
          ORDER BY project ASC, suite ASC"
    );
    let rows: Vec<(String, f64)> = conn.query_map(&sql, &args, |row| {
        Ok((row.get::<String>(0)?, row.get::<f64>(1)?))
    })?;
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for (project, rate) in rows {
        out.entry(project).or_default().push(rate);
    }
    Ok(out)
}

/// Whether the project has any visible run — the existence test both detail
/// endpoints 404 on.
///
/// `.query_row_opt(...)?`, never `.is_ok()`: this answer becomes a 404, so
/// swallowing the error would report "no such set" for a locked database or a
/// corrupt page, and the operator would never see the 500 that is really
/// happening.
fn project_has_visible_runs(
    conn: &mut Conn<'_>,
    project: &str,
    suite: Option<&str>,
    vis: &RunVisibility,
) -> anyhow::Result<bool> {
    let mut args: Vec<Value> = vec![project.to_string().into(), optional_text(suite)];
    let visible = visibility_predicate("runs", vis, &mut args);
    // `?2 IS NULL OR suite = ?2`: the project form ignores the suite column.
    let sql = format!(
        "SELECT 1 FROM runs
          WHERE project = ?1 AND (?2 IS NULL OR suite = ?2) AND {visible} LIMIT 1"
    );
    Ok(conn.query_row_opt(&sql, &args, |_| Ok(()))?.is_some())
}

fn run_set_project(
    conn: &mut Conn<'_>,
    project: &str,
    vis: &RunVisibility,
) -> anyhow::Result<Option<ProjectSetDetailResponse>> {
    if !project_has_visible_runs(conn, project, None, vis)? {
        return Ok(None);
    }

    let mut args: Vec<Value> = vec![project.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT suite, {AGGREGATES}
           FROM runs
          WHERE project = ?1 AND suite IS NOT NULL AND {visible}
          GROUP BY suite
          ORDER BY suite ASC"
    );
    let rows: Vec<(String, Aggregates)> = conn.query_map(&sql, &args, |row| {
        Ok((row.get::<String>(0)?, Aggregates::read(row, 1)?))
    })?;

    let mut sparklines = sparklines(conn, project, None, vis)?;
    let mut suites = Vec::with_capacity(rows.len());
    for (suite, agg) in rows {
        let sparkline = sparklines.remove(&suite).unwrap_or_default();
        suites.push(SuiteSetView {
            run_count: agg.run_count,
            last_run_at: agg.last_run_at,
            pass_count: agg.pass_count,
            fail_count: agg.fail_count,
            error_count: agg.error_count,
            case_count: agg.case_count,
            empty_count: agg.reportable_empty_count(),
            latest_pass_rate: sparkline.last().copied(),
            sparkline,
            baseline_run_id: read_baseline(conn, project, &suite, vis)?,
            restricted: restricted(conn, Some(project), Some(&suite))?,
            my_level: my_level(conn, vis, project, Some(&suite))?,
            suite,
        });
    }

    Ok(Some(ProjectSetDetailResponse {
        project: project.to_string(),
        restricted: restricted(conn, Some(project), None)?,
        my_level: my_level(conn, vis, project, None)?,
        suites,
    }))
}

fn run_set_suite(
    conn: &mut Conn<'_>,
    project: &str,
    suite: &str,
    vis: &RunVisibility,
) -> anyhow::Result<Option<SuiteSetDetailResponse>> {
    if !project_has_visible_runs(conn, project, Some(suite), vis)? {
        return Ok(None);
    }

    let mut args: Vec<Value> = vec![project.to_string().into(), suite.to_string().into()];
    let visible = visibility_predicate("runs", vis, &mut args);
    let sql =
        format!("SELECT {AGGREGATES} FROM runs WHERE project = ?1 AND suite = ?2 AND {visible}");
    let agg = conn.query_row(&sql, &args, |row| Aggregates::read(row, 0))?;

    let sparkline = sparklines(conn, project, Some(suite), vis)?
        .remove(suite)
        .unwrap_or_default();

    Ok(Some(SuiteSetDetailResponse {
        project: project.to_string(),
        suite: suite.to_string(),
        restricted: restricted(conn, Some(project), Some(suite))?,
        my_level: my_level(conn, vis, project, Some(suite))?,
        run_count: agg.run_count,
        last_run_at: agg.last_run_at,
        pass_count: agg.pass_count,
        fail_count: agg.fail_count,
        error_count: agg.error_count,
        case_count: agg.case_count,
        empty_count: agg.reportable_empty_count(),
        latest_pass_rate: sparkline.last().copied(),
        sparkline,
        baseline_run_id: read_baseline(conn, project, suite, vis)?,
    }))
}

/// Each suite's last [`SPARKLINE_RUNS`] visible pass rates, **oldest first**.
///
/// `suite: Some(_)` narrows to one suite; `None` covers the whole project in a
/// single pass, so the project detail does not issue a query per suite.
fn sparklines(
    conn: &mut Conn<'_>,
    project: &str,
    suite: Option<&str>,
    vis: &RunVisibility,
) -> anyhow::Result<HashMap<String, Vec<f64>>> {
    let mut args: Vec<Value> = vec![project.to_string().into(), optional_text(suite)];
    let visible = visibility_predicate("runs", vis, &mut args);
    let cap = SPARKLINE_RUNS;
    // Newest-first inside the window (that is what "the last 20" means), then
    // re-sorted oldest-first for the consumer.
    let sql = format!(
        "SELECT suite, pass_rate FROM (
             SELECT suite, created_at, id, {PASS_RATE} AS pass_rate,
                    ROW_NUMBER() OVER (PARTITION BY suite
                                       ORDER BY created_at DESC, id DESC) AS rn
               FROM runs
              WHERE project = ?1 AND suite IS NOT NULL
                AND (?2 IS NULL OR suite = ?2) AND {visible}
         ) AS ranked
          WHERE rn <= {cap}
          ORDER BY suite ASC, created_at ASC, id ASC"
    );
    let rows: Vec<(String, f64)> = conn.query_map(&sql, &args, |row| {
        Ok((row.get::<String>(0)?, row.get::<f64>(1)?))
    })?;
    let mut out: HashMap<String, Vec<f64>> = HashMap::new();
    for (suite, rate) in rows {
        out.entry(suite).or_default().push(rate);
    }
    Ok(out)
}
