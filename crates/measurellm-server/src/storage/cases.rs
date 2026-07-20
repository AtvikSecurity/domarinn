//! Case list (lean rows) and case detail (blob decompress).

use rusqlite::{params, Connection};

use measurellm_core::result::CaseStatus;

use super::{decompress, from_microusd, Storage};

impl Storage {
    pub async fn list_cases(&self, filter: CaseListFilter) -> anyhow::Result<serde_json::Value> {
        self.runs.read(move |conn| filter.query(conn)).await
    }

    pub async fn get_case(
        &self,
        run_id: String,
        case_key: String,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        self.runs
            .read(move |conn| get_case_detail(conn, &run_id, &case_key))
            .await
    }
}

/// Filters for `GET /runs/{id}/cases`.
#[derive(Debug, Clone, Default)]
pub struct CaseListFilter {
    pub run_id: String,
    pub status: Option<CaseStatus>,
    pub tag: Option<String>,
    pub q: Option<String>,
    pub limit: i64,
    pub cursor: Option<i64>,
}

impl CaseListFilter {
    fn query(self, conn: &Connection) -> anyhow::Result<serde_json::Value> {
        let mut sql = String::from(
            "SELECT case_key, idx, name, status, output_preview, asserts,
                    prompt_tokens, completion_tokens, cost_microusd, latency_ms
             FROM cases WHERE run_id = ?1",
        );
        let mut args: Vec<rusqlite::types::Value> = vec![self.run_id.clone().into()];
        if let Some(status) = self.status {
            args.push(status.as_str().to_string().into());
            sql.push_str(&format!(" AND status = ?{}", args.len()));
        }
        if let Some(tag) = &self.tag {
            args.push(tag.clone().into());
            sql.push_str(&format!(
                " AND case_key IN (SELECT case_key FROM case_tags WHERE run_id = ?1 AND tag = ?{})",
                args.len()
            ));
        }
        if let Some(q) = &self.q {
            let like = format!("%{}%", q);
            args.push(like.clone().into());
            let a = args.len();
            args.push(like.into());
            let b = args.len();
            sql.push_str(&format!(" AND (output_text LIKE ?{a} OR name LIKE ?{b})"));
        }
        if let Some(cursor) = self.cursor {
            args.push(cursor.into());
            sql.push_str(&format!(" AND idx > ?{}", args.len()));
        }
        args.push((self.limit + 1).into());
        sql.push_str(&format!(" ORDER BY idx ASC LIMIT ?{}", args.len()));

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            let asserts_str: Option<String> = row.get(5)?;
            let asserts: serde_json::Value = asserts_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!([]));
            Ok((
                row.get::<_, i64>(1)?, // idx
                serde_json::json!({
                    "case_key": row.get::<_, String>(0)?,
                    "idx": row.get::<_, i64>(1)?,
                    "name": row.get::<_, Option<String>>(2)?,
                    "status": row.get::<_, String>(3)?,
                    "output_preview": row.get::<_, Option<String>>(4)?,
                    "asserts": asserts,
                    "prompt_tokens": row.get::<_, Option<i64>>(6)?,
                    "completion_tokens": row.get::<_, Option<i64>>(7)?,
                    "cost_usd": from_microusd(row.get::<_, Option<i64>>(8)?),
                    "latency_ms": row.get::<_, Option<i64>>(9)?,
                }),
            ))
        })?;
        let mut collected: Vec<(i64, serde_json::Value)> = Vec::new();
        for row in rows {
            collected.push(row?);
        }

        let mut next_cursor = None;
        if collected.len() as i64 > self.limit {
            collected.pop();
            if let Some((idx, _)) = collected.last() {
                next_cursor = Some(idx.to_string());
            }
        }

        let cases: Vec<serde_json::Value> = collected.into_iter().map(|(_, v)| v).collect();
        Ok(serde_json::json!({
            "cases": cases,
            "next_cursor": next_cursor,
        }))
    }
}

fn get_case_detail(
    conn: &Connection,
    run_id: &str,
    case_key: &str,
) -> anyhow::Result<Option<serde_json::Value>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT detail FROM cases WHERE run_id = ?1 AND case_key = ?2",
            params![run_id, case_key],
            |row| row.get(0),
        )
        .ok();
    let Some(blob) = blob else {
        return Ok(None);
    };
    let bytes = decompress(&blob)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(Some(value))
}
