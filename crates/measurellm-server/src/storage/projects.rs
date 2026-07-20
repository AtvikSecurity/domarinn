//! Projects, suites (with a recent pass-rate series), and baseline management.

use rusqlite::{params, Connection, TransactionBehavior};

use measurellm_core::ids::RunId;

use super::{ms_to_rfc3339, now_ms, Storage};

impl Storage {
    pub async fn list_projects(&self) -> anyhow::Result<serde_json::Value> {
        self.runs.read(list_projects).await
    }

    pub async fn list_suites(&self, project: String) -> anyhow::Result<serde_json::Value> {
        self.runs
            .read(move |conn| list_suites(conn, &project))
            .await
    }

    pub async fn set_baseline(
        &self,
        project: String,
        suite: String,
        run_id: RunId,
    ) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let exists = tx
                    .query_row(
                        "SELECT 1 FROM runs WHERE id = ?1",
                        params![run_id.as_str()],
                        |_| Ok(()),
                    )
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

fn list_projects(conn: &Connection) -> anyhow::Result<serde_json::Value> {
    let mut stmt = conn.prepare(
        "SELECT project,
                COUNT(*) AS run_count,
                COUNT(DISTINCT suite) AS suite_count,
                MAX(created_at) AS last_run_at
         FROM runs
         WHERE project IS NOT NULL
         GROUP BY project
         ORDER BY last_run_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "project": row.get::<_, String>(0)?,
            "run_count": row.get::<_, i64>(1)?,
            "suite_count": row.get::<_, i64>(2)?,
            "last_run_at": ms_to_rfc3339(row.get::<_, i64>(3)?),
        }))
    })?;
    let projects: Vec<serde_json::Value> = rows.collect::<Result<_, _>>()?;
    Ok(serde_json::json!({ "projects": projects }))
}

fn list_suites(conn: &Connection, project: &str) -> anyhow::Result<serde_json::Value> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT suite FROM runs WHERE project = ?1 AND suite IS NOT NULL")?;
    let suite_names: Vec<String> = stmt
        .query_map(params![project], |row| row.get::<_, String>(0))?
        .collect::<Result<_, _>>()?;

    let mut suites = Vec::new();
    for suite in suite_names {
        let mut series_stmt = conn.prepare(
            "SELECT id, created_at, case_count, pass_count
             FROM runs WHERE project = ?1 AND suite = ?2
             ORDER BY created_at DESC LIMIT 20",
        )?;
        let series: Vec<serde_json::Value> = series_stmt
            .query_map(params![project, suite], |row| {
                let case_count: i64 = row.get(2)?;
                let pass_count: i64 = row.get(3)?;
                let pass_rate = if case_count > 0 {
                    pass_count as f64 / case_count as f64
                } else {
                    0.0
                };
                Ok(serde_json::json!({
                    "run_id": row.get::<_, String>(0)?,
                    "created_at": ms_to_rfc3339(row.get::<_, i64>(1)?),
                    "total": case_count,
                    "passed": pass_count,
                    "pass_rate": pass_rate,
                }))
            })?
            .collect::<Result<_, _>>()?;

        let baseline_run_id: Option<String> = conn
            .query_row(
                "SELECT run_id FROM baselines WHERE project = ?1 AND suite = ?2",
                params![project, suite],
                |row| row.get(0),
            )
            .ok();

        let last_run_at = series.first().and_then(|s| s.get("created_at").cloned());

        suites.push(serde_json::json!({
            "suite": suite,
            "run_count": series.len(),
            "last_run_at": last_run_at,
            "baseline_run_id": baseline_run_id,
            "series": series,
        }));
    }

    Ok(serde_json::json!({
        "project": project,
        "suites": suites,
    }))
}
