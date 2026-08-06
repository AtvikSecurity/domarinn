//! Run ingest (content-hash idempotency) and run list / detail / export queries.

use std::collections::BTreeMap;

use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use domarinn_core::ids::RunId;
use domarinn_core::result::{AssertStatus, RunResult};

use super::{
    compress, content_hash, decompress, empty_to_none, encode_cursor, from_microusd, ms_to_rfc3339,
    now_ms, sha256_hex, to_microusd, IngestOutcome, Storage,
};
use crate::domain::{CachedFilter, OriginFilter, RunStatusFilter};
use crate::dto::config::RunConfigResponse;
use crate::dto::runs::{CaseAssertLean, RunDetailResponse, RunListItem};
use crate::runsets::{visibility_predicate, visible_run_predicate, RunVisibility};

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

    /// A run's detail, or `None` when it does not exist **or** is invisible to
    /// this caller. The two are deliberately indistinguishable: the handler
    /// renders both as a 404, so a restricted run leaks nothing by existing.
    pub async fn get_run(
        &self,
        id: RunId,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<RunDetailResponse>> {
        self.runs
            .read(move |conn| get_run_detail(conn, &id, &vis))
            .await
    }

    pub async fn export_run(
        &self,
        id: RunId,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        self.runs
            .read(move |conn| export_run_blob(conn, &id, &vis))
            .await
    }

    pub async fn get_run_config(
        &self,
        id: RunId,
        vis: RunVisibility,
    ) -> anyhow::Result<Option<RunConfigResponse>> {
        self.runs
            .read(move |conn| run_config_blob(conn, &id, &vis))
            .await
    }

    /// Whether the run exists *and* this caller may see it — the gate `/cases`
    /// and `/matrix` share. `.optional()?`, never `.is_ok()` (see `sets`).
    pub async fn run_exists(&self, id: RunId, vis: RunVisibility) -> anyhow::Result<bool> {
        self.runs
            .read(move |conn| {
                let mut args: Vec<rusqlite::types::Value> = vec![id.as_str().to_string().into()];
                let clause = visibility_predicate("runs", &vis, &mut args);
                let sql = format!("SELECT 1 FROM runs WHERE id = ?1 AND {clause}");
                let found = conn
                    .query_row(&sql, rusqlite::params_from_iter(args.iter()), |_| Ok(()))
                    .optional()?;
                Ok(found.is_some())
            })
            .await
    }

    pub async fn delete_run(&self, id: RunId) -> anyhow::Result<bool> {
        self.runs
            .write(move |conn| {
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let n = tx.execute("DELETE FROM runs WHERE id = ?1", params![id.as_str()])?;
                // FTS virtual tables have no FK cascade; drop the search rows
                // in the same transaction.
                tx.execute(
                    "DELETE FROM runs_fts WHERE run_id = ?1",
                    params![id.as_str()],
                )?;
                tx.execute(
                    "DELETE FROM cases_fts WHERE run_id = ?1",
                    params![id.as_str()],
                )?;
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
    // Migration-6 column: whether the provider response was a cache hit,
    // promoted so the case list can filter to fresh responses at SQL level.
    cached: bool,
    // Migration-13 column: the key this case's provider call was addressed by.
    // Plain NULL when absent, no sentinel — the field is new, so absence means
    // "had no key" rather than "not yet backfilled".
    cache_key: Option<String>,
    // Migration-3 cell columns, promoted out of the `detail` blob so the matrix
    // views can filter/join without decompressing every row.
    provider_id: String,
    prompt_id: Option<String>,
    test_id: String,
    repeat_idx: i64,
    score: f64,
    stop_reason: Option<String>,
    tags: Vec<String>,
    // Search-index text that has no `cases` column of its own (`cases_fts`
    // is written in the same transaction; see `storage::search`).
    prompt_text: Option<String>,
    /// Also promoted to a `cases` column (migration 7), because an errored case
    /// has no output and therefore no `output_preview` to show in the grid.
    error: Option<String>,
    // Migration-9 columns: component identity, promoted so `compare` can
    // classify what moved from columns alone.
    prompt_digest: Option<String>,
    provider_digest: Option<String>,
    assert_digest: Option<String>,
    /// Migration-10 column: the structured failure class beside the prose.
    error_class: Option<String>,
    /// Migration-15 column: why the output was empty. `String`, not
    /// `Option<String>`, because the column is a tri-state whose NULL is
    /// reserved for "not yet backfilled" — ingest writes `''` for "no reason"
    /// (see `storage::backfill`).
    empty_reason: String,
    /// Migration-17 column: the provider that actually answered, when a
    /// fallback stood in for the configured one. `provider_id` above stays the
    /// configured provider (matrix columns and `case_key` joins depend on it),
    /// so this is the only record of who spent the tokens. `String` for the
    /// same tri-state reason as `empty_reason`: `''` means "the configured
    /// provider answered", NULL is reserved for "not yet backfilled".
    answered_by_provider_id: String,
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
    config_digest: String,
    /// The run's human label: `origin.note` (from `--note`, itself falling back
    /// to the suite's `description`), or the snapshot's `description` for runs
    /// from clients that predate provenance collection. Feeds both the
    /// `description` column and its `runs_fts` slot, which existed and were
    /// never populated until now.
    ///
    /// Forward-only: existing rows keep their NULL and stay unsearchable by
    /// note, even though `config_snapshot.description` is sitting in their
    /// blobs. Backfilling would mean decompressing every historical run at
    /// startup for a search convenience — the same trade migration 9 declines.
    description: Option<String>,
    // Migration-8 columns, promoted from `RunOrigin` so the run list can render
    // and filter on who produced a run without decompressing every blob.
    actor: Option<String>,
    host: Option<String>,
    domarinn_version: Option<String>,
    // Migration-9 columns, promoted from `ConfigDigests`.
    prompts_digest: Option<String>,
    providers_digest: Option<String>,
    tests_digest: Option<String>,
    asserts_digest: Option<String>,
    grader_digest: Option<String>,
    // Migration-6 columns, promoted from `RunSummary` so the run list can
    // filter fully-cached CI runs at SQL level.
    cache_hits: i64,
    cache_misses: i64,
    // Migration-12 columns. `Option` rather than `i64`, so a run that reported
    // no cache activity stays distinguishable from one recorded before the
    // columns existed — the whole reason there is no backfill.
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cache_savings_microusd: Option<i64>,
    grader_cost_microusd: Option<i64>,
    /// Migration-15 column: how many of this run's cases came back empty.
    /// Counted off the cases rather than read from `summary.empty_counts` so an
    /// older client that omits the summary field still lands the right number.
    /// A present-but-empty reason (`""`, only reachable from a hand-authored
    /// document) does not count: it stores as the `''` "known: not empty"
    /// sentinel, which the detail `GROUP BY` and the case grid both exclude.
    empty_count: i64,
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
                cached: case.cached,
                cache_key: case.cache_key.clone(),
                provider_id: case.cell.provider_id.clone(),
                prompt_id: case.cell.prompt_id.clone(),
                test_id: case.cell.test_id.clone(),
                repeat_idx: case.cell.repeat as i64,
                score: case.score,
                stop_reason: case.stop_reason.clone(),
                tags: case.tags.clone(),
                prompt_text: case.prompt.as_ref().map(super::search::flatten_prompt),
                error: case.error.clone(),
                prompt_digest: case.prompt_digest.clone(),
                provider_digest: case.provider_digest.clone(),
                assert_digest: case.assert_digest.clone(),
                error_class: case.error_class.as_ref().map(|c| c.as_str().to_string()),
                empty_reason: case
                    .empty_reason
                    .as_ref()
                    .map(|r| r.as_str().to_string())
                    .unwrap_or_default(),
                answered_by_provider_id: case.answered_by_provider_id.clone().unwrap_or_default(),
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
            description: run
                .origin
                .as_ref()
                .and_then(|o| o.note.clone())
                .or_else(|| {
                    run.config_snapshot
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(str::to_string)
                })
                .filter(|d| !d.trim().is_empty()),
            actor: run.origin.as_ref().and_then(|o| o.actor.clone()),
            host: run.origin.as_ref().and_then(|o| o.host.clone()),
            domarinn_version: run.origin.as_ref().and_then(|o| o.version.clone()),
            prompts_digest: run.digests.as_ref().and_then(|d| d.prompts.clone()),
            providers_digest: run.digests.as_ref().and_then(|d| d.providers.clone()),
            tests_digest: run.digests.as_ref().and_then(|d| d.tests.clone()),
            asserts_digest: run.digests.as_ref().and_then(|d| d.asserts.clone()),
            grader_digest: run.digests.as_ref().and_then(|d| d.grader.clone()),
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
            config_digest: run.config_digest.clone(),
            cache_hits: run.summary.cache_hits as i64,
            cache_misses: run.summary.cache_misses as i64,
            // Zero means "reported, and it was zero"; the column stays NULL
            // only for rows written before migration 12.
            cache_read_tokens: Some(run.summary.cache_read_tokens as i64),
            cache_write_tokens: Some(run.summary.cache_write_tokens as i64),
            cache_savings_microusd: to_microusd(run.summary.cache_savings_usd),
            grader_cost_microusd: to_microusd(run.summary.grader_cost_usd),
            empty_count: run
                .cases
                .iter()
                .filter(|c| {
                    c.empty_reason
                        .as_ref()
                        .is_some_and(|r| !r.as_str().is_empty())
                })
                .count() as i64,
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
                content_hash, uploaded_by, config_digest, cache_hits, cache_misses,
                actor, host, domarinn_version,
                prompts_digest, providers_digest, tests_digest, asserts_digest, grader_digest,
                cache_read_tokens, cache_write_tokens, cache_savings_microusd, grader_cost_microusd,
                empty_count
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16,
                ?17, ?18, ?19, ?20,
                ?21, ?22, ?23, ?24, ?25,
                ?26, ?27, ?28,
                ?29, ?30, ?31, ?32, ?33,
                ?34, ?35, ?36, ?37,
                ?38
            )",
            params![
                self.id,
                self.project,
                self.suite,
                self.created_at,
                now_ms(),
                self.schema_version,
                self.description,
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
                self.config_digest,
                self.cache_hits,
                self.cache_misses,
                self.actor,
                self.host,
                self.domarinn_version,
                self.prompts_digest,
                self.providers_digest,
                self.tests_digest,
                self.asserts_digest,
                self.grader_digest,
                self.cache_read_tokens,
                self.cache_write_tokens,
                self.cache_savings_microusd,
                self.grader_cost_microusd,
                self.empty_count,
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
                    latency_ms, detail,
                    provider_id, prompt_id, test_id, repeat_idx, score, stop_reason, cached,
                    error, prompt_digest, provider_digest, assert_digest, error_class,
                    cache_key, empty_reason, answered_by_provider_id
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27,
                    ?28, ?29
                )",
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
                    case.provider_id,
                    case.prompt_id,
                    case.test_id,
                    case.repeat_idx,
                    case.score,
                    case.stop_reason,
                    case.cached as i64,
                    // '' rather than NULL for "no error": NULL is reserved for
                    // "not yet backfilled" (see storage::backfill).
                    case.error.as_deref().unwrap_or_default(),
                    // Plain NULL here, unlike `error` above: these columns are
                    // never backfilled, so NULL is unambiguous — the client did
                    // not record a digest — and a sentinel would add a state
                    // with no reader.
                    case.prompt_digest,
                    case.provider_digest,
                    case.assert_digest,
                    case.error_class,
                    case.cache_key,
                    // Already `''` rather than NULL for "no reason", for the
                    // same reason `error` above is (see storage::backfill).
                    case.empty_reason,
                    // Same again: `''` means "the configured provider answered",
                    // NULL means "not yet backfilled".
                    case.answered_by_provider_id,
                ],
            )?;
            for tag in &case.tags {
                tx.execute(
                    "INSERT OR IGNORE INTO case_tags (run_id, case_key, tag) VALUES (?1, ?2, ?3)",
                    params![self.id, case.case_key, tag],
                )?;
            }
            super::search::index_case(
                &tx,
                &super::search::CaseFtsRow {
                    run_id: &self.id,
                    case_key: &case.case_key,
                    name: case.name.as_deref(),
                    prompt_text: case.prompt_text.as_deref(),
                    output_text: case.output_text.as_deref(),
                    error: case.error.as_deref(),
                    tags: &case.tags,
                },
            )?;
        }

        super::search::index_run(
            &tx,
            &super::search::RunFtsRow {
                run_id: &self.id,
                project: self.project.as_deref(),
                suite: self.suite.as_deref(),
                branch: self.git_branch.as_deref(),
                commit: self.git_commit.as_deref(),
                description: self.description.as_deref(),
                tags: &self.tags,
            },
        )?;

        tx.commit()?;
        Ok(IngestOutcome::Created)
    }
}

// ---------------------------------------------------------------------------
// Run listing
// ---------------------------------------------------------------------------

/// Filters for `GET /runs`.
///
/// `visibility` is required rather than optional: there is no safe default, so
/// every construction site has to decide what the caller may see.
#[derive(Debug, Clone)]
pub struct RunListFilter {
    pub visibility: RunVisibility,
    pub project: Option<String>,
    pub suite: Option<String>,
    pub tag: Option<String>,
    pub branch: Option<String>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
    pub status: Option<RunStatusFilter>,
    pub cached: Option<CachedFilter>,
    /// CI vs developer runs — the facet that separates the canonical stream
    /// from iteration noise.
    pub origin: Option<OriginFilter>,
    /// Matches either the recorded `origin.actor` (who ran it) or the
    /// authenticated `uploaded_by` (who pushed it). They answer different
    /// questions and are often different values — a filter that matched only
    /// one would silently miss half the runs a person is responsible for.
    pub actor: Option<String>,
    pub limit: i64,
    pub cursor: Option<(i64, RunId)>,
}

/// A page of run summaries plus an optional next cursor.
pub struct RunListPage {
    pub runs: Vec<RunListItem>,
    pub next_cursor: Option<String>,
    /// How many runs the `cached=exclude` filter suppressed across the whole
    /// filtered set. Computed only on the first page of an `exclude` query.
    pub cached_hidden: Option<i64>,
}

/// A run whose every provider call was a cache hit. NULL-safe by construction:
/// `NULL = 0` and `NULL > 0` are both NULL (never true), so legacy rows and the
/// -1 undecodable-blob sentinel fail these checks and "unknown" never counts as
/// cached.
///
/// Written without `COALESCE` deliberately. It is exactly equivalent —
/// `COALESCE(x, -1) = 0` is true iff `x = 0` and `x` is not null, which is what
/// `x = 0` already means — but a function call around the column defeats index
/// matching, and this predicate runs an unbounded `COUNT(*)` over the whole
/// `runs` table on every first page of the default view (see `cached_hidden`
/// below). The bare form lets migration 11's partial index serve it.
const FULLY_CACHED: &str = "cache_misses = 0 AND cache_hits > 0";

impl RunListFilter {
    fn query(self, conn: &Connection) -> anyhow::Result<RunListPage> {
        let mut sql = String::from(
            "SELECT id, project, suite, created_at, git_branch, git_commit, git_dirty,
                    case_count, pass_count, fail_count, error_count,
                    prompt_tokens, completion_tokens, cost_microusd, duration_ms,
                    cache_hits, cache_misses,
                    actor, host, uploaded_by, ci_provider, ci_run_url,
                    description, domarinn_version, empty_count
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
        match self.origin {
            Some(OriginFilter::Ci) => clauses.push("ci_provider IS NOT NULL".into()),
            Some(OriginFilter::Local) => clauses.push("ci_provider IS NULL".into()),
            None => {}
        }
        if let Some(actor) = &self.actor {
            clauses.push(format!(
                "(actor = ?{a} OR uploaded_by = ?{a})",
                a = args.len() + 1
            ));
            args.push(actor.clone().into());
        }
        match self.status {
            Some(RunStatusFilter::Fail) => clauses.push("fail_count > 0".into()),
            Some(RunStatusFilter::Error) => clauses.push("error_count > 0".into()),
            Some(RunStatusFilter::Pass) => {
                clauses.push("fail_count = 0 AND error_count = 0".into())
            }
            None => {}
        }

        // Access control, appended with the rest of the filters so the
        // `cached_hidden` count below covers exactly the same rows the page
        // does — a count that included invisible runs would be a disclosure.
        clauses.push(visibility_predicate("runs", &self.visibility, &mut args));

        // Every fully-cached run, whatever its verdict. This used to spare the
        // ones that failed, on the reasoning that verdicts are not cached — a
        // claim that stopped being true in 0.5.0, when graders moved into the
        // same cache and key space as provider calls. See `cached_hidden_sql`
        // for the full account, including the two paths that really can move a
        // verdict inside a fully-cached run.
        let hidden_predicate = format!("({FULLY_CACHED})");
        // Count what `exclude` suppresses BEFORE the cached/cursor clauses land
        // (first page only — the count spans the whole filtered set anyway).
        let cached_hidden = if self.cached == Some(CachedFilter::Exclude) && self.cursor.is_none() {
            let mut count_sql = format!("SELECT COUNT(*) FROM runs WHERE {hidden_predicate}");
            for clause in &clauses {
                count_sql.push_str(" AND ");
                count_sql.push_str(clause);
            }
            let n: i64 =
                conn.query_row(&count_sql, rusqlite::params_from_iter(args.iter()), |row| {
                    row.get(0)
                })?;
            Some(n)
        } else {
            None
        };
        match self.cached {
            // `IS NOT 1` rather than `NOT (...)`, and it is not a style choice:
            // a legacy row has NULL cache counters, so the predicate evaluates
            // to NULL and `NOT NULL` is NULL — which SQLite filters out, hiding
            // exactly the rows the "never hide what we cannot classify" rule
            // exists to protect. `IS NOT 1` is NULL-safe: NULL IS NOT 1 is true.
            // (Pinned by `legacy_null_cache_columns_are_never_hidden`, which
            // caught this the moment the COALESCE wrapper came off.)
            Some(CachedFilter::Exclude) => clauses.push(format!("{hidden_predicate} IS NOT 1")),
            Some(CachedFilter::Only) => clauses.push(format!("({FULLY_CACHED})")),
            Some(CachedFilter::All) | None => {}
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
                cache_hits: row.get(15)?,
                cache_misses: row.get(16)?,
                actor: row.get(17)?,
                host: row.get(18)?,
                uploaded_by: row.get(19)?,
                ci_provider: row.get(20)?,
                ci_run_url: row.get(21)?,
                description: row.get(22)?,
                domarinn_version: row.get(23)?,
                empty_count: row.get(24)?,
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
            cached_hidden,
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
    cache_hits: Option<i64>,
    cache_misses: Option<i64>,
    actor: Option<String>,
    host: Option<String>,
    uploaded_by: Option<String>,
    ci_provider: Option<String>,
    ci_run_url: Option<String>,
    description: Option<String>,
    domarinn_version: Option<String>,
    empty_count: Option<i64>,
}

/// Map a stored cache counter to its wire value: NULL (legacy, pre-backfill)
/// and the -1 undecodable-blob sentinel both surface as `None`.
fn clean_cache_count(v: Option<i64>) -> Option<i64> {
    v.filter(|n| *n >= 0)
}

/// Map a stored `runs.empty_count` to its wire value: the key is present only
/// when there is something to report. NULL (legacy, pre-backfill) and the `-1`
/// undecodable-blob sentinel are *unknown*, and a plain `0` is "known: none" —
/// all three surface as `None` so the wire has one meaning for absent. See
/// [`RunListItem::empty_count`] for why a reader must not render that as `0`.
fn reportable_empty_count(v: Option<i64>) -> Option<u64> {
    v.filter(|n| *n > 0).map(|n| n as u64)
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
            cache_hits: clean_cache_count(self.cache_hits),
            cache_misses: clean_cache_count(self.cache_misses),
            empty_count: reportable_empty_count(self.empty_count),
            actor: self.actor.clone(),
            host: self.host.clone(),
            uploaded_by: self.uploaded_by.clone(),
            ci_provider: self.ci_provider.clone(),
            ci_run_url: self.ci_run_url.clone(),
            note: self.description.clone(),
            domarinn_version: self.domarinn_version.clone(),
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

fn get_run_detail(
    conn: &Connection,
    id: &RunId,
    vis: &RunVisibility,
) -> anyhow::Result<Option<RunDetailResponse>> {
    let mut args: Vec<rusqlite::types::Value> = vec![id.as_str().to_string().into()];
    let clause = visibility_predicate("runs", vis, &mut args);
    let sql = format!(
        "SELECT id, project, suite, created_at, uploaded_at, schema_version,
                    git_branch, git_commit, git_dirty, ci_provider, ci_run_url,
                    case_count, pass_count, fail_count, error_count,
                    prompt_tokens, completion_tokens, cost_microusd, duration_ms,
                    content_hash, uploaded_by, config_digest, cache_hits, cache_misses,
                    actor, host, description, domarinn_version,
                    cache_read_tokens, cache_write_tokens,
                    cache_savings_microusd, grader_cost_microusd
             FROM runs WHERE id = ?1 AND {clause}"
    );
    let row = conn
        .query_row(&sql, rusqlite::params_from_iter(args.iter()), |row| {
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
                // Map the empty-string backfill sentinel to `None`.
                config_digest: empty_to_none(row.get::<_, Option<String>>(21)?),
                cache_hits: clean_cache_count(row.get::<_, Option<i64>>(22)?),
                cache_misses: clean_cache_count(row.get::<_, Option<i64>>(23)?),
                actor: row.get::<_, Option<String>>(24)?,
                host: row.get::<_, Option<String>>(25)?,
                note: row.get::<_, Option<String>>(26)?,
                domarinn_version: row.get::<_, Option<String>>(27)?,
                // Migration-12 columns. NULL for anything stored before
                // them, and rendered as absent rather than zero — the run
                // may well have had cache activity we simply did not
                // record.
                cache_read_tokens: row.get::<_, Option<i64>>(28)?,
                cache_write_tokens: row.get::<_, Option<i64>>(29)?,
                cache_savings_usd: from_microusd(row.get::<_, Option<i64>>(30)?),
                grader_cost_usd: from_microusd(row.get::<_, Option<i64>>(31)?),
                // Filled in below, after the row is loaded.
                tags: Vec::new(),
                assert_labels: Vec::new(),
                empty_counts: None,
            })
        })
        // Never `.ok()`: a fault must not read as a missing run (see `sets`).
        .optional()?;

    let Some(mut detail) = row else {
        return Ok(None);
    };

    detail.tags = load_run_tags(conn, id.as_str())?;
    detail.assert_labels = distinct_assert_labels(conn, id.as_str())?;
    detail.empty_counts = empty_counts_by_reason(conn, id.as_str())?;
    Ok(Some(detail))
}

/// This run's empty-output cases tallied by reason, or `None` when it has none.
///
/// Derived from the `cases` rows, exactly like the `runs.empty_count` column
/// the list row reads — the same source, grouped instead of counted. That is
/// what makes the list count, this map, and the case grid agree by
/// construction — the backfill's `-1` corrupt-blob sentinel aside — even for an
/// externally authored document whose own `summary.empty_counts` contradicts
/// its cases. (The document keeps its summary map untouched for export
/// consumers; nothing here rewrites it.)
///
/// `empty_reason <> ''` excludes the "known: not empty" sentinel, the same
/// guard [`super::cases::CaseListFilter`] applies to the wire filter, and NULL
/// excludes legacy pre-backfill rows. Index-covered by migration 15's
/// `idx_cases_run_empty_reason(run_id, empty_reason)`.
fn empty_counts_by_reason(
    conn: &Connection,
    run_id: &str,
) -> anyhow::Result<Option<BTreeMap<String, u64>>> {
    let mut stmt = conn.prepare(
        "SELECT empty_reason, COUNT(*) FROM cases
          WHERE run_id = ?1 AND empty_reason IS NOT NULL AND empty_reason <> ''
          GROUP BY empty_reason",
    )?;
    let rows = stmt.query_map(params![run_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut counts = BTreeMap::new();
    for row in rows {
        let (reason, n) = row?;
        counts.insert(reason, n.max(0) as u64);
    }
    // Omitted, never `{}` — see `RunDetailResponse::empty_counts`.
    Ok((!counts.is_empty()).then_some(counts))
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

fn export_run_blob(
    conn: &Connection,
    id: &RunId,
    vis: &RunVisibility,
) -> anyhow::Result<Option<serde_json::Value>> {
    let Some(blob) = load_run_blob(conn, id, vis)? else {
        return Ok(None);
    };
    let bytes = decompress(&blob)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(Some(value))
}

/// The stored run document for `id`, or `None` when there is none *or* the run
/// is invisible. `run_blobs` carries no `project`/`suite` of its own, so the
/// access check rides on an `EXISTS` over `runs`.
fn load_run_blob(
    conn: &Connection,
    id: &RunId,
    vis: &RunVisibility,
) -> anyhow::Result<Option<Vec<u8>>> {
    let mut args: Vec<rusqlite::types::Value> = vec![id.as_str().to_string().into()];
    let clause = visible_run_predicate(1, vis, &mut args);
    let sql = format!("SELECT body FROM run_blobs WHERE run_id = ?1 AND {clause}");
    Ok(conn
        .query_row(&sql, rusqlite::params_from_iter(args.iter()), |row| {
            row.get(0)
        })
        .ok())
}

/// Like [`export_run_blob`], but extracts just the `config_digest` and
/// `config_snapshot` from the stored run document — the cheap config fetch
/// behind `GET /runs/{id}/config`. Returns `None` when the run has no stored
/// blob (unknown run). The digest is `None` when absent or the empty-string
/// sentinel; the snapshot is `null` when the document has none.
fn run_config_blob(
    conn: &Connection,
    id: &RunId,
    vis: &RunVisibility,
) -> anyhow::Result<Option<RunConfigResponse>> {
    let Some(blob) = load_run_blob(conn, id, vis)? else {
        return Ok(None);
    };
    let bytes = decompress(&blob)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let config_digest = empty_to_none(
        value
            .get("config_digest")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    );
    let config = value
        .get("config_snapshot")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    Ok(Some(RunConfigResponse {
        run_id: id.clone(),
        config_digest,
        config,
    }))
}
