//! Run ingest (content-hash idempotency) and run list / detail / export queries.

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};

use measurellm_core::ids::RunId;
use measurellm_core::result::{AssertStatus, RunResult};

use super::{
    compress, content_hash, decompress, encode_cursor, from_microusd, ms_to_rfc3339, now_ms,
    sha256_hex, to_microusd, IngestOutcome, Storage,
};
use crate::domain::RunStatusFilter;
use crate::dto::runs::{CaseAssertLean, RunDetailResponse, RunListItem};

impl Storage {
    /// Ingest a run in a single transaction. Idempotent by (id, content_hash).
    #[tracing::instrument(
        skip_all,
        fields(run_id = %run.run_id, project = ?run.project, suite = ?run.suite)
    )]
    pub async fn ingest_run(
        &self,
        run: RunResult,
        uploaded_by: Option<String>,
    ) -> anyhow::Result<IngestOutcome> {
        let prepared = PreparedRun::build(&run, uploaded_by)?;
        self.runs.write(move |conn| prepared.insert(conn)).await
    }

    pub async fn list_runs(&self, filter: RunListFilter) -> anyhow::Result<RunListPage> {
        self.runs.read(move |conn| filter.query(conn)).await
    }

    pub async fn get_run(&self, id: RunId) -> anyhow::Result<Option<RunDetailResponse>> {
        self.runs.read(move |conn| get_run_detail(conn, &id)).await
    }

    pub async fn export_run(&self, id: RunId) -> anyhow::Result<Option<serde_json::Value>> {
        self.runs.read(move |conn| export_run_blob(conn, &id)).await
    }

    pub async fn run_exists(&self, id: RunId) -> anyhow::Result<bool> {
        self.runs
            .read(move |conn| {
                Ok(conn
                    .query_row(
                        "SELECT 1 FROM runs WHERE id = ?1",
                        params![id.as_str()],
                        |_| Ok(()),
                    )
                    .is_ok())
            })
            .await
    }

    pub async fn delete_run(&self, id: RunId) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let n = tx.execute("DELETE FROM runs WHERE id = ?1", params![id.as_str()])?;
                tx.commit()?;
                Ok(n > 0)
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Prepared run: everything computed before we take the write lock.
// ---------------------------------------------------------------------------

// `id`/`case_key` are plain `String` here (converted from `RunId`/`CaseKey` at
// construction, below) since every field in these two structs exists only to
// be bound straight into `params![]` a few lines later — there is no further
// logic that benefits from the newtype.

struct PreparedCase {
    case_key: String,
    idx: i64,
    name: Option<String>,
    status: String,
    output_preview: Option<String>,
    output_text: Option<String>,
    output_hash: Option<String>,
    asserts_json: String,
    prompt_tokens: Option<i64>,
    completion_tokens: Option<i64>,
    cost_microusd: Option<i64>,
    latency_ms: i64,
    detail: Vec<u8>,
    tags: Vec<String>,
}

struct PreparedRun {
    id: String,
    project: Option<String>,
    suite: Option<String>,
    created_at: i64,
    schema_version: i64,
    git_branch: Option<String>,
    git_commit: Option<String>,
    git_dirty: Option<i64>,
    ci_provider: Option<String>,
    ci_run_url: Option<String>,
    case_count: i64,
    pass_count: i64,
    fail_count: i64,
    error_count: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cost_microusd: Option<i64>,
    duration_ms: i64,
    content_hash: String,
    uploaded_by: Option<String>,
    tags: Vec<String>,
    blob: Vec<u8>,
    cases: Vec<PreparedCase>,
}

impl PreparedRun {
    fn build(run: &RunResult, uploaded_by: Option<String>) -> anyhow::Result<PreparedRun> {
        let value = serde_json::to_value(run).context("serializing run")?;
        let content_hash = content_hash(&value);
        let blob = compress(&serde_json::to_vec(&value)?)?;

        let git = run.git.as_ref();
        let ci = run.ci.as_ref();
        let created_at = run.finished_at.timestamp_millis();
        let duration_ms = (run.finished_at - run.started_at).num_milliseconds().max(0);

        let mut cases = Vec::with_capacity(run.cases.len());
        for (idx, case) in run.cases.iter().enumerate() {
            let (preview, text, hash) = match &case.output {
                Some(output) => {
                    let text = output.as_text().into_owned();
                    let preview: String = text.chars().take(300).collect();
                    let hash = sha256_hex(text.as_bytes());
                    (Some(preview), Some(text), Some(hash))
                }
                None => (None, None, None),
            };
            let asserts_json = serde_json::to_string(
                &case
                    .asserts
                    .iter()
                    .map(|a| CaseAssertLean {
                        label: a.kind,
                        kind: a.kind,
                        passed: matches!(a.status, AssertStatus::Pass),
                        score: a.score,
                    })
                    .collect::<Vec<_>>(),
            )?;
            let detail = compress(&serde_json::to_vec(case)?)?;
            let usage = case.usage.as_ref();
            cases.push(PreparedCase {
                case_key: case.case_key.to_string(),
                idx: idx as i64,
                name: case.name.clone(),
                status: case.status.as_str().to_string(),
                output_preview: preview,
                output_text: text,
                output_hash: hash,
                asserts_json,
                prompt_tokens: usage.map(|u| u.input_tokens as i64),
                completion_tokens: usage.map(|u| u.output_tokens as i64),
                cost_microusd: to_microusd(case.cost_usd),
                latency_ms: case.latency_ms as i64,
                detail,
                tags: case.tags.clone(),
            });
        }

        Ok(PreparedRun {
            id: run.run_id.to_string(),
            project: run.project.clone(),
            suite: run.suite.clone(),
            created_at,
            schema_version: run.schema_version as i64,
            git_branch: git.and_then(|g| g.branch.clone()),
            git_commit: git.and_then(|g| g.commit.clone()),
            git_dirty: git.map(|g| g.dirty as i64),
            ci_provider: ci.and_then(|c| c.provider.clone()),
            ci_run_url: ci.and_then(|c| c.run_url.clone()),
            case_count: run.summary.total as i64,
            pass_count: run.summary.passed as i64,
            fail_count: run.summary.failed as i64,
            error_count: run.summary.errored as i64,
            prompt_tokens: run.summary.prompt_tokens as i64,
            completion_tokens: run.summary.completion_tokens as i64,
            cost_microusd: to_microusd(run.summary.cost_usd),
            duration_ms,
            content_hash,
            uploaded_by,
            tags: run.filters.tags.clone(),
            blob,
            cases,
        })
    }

    fn insert(self, conn: &mut Connection) -> anyhow::Result<IngestOutcome> {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT content_hash FROM runs WHERE id = ?1",
                params![self.id],
                |row| row.get(0),
            )
            .ok();
        if let Some(existing_hash) = existing {
            return Ok(if existing_hash == self.content_hash {
                IngestOutcome::Existing
            } else {
                IngestOutcome::Conflict
            });
        }

        tx.execute(
            "INSERT INTO runs (
                id, project, suite, created_at, uploaded_at, schema_version, description,
                git_branch, git_commit, git_dirty, ci_provider, ci_run_url,
                case_count, pass_count, fail_count, error_count,
                prompt_tokens, completion_tokens, cost_microusd, duration_ms,
                content_hash, uploaded_by
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20,
                ?21, ?22
            )",
            params![
                self.id,
                self.project,
                self.suite,
                self.created_at,
                now_ms(),
                self.schema_version,
                Option::<String>::None,
                self.git_branch,
                self.git_commit,
                self.git_dirty,
                self.ci_provider,
                self.ci_run_url,
                self.case_count,
                self.pass_count,
                self.fail_count,
                self.error_count,
                self.prompt_tokens,
                self.completion_tokens,
                self.cost_microusd,
                self.duration_ms,
                self.content_hash,
                self.uploaded_by,
            ],
        )?;

        tx.execute(
            "INSERT INTO run_blobs (run_id, encoding, body) VALUES (?1, 'zstd', ?2)",
            params![self.id, self.blob],
        )?;

        for tag in &self.tags {
            tx.execute(
                "INSERT OR IGNORE INTO run_tags (run_id, tag) VALUES (?1, ?2)",
                params![self.id, tag],
            )?;
        }

        for case in &self.cases {
            tx.execute(
                "INSERT OR IGNORE INTO cases (
                    run_id, case_key, idx, name, status, output_preview, output_text,
                    output_hash, asserts, prompt_tokens, completion_tokens, cost_microusd,
                    latency_ms, detail
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    self.id,
                    case.case_key,
                    case.idx,
                    case.name,
                    case.status,
                    case.output_preview,
                    case.output_text,
                    case.output_hash,
                    case.asserts_json,
                    case.prompt_tokens,
                    case.completion_tokens,
                    case.cost_microusd,
                    case.latency_ms,
                    case.detail,
                ],
            )?;
            for tag in &case.tags {
                tx.execute(
                    "INSERT OR IGNORE INTO case_tags (run_id, case_key, tag) VALUES (?1, ?2, ?3)",
                    params![self.id, case.case_key, tag],
                )?;
            }
        }

        tx.commit()?;
        Ok(IngestOutcome::Created)
    }
}

// ---------------------------------------------------------------------------
// Run listing
// ---------------------------------------------------------------------------

/// Filters for `GET /runs`.
#[derive(Debug, Clone, Default)]
pub struct RunListFilter {
    pub project: Option<String>,
    pub suite: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub status: Option<RunStatusFilter>,
    pub limit: i64,
    pub cursor: Option<(i64, RunId)>,
}

/// A page of run summaries plus an optional next cursor.
pub struct RunListPage {
    pub runs: Vec<RunListItem>,
    pub next_cursor: Option<String>,
}

impl RunListFilter {
    fn query(self, conn: &Connection) -> anyhow::Result<RunListPage> {
        let mut sql = String::from(
            "SELECT id, project, suite, created_at, git_branch, git_commit, git_dirty,
                    case_count, pass_count, fail_count, error_count,
                    prompt_tokens, completion_tokens, cost_microusd, duration_ms
             FROM runs",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(project) = &self.project {
            clauses.push(format!("project = ?{}", args.len() + 1));
            args.push(project.clone().into());
        }
        if let Some(suite) = &self.suite {
            clauses.push(format!("suite = ?{}", args.len() + 1));
            args.push(suite.clone().into());
        }
        if let Some(branch) = &self.branch {
            clauses.push(format!("git_branch = ?{}", args.len() + 1));
            args.push(branch.clone().into());
        }
        if let Some(since) = self.since_ms {
            clauses.push(format!("created_at >= ?{}", args.len() + 1));
            args.push(since.into());
        }
        if let Some(until) = self.until_ms {
            clauses.push(format!("created_at <= ?{}", args.len() + 1));
            args.push(until.into());
        }
        if let Some(tag) = &self.tag {
            clauses.push(format!(
                "id IN (SELECT run_id FROM run_tags WHERE tag = ?{})",
                args.len() + 1
            ));
            args.push(tag.clone().into());
        }
        match self.status {
            Some(RunStatusFilter::Fail) => clauses.push("fail_count > 0".into()),
            Some(RunStatusFilter::Error) => clauses.push("error_count > 0".into()),
            Some(RunStatusFilter::Pass) => {
                clauses.push("fail_count = 0 AND error_count = 0".into())
            }
            None => {}
        }
        if let Some((c_created, c_id)) = &self.cursor {
            clauses.push(format!(
                "(created_at < ?{a} OR (created_at = ?{a} AND id < ?{b}))",
                a = args.len() + 1,
                b = args.len() + 2
            ));
            args.push((*c_created).into());
            args.push(c_id.as_str().to_string().into());
        }

        if !clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&clauses.join(" AND "));
        }
        sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
        // fetch one extra to detect a next page
        let fetch = self.limit + 1;
        args.push(fetch.into());
        let limit_idx = args.len();
        sql = sql.replacen("LIMIT ?", &format!("LIMIT ?{limit_idx}"), 1);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
            Ok(RunRow {
                id: row.get(0)?,
                project: row.get(1)?,
                suite: row.get(2)?,
                created_at: row.get(3)?,
                git_branch: row.get(4)?,
                git_commit: row.get(5)?,
                git_dirty: row.get(6)?,
                case_count: row.get(7)?,
                pass_count: row.get(8)?,
                fail_count: row.get(9)?,
                error_count: row.get(10)?,
                prompt_tokens: row.get(11)?,
                completion_tokens: row.get(12)?,
                cost_microusd: row.get(13)?,
                duration_ms: row.get(14)?,
            })
        })?;
        let mut collected: Vec<RunRow> = Vec::new();
        for row in rows {
            collected.push(row?);
        }

        let mut next_cursor = None;
        if collected.len() as i64 > self.limit {
            let last = collected.pop().unwrap();
            let anchor = collected.last().unwrap_or(&last);
            next_cursor = Some(encode_cursor(
                anchor.created_at,
                &RunId::new(anchor.id.as_str()),
            ));
        }

        let mut out = Vec::with_capacity(collected.len());
        for run in &collected {
            let tags = load_run_tags(conn, &run.id)?;
            out.push(run.to_dto(tags));
        }

        Ok(RunListPage {
            runs: out,
            next_cursor,
        })
    }
}

struct RunRow {
    id: String,
    project: Option<String>,
    suite: Option<String>,
    created_at: i64,
    git_branch: Option<String>,
    git_commit: Option<String>,
    git_dirty: Option<i64>,
    case_count: i64,
    pass_count: i64,
    fail_count: i64,
    error_count: i64,
    prompt_tokens: i64,
    completion_tokens: i64,
    cost_microusd: Option<i64>,
    duration_ms: i64,
}

impl RunRow {
    fn to_dto(&self, tags: Vec<String>) -> RunListItem {
        let pass_rate = if self.case_count > 0 {
            self.pass_count as f64 / self.case_count as f64
        } else {
            0.0
        };
        RunListItem {
            id: RunId::new(self.id.as_str()),
            project: self.project.clone(),
            suite: self.suite.clone(),
            created_at: ms_to_rfc3339(self.created_at),
            git_branch: self.git_branch.clone(),
            git_commit: self.git_commit.clone(),
            git_dirty: self.git_dirty.map(|d| d != 0),
            case_count: self.case_count,
            pass_count: self.pass_count,
            fail_count: self.fail_count,
            error_count: self.error_count,
            pass_rate,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            cost_usd: from_microusd(self.cost_microusd),
            duration_ms: self.duration_ms,
            tags,
        }
    }
}

pub(super) fn load_run_tags(conn: &Connection, run_id: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT tag FROM run_tags WHERE run_id = ?1 ORDER BY tag")?;
    let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ---------------------------------------------------------------------------
// Run detail & export
// ---------------------------------------------------------------------------

fn get_run_detail(conn: &Connection, id: &RunId) -> anyhow::Result<Option<RunDetailResponse>> {
    let row = conn
        .query_row(
            "SELECT id, project, suite, created_at, uploaded_at, schema_version,
                    git_branch, git_commit, git_dirty, ci_provider, ci_run_url,
                    case_count, pass_count, fail_count, error_count,
                    prompt_tokens, completion_tokens, cost_microusd, duration_ms,
                    content_hash, uploaded_by
             FROM runs WHERE id = ?1",
            params![id.as_str()],
            |row| {
                Ok(RunDetailResponse {
                    id: RunId::new(row.get::<_, String>(0)?),
                    project: row.get::<_, Option<String>>(1)?,
                    suite: row.get::<_, Option<String>>(2)?,
                    created_at: ms_to_rfc3339(row.get::<_, i64>(3)?),
                    uploaded_at: ms_to_rfc3339(row.get::<_, i64>(4)?),
                    schema_version: row.get::<_, i64>(5)?,
                    git_branch: row.get::<_, Option<String>>(6)?,
                    git_commit: row.get::<_, Option<String>>(7)?,
                    git_dirty: row.get::<_, Option<i64>>(8)?.map(|d| d != 0),
                    ci_provider: row.get::<_, Option<String>>(9)?,
                    ci_run_url: row.get::<_, Option<String>>(10)?,
                    case_count: row.get::<_, i64>(11)?,
                    pass_count: row.get::<_, i64>(12)?,
                    fail_count: row.get::<_, i64>(13)?,
                    error_count: row.get::<_, i64>(14)?,
                    prompt_tokens: row.get::<_, i64>(15)?,
                    completion_tokens: row.get::<_, i64>(16)?,
                    cost_usd: from_microusd(row.get::<_, Option<i64>>(17)?),
                    duration_ms: row.get::<_, i64>(18)?,
                    content_hash: row.get::<_, String>(19)?,
                    uploaded_by: row.get::<_, Option<String>>(20)?,
                    // Filled in below, after the row is loaded.
                    tags: Vec::new(),
                    assert_labels: Vec::new(),
                })
            },
        )
        .ok();

    let Some(mut detail) = row else {
        return Ok(None);
    };

    detail.tags = load_run_tags(conn, id.as_str())?;
    detail.assert_labels = distinct_assert_labels(conn, id.as_str())?;
    Ok(Some(detail))
}

fn distinct_assert_labels(conn: &Connection, run_id: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT asserts FROM cases WHERE run_id = ?1")?;
    let rows = stmt.query_map(params![run_id], |row| row.get::<_, Option<String>>(0))?;
    let mut seen: Vec<String> = Vec::new();
    for row in rows {
        let Some(json) = row? else { continue };
        // Graceful degrade (same rationale as `CaseListFilter::query` in
        // storage/cases.rs): rows written by this codebase always serialize as
        // a `Vec<CaseAssertLean>`, so a parse failure here means a hand-tampered
        // or corrupt `asserts` blob. Treat it as empty (contributing no labels)
        // and warn rather than failing the whole run detail.
        let parsed: Vec<CaseAssertLean> = serde_json::from_str(&json).unwrap_or_else(|e| {
            tracing::warn!(
                run_id = %run_id,
                error = %e,
                "unparseable stored asserts; treating as empty"
            );
            Vec::new()
        });
        for a in parsed {
            let label = a.label.as_str();
            if !seen.iter().any(|s| s == label) {
                seen.push(label.to_string());
            }
        }
    }
    seen.sort();
    Ok(seen)
}

fn export_run_blob(conn: &Connection, id: &RunId) -> anyhow::Result<Option<serde_json::Value>> {
    let blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT body FROM run_blobs WHERE run_id = ?1",
            params![id.as_str()],
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
