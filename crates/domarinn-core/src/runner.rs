//! The run orchestrator: expand the matrix, call providers (through the cache),
//! evaluate assertions, and assemble a [`RunResult`].
//!
//! Phase 2 runs sequentially; the cell loop is written so a later phase can
//! parallelize it while preserving deterministic output order (cells carry their
//! index).

use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value as Json;

use crate::assertion::AssertOutcome;
use crate::asserts::{evaluate_local, is_local, MetricCtx};
use crate::cache::{CacheBackend, CacheEntry, CacheMode};
use crate::cache_key::provider_cache_key;
use crate::config::{Assert, AssertKind, Suite, TestCase};
use crate::filter::{Filter, FilterOpts};
use crate::generate::resolve_generators;
use crate::ids::{CaseKey, RunId};
use crate::progress::{ProgressEvent, ProgressSink};
use crate::provider::{
    CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse, TestMeta,
};
use crate::provider_factory::build_provider;
use crate::render::{self, render_prompt};
use crate::resolve::expand_tests;
use crate::result::{
    AssertResult, AssertStatus, CaseResult, CaseStatus, CellKey, FilterSpec, RunResult, RunSummary,
    RESULT_SCHEMA_VERSION,
};
use crate::scoring::{case_verdict, remaining_can_change_outcome, Scored};
use crate::template::TemplateEngine;
use crate::types::{Output, RenderedPrompt};

/// Upper bound on the raw provider metadata persisted per case. A payload over
/// this size is dropped wholesale (truncated JSON is useless) rather than stored.
const RAW_MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("building provider: {0}")]
    Factory(#[from] crate::provider_factory::FactoryError),
    #[error("expanding tests: {0}")]
    Resolve(#[from] crate::resolve::ResolveError),
    #[error("running generator: {0}")]
    Generate(#[from] crate::generate::GenerateError),
}

/// Options controlling a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub filter: FilterOpts,
    /// Number of trials per cell (variance). Default 1.
    pub repeat: u32,
    pub cache_mode: CacheMode,
    /// Max concurrent provider calls. `None` uses the suite's `runner.concurrency`.
    pub concurrency: Option<usize>,
    /// Persist the provider's raw response metadata in each `CaseResult`. Default
    /// `true`; disabled by `--no-raw` to keep result documents small.
    pub include_raw: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            filter: FilterOpts::default(),
            repeat: 1,
            cache_mode: CacheMode::ReadWrite,
            concurrency: None,
            include_raw: true,
        }
    }
}

/// Retry/backoff settings derived from the suite `runner.retries`.
#[derive(Debug, Clone, Copy)]
struct RetryConfig {
    max: u32,
    initial_ms: u64,
    max_ms: u64,
}

impl RetryConfig {
    fn from_suite(suite: &Suite) -> Self {
        match suite.runner.as_ref().and_then(|r| r.retries.as_ref()) {
            Some(r) => RetryConfig {
                max: r.max,
                initial_ms: r.initial_ms.unwrap_or(500),
                max_ms: r.max_ms.unwrap_or(8_000),
            },
            None => RetryConfig {
                max: 0,
                initial_ms: 500,
                max_ms: 8_000,
            },
        }
    }

    /// Backoff before attempt `attempt` (1-based), honoring a server hint.
    fn backoff(
        &self,
        attempt: u32,
        retry_after: Option<std::time::Duration>,
    ) -> std::time::Duration {
        if let Some(hint) = retry_after {
            return hint;
        }
        let exp = self
            .initial_ms
            .saturating_mul(1u64 << attempt.min(16).saturating_sub(1));
        std::time::Duration::from_millis(exp.min(self.max_ms))
    }
}

/// A grader for the non-local assert kinds (`exec`, `llm-rubric`, `similar`).
///
/// `Ok(outcome)` is a real verdict (pass or fail). `Err(reason)` is a grader
/// problem — a missing/unconfigured grader, a transport error, or a truncated
/// verdict — which the runner records as an `Error` (fail closed), distinct from
/// a graded-and-failed assertion. When no grader is provided at all, deferred
/// asserts likewise fail closed as errors.
#[async_trait]
pub trait AssertGrader: Send + Sync {
    async fn grade(
        &self,
        assert: &Assert,
        output: &Output,
        vars: &Json,
        engine: &TemplateEngine,
        working_dir: Option<&Path>,
    ) -> Result<AssertOutcome, String>;
}

/// Run a suite and produce a [`RunResult`].
///
/// The stable entry point used by the server and embedders: a thin delegate to
/// [`run_with_progress`] with no progress sink. Its signature is intentionally
/// unchanged — front-ends that want live progress call `run_with_progress`.
pub async fn run(
    suite: &Suite,
    base_dir: &Path,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    opts: &RunOptions,
) -> Result<RunResult, RunError> {
    run_with_progress(suite, base_dir, cache, grader, opts, None).await
}

/// Run a suite and produce a [`RunResult`], emitting [`ProgressEvent`]s to an
/// optional [`ProgressSink`] as it goes.
///
/// The sink is a bare `Option<&dyn ProgressSink>` parameter rather than a
/// [`RunOptions`] field on purpose: a trait object is neither `Debug` nor
/// `Clone`, and `RunOptions` must keep both derives. See [`crate::progress`]
/// for the full rationale (sync trait, not a channel; core stays UI-agnostic).
#[tracing::instrument(name = "run", skip_all, fields(project = ?suite.project, suite = ?suite.suite))]
pub async fn run_with_progress(
    suite: &Suite,
    base_dir: &Path,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    opts: &RunOptions,
    progress: Option<&dyn ProgressSink>,
) -> Result<RunResult, RunError> {
    let started_at = Utc::now();
    let engine = TemplateEngine::new();
    let filter = Filter::build(&opts.filter).map_err(|e| {
        RunError::Resolve(crate::resolve::ResolveError::Parse {
            path: "<filter>".into(),
            message: e.to_string(),
        })
    })?;

    // Providers (embeddings providers are grader helpers, not systems under test).
    let providers: Vec<Box<dyn Provider>> = suite
        .providers
        .iter()
        .filter(|p| !matches!(p.kind, crate::config::ProviderKind::Embeddings { .. }))
        .filter(|p| opts.filter.providers.is_empty() || opts.filter.providers.contains(&p.id))
        .map(build_provider)
        .collect::<Result<_, _>>()?;

    // Tests (files + inline + generators).
    let expanded = expand_tests(suite, base_dir)?;
    let mut tests = expanded.tests;
    tests.extend(resolve_generators(&expanded.deferred_generators, base_dir).await?);

    // Prompt slots: each prompt, or a single None slot when there are no prompts.
    let prompt_slots: Vec<Option<&crate::config::Prompt>> = if suite.prompts.is_empty() {
        vec![None]
    } else {
        suite.prompts.iter().map(Some).collect()
    };

    let repeat = opts.repeat.max(1);
    let ctx = CallCtx {
        working_dir: Some(base_dir.to_path_buf()),
    };
    let retry_cfg = RetryConfig::from_suite(suite);
    let concurrency = opts
        .concurrency
        .or_else(|| suite.runner.as_ref().and_then(|r| r.concurrency))
        .unwrap_or(1)
        .max(1);

    // Expand the matrix into indexed cells so completion order does not affect
    // output order.
    struct Cell<'a> {
        provider: &'a dyn Provider,
        prompt: Option<&'a crate::config::Prompt>,
        test: &'a TestCase,
        repeat: u32,
    }
    let mut cells: Vec<Cell> = Vec::new();
    for provider in &providers {
        for prompt in &prompt_slots {
            if let Some(p) = prompt {
                if !filter.prompt_included(&p.id) {
                    continue;
                }
            }
            for test in &tests {
                if !filter.matches_test(test) || !filter.provider_included(provider.id(), test) {
                    continue;
                }
                for repeat_idx in 0..repeat {
                    cells.push(Cell {
                        provider: provider.as_ref(),
                        prompt: *prompt,
                        test,
                        repeat: repeat_idx,
                    });
                }
            }
        }
    }

    let total = cells.len();
    if let Some(sink) = progress {
        sink.event(&ProgressEvent::RunStarted { total });
    }
    let mut slots: Vec<Option<CaseResult>> = (0..total).map(|_| None).collect();
    let completed: Vec<(usize, CaseResult)> = futures::stream::iter(cells.into_iter().enumerate())
        .map(|(i, cell)| {
            let ctx = &ctx;
            let engine = &engine;
            async move {
                // First statement in the per-cell task, so `CaseStarted` reflects
                // true in-flight order under `buffer_unordered`, not output order.
                if let Some(sink) = progress {
                    sink.event(&ProgressEvent::CaseStarted {
                        index: i,
                        cell: CellKey {
                            provider_id: cell.provider.id().to_string(),
                            prompt_id: cell.prompt.map(|p| p.id.clone()),
                            test_id: cell.test.id.clone().unwrap_or_default(),
                            repeat: cell.repeat,
                        },
                        name: cell
                            .test
                            .description
                            .clone()
                            .or_else(|| cell.test.id.clone()),
                    });
                }
                let case = run_cell(
                    cell.provider,
                    cell.prompt,
                    cell.test,
                    cell.repeat,
                    engine,
                    cache,
                    grader,
                    ctx,
                    base_dir,
                    opts.cache_mode,
                    opts.include_raw,
                    &retry_cfg,
                )
                .await;
                if let Some(sink) = progress {
                    sink.event(&ProgressEvent::CaseFinished {
                        index: i,
                        cell: case.cell.clone(),
                        name: case.name.clone(),
                        status: case.status,
                        score: case.score,
                        latency_ms: case.latency_ms,
                        cached: case.cached,
                    });
                }
                (i, case)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    for (i, case) in completed {
        slots[i] = Some(case);
    }
    let cases: Vec<CaseResult> = slots
        .into_iter()
        .map(|c| c.expect("every cell filled"))
        .collect();

    let finished_at = Utc::now();
    let summary = summarize(&cases);
    if let Some(sink) = progress {
        sink.event(&ProgressEvent::RunFinished {
            summary: summary.clone(),
        });
    }
    let config_snapshot = serde_json::to_value(suite).unwrap_or(Json::Null);
    let config_digest = format!(
        "blake3:{}",
        blake3::hash(crate::cache::canonical_json(&config_snapshot).as_bytes()).to_hex()
    );

    Ok(RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: RunId::generate(),
        project: suite.project.clone(),
        suite: suite.suite.clone(),
        started_at,
        finished_at,
        config_digest,
        config_snapshot,
        git: None,
        ci: None,
        filters: FilterSpec {
            tags: opts.filter.tags.clone(),
            filters: opts.filter.filters.clone(),
            providers: opts.filter.providers.clone(),
            prompts: opts.filter.prompts.clone(),
        },
        cases,
        summary,
    })
}

/// Evaluate one matrix cell.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    name = "case",
    skip_all,
    fields(
        provider = %provider.id(),
        prompt = prompt.map(|p| p.id.as_str()).unwrap_or(""),
        test = test.id.as_deref().unwrap_or(""),
        repeat = repeat,
    )
)]
async fn run_cell(
    provider: &dyn Provider,
    prompt: Option<&crate::config::Prompt>,
    test: &TestCase,
    repeat: u32,
    engine: &TemplateEngine,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    ctx: &CallCtx,
    base_dir: &Path,
    cache_mode: CacheMode,
    include_raw: bool,
    retry_cfg: &RetryConfig,
) -> CaseResult {
    let test_id = test.id.clone().unwrap_or_default();
    let cell = CellKey {
        provider_id: provider.id().to_string(),
        prompt_id: prompt.map(|p| p.id.clone()),
        test_id: test_id.clone(),
        repeat,
    };
    let case_key = cell.case_key();
    let name = test.description.clone().or_else(|| test.id.clone());

    // Render the test's vars once. `rendered_vars` excludes the environment, so
    // it is a stable request identity (cache key) and does not leak the whole
    // environment to exec providers. `render_ctx` adds `env` for rendering
    // prompts and `jinja` assertions (`{{ env.X }}`), and never enters the key.
    let rendered_vars = match render::render_vars(&test.vars, engine) {
        Ok(v) => v,
        Err(e) => {
            return error_case(
                cell,
                case_key,
                name,
                test,
                format!("rendering vars: {e}"),
                None,
            )
        }
    };
    let var_ctx = render::context_with_env(&rendered_vars);
    let rendered_prompt = match prompt {
        Some(p) => match render_prompt(p, &var_ctx, engine, base_dir) {
            Ok(rp) => Some(rp),
            Err(e) => {
                return error_case(
                    cell,
                    case_key,
                    name,
                    test,
                    format!("rendering prompt: {e}"),
                    None,
                )
            }
        },
        None => None,
    };

    let req = ProviderRequest {
        prompt: rendered_prompt.clone(),
        vars: rendered_vars.into_iter().collect(),
        params: serde_json::Map::new(),
        test: TestMeta {
            id: test_id.clone(),
            tags: test.tags.clone(),
        },
    };

    // Latency assertions must not observe a cached (near-zero) latency.
    let bypass_cache = has_latency_assert(&test.assert);
    let effective_mode = if bypass_cache {
        CacheMode::Disabled
    } else {
        cache_mode
    };

    let start = Instant::now();
    let (response, cached, attempts) = match call_with_cache(
        provider,
        &req,
        ctx,
        cache,
        effective_mode,
        repeat,
        retry_cfg,
    )
    .await
    {
        Ok(triple) => triple,
        Err(e) => {
            return error_case(cell, case_key, name, test, e, rendered_prompt);
        }
    };
    let latency_ms = start.elapsed().as_millis() as u64;

    let metrics = MetricCtx {
        latency_ms,
        cost_usd: response.cost_usd,
        total_tokens: response.usage.as_ref().map(|u| u.total()),
    };

    let (assert_results, scored) = evaluate_asserts(
        &test.assert,
        &response.output,
        &var_ctx,
        engine,
        grader,
        base_dir,
        &metrics,
        test.threshold,
    )
    .await;

    let any_error = assert_results
        .iter()
        .any(|a| a.status == AssertStatus::Error);
    let verdict = case_verdict(&scored, test.threshold);
    let status = if any_error {
        CaseStatus::Error
    } else if verdict.passed {
        CaseStatus::Pass
    } else {
        CaseStatus::Fail
    };

    CaseResult {
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        status,
        score: verdict.score,
        output: Some(response.output),
        prompt: rendered_prompt,
        stop_reason: response.stop_reason,
        raw: raw_to_persist(include_raw, response.raw),
        asserts: assert_results,
        usage: response.usage,
        cost_usd: response.cost_usd,
        latency_ms,
        cached,
        attempts,
        error: any_error.then(|| "one or more assertions errored".to_string()),
    }
}

/// Call a provider, consulting the cache per `mode` and retrying retriable
/// errors with backoff. Returns `(response, cached, attempts)`.
#[tracing::instrument(name = "provider_call", skip_all, fields(provider = %provider.id()))]
async fn call_with_cache(
    provider: &dyn Provider,
    req: &ProviderRequest,
    ctx: &CallCtx,
    cache: &dyn CacheBackend,
    mode: CacheMode,
    repeat: u32,
    retry_cfg: &RetryConfig,
) -> Result<(ProviderResponse, bool, u32), String> {
    let use_cache = mode != CacheMode::Disabled && provider.cacheable();
    let key = use_cache.then(|| provider_cache_key(&provider.fingerprint(), req, repeat));

    if let Some(key) = &key {
        match cache.get(key).await {
            Ok(Some(entry)) => {
                tracing::debug!(%key, "cache hit");
                return Ok((entry_to_response(entry), true, 0));
            }
            Ok(None) => {
                tracing::debug!(%key, "cache miss");
                if mode == CacheMode::ReadOnlyStrict {
                    return Err(format!("cache-only: miss for key {key}"));
                }
            }
            Err(e) => return Err(format!("cache read error: {e}")),
        }
    }

    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match provider.call(req, ctx).await {
            Ok(response) => {
                if let Some(key) = &key {
                    if mode == CacheMode::ReadWrite {
                        let entry = response_to_entry(provider, &response);
                        // A cache write failure must not fail the run.
                        if let Err(e) = cache.put(key, &entry).await {
                            tracing::warn!(error = %e, "cache write failed");
                        }
                    }
                }
                return Ok((response, false, attempt));
            }
            Err(ProviderError::Retriable {
                source,
                retry_after,
            }) => {
                if attempt > retry_cfg.max {
                    return Err(format!(
                        "provider error after {attempt} attempt(s): {source}"
                    ));
                }
                let delay = retry_cfg.backoff(attempt, retry_after);
                tracing::warn!(
                    attempt,
                    delay_ms = delay.as_millis() as u64,
                    error = %source,
                    "retriable provider error; retrying"
                );
                tokio::time::sleep(delay).await;
            }
            Err(ProviderError::Fatal(e)) => return Err(format!("provider error: {e}")),
        }
    }
}

/// Evaluate all asserts: local first, then (if they can still change the
/// outcome) the graded ones; otherwise mark them skipped.
#[allow(clippy::too_many_arguments)]
async fn evaluate_asserts(
    asserts: &[Assert],
    output: &Output,
    vars: &Json,
    engine: &TemplateEngine,
    grader: Option<&dyn AssertGrader>,
    base_dir: &Path,
    metrics: &MetricCtx,
    threshold: Option<f64>,
) -> (Vec<AssertResult>, Vec<Scored>) {
    // Slot results by original index so output order matches config order.
    let mut results: Vec<Option<AssertResult>> = vec![None; asserts.len()];
    let mut scored: Vec<Scored> = Vec::new();

    // Local (deterministic) asserts first.
    let mut deferred_indices: Vec<usize> = Vec::new();
    for (i, assert) in asserts.iter().enumerate() {
        if is_local(&assert.kind) {
            let outcome = evaluate_local(assert, output, engine, vars, metrics)
                .expect("local assert yields an outcome");
            scored.push(scored_of(assert, &outcome));
            results[i] = Some(assert_result(
                assert,
                &outcome,
                AssertStatus::from_pass(outcome.passed),
            ));
        } else {
            deferred_indices.push(i);
        }
    }

    // Decide whether the deferred asserts still matter.
    let remaining_weight: f64 = deferred_indices.iter().map(|i| asserts[*i].weight).sum();
    let matters = remaining_can_change_outcome(&scored, remaining_weight, threshold);

    for i in deferred_indices {
        let assert = &asserts[i];
        if !matters {
            results[i] = Some(skipped_result(assert));
            continue;
        }
        match grader {
            Some(g) => match g.grade(assert, output, vars, engine, Some(base_dir)).await {
                Ok(outcome) => {
                    scored.push(scored_of(assert, &outcome));
                    results[i] = Some(assert_result(
                        assert,
                        &outcome,
                        AssertStatus::from_pass(outcome.passed),
                    ));
                }
                // Fail closed: a grader problem is an error, not a plain fail.
                Err(reason) => {
                    results[i] = Some(error_assert(assert, reason));
                }
            },
            None => {
                // Fail closed: a deferred assert with no grader is an error.
                results[i] = Some(error_assert(
                    assert,
                    format!(
                        "no grader available for '{}' assertions in this run",
                        assert.kind.name().as_str()
                    ),
                ));
            }
        }
    }

    (results.into_iter().map(|r| r.unwrap()).collect(), scored)
}

fn scored_of(assert: &Assert, outcome: &AssertOutcome) -> Scored {
    Scored {
        weight: assert.weight,
        score: outcome.score,
        passed: outcome.passed,
    }
}

fn assert_result(assert: &Assert, outcome: &AssertOutcome, status: AssertStatus) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status,
        score: outcome.score,
        weight: assert.weight,
        reason: outcome.reason.clone(),
        details: outcome.details.clone(),
        cached: false,
    }
}

fn error_assert(assert: &Assert, reason: String) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status: AssertStatus::Error,
        score: 0.0,
        weight: assert.weight,
        reason,
        details: None,
        cached: false,
    }
}

fn skipped_result(assert: &Assert) -> AssertResult {
    AssertResult {
        kind: assert.kind.name(),
        status: AssertStatus::Skipped,
        score: 0.0,
        weight: assert.weight,
        reason: "skipped: outcome already decided".into(),
        details: None,
        cached: false,
    }
}

impl AssertStatus {
    fn from_pass(pass: bool) -> AssertStatus {
        if pass {
            AssertStatus::Pass
        } else {
            AssertStatus::Fail
        }
    }
}

fn has_latency_assert(asserts: &[Assert]) -> bool {
    asserts
        .iter()
        .any(|a| matches!(a.kind, AssertKind::Latency { .. }))
}

fn error_case(
    cell: CellKey,
    case_key: CaseKey,
    name: Option<String>,
    test: &TestCase,
    error: String,
    prompt: Option<RenderedPrompt>,
) -> CaseResult {
    CaseResult {
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        status: CaseStatus::Error,
        score: 0.0,
        output: None,
        prompt,
        stop_reason: None,
        raw: None,
        asserts: Vec::new(),
        usage: None,
        cost_usd: None,
        latency_ms: 0,
        cached: false,
        attempts: 1,
        error: Some(error),
    }
}

/// Apply the raw-metadata retention policy: keep the payload only when raw
/// persistence is enabled and it fits within [`RAW_MAX_BYTES`]; otherwise drop
/// it to `None` (an oversized blob is dropped whole — truncated JSON is useless).
fn raw_to_persist(include_raw: bool, raw: Option<Json>) -> Option<Json> {
    if !include_raw {
        return None;
    }
    let raw = raw?;
    match serde_json::to_vec(&raw) {
        Ok(bytes) if bytes.len() > RAW_MAX_BYTES => {
            tracing::debug!(
                raw_bytes = bytes.len(),
                max_bytes = RAW_MAX_BYTES,
                "dropping oversized raw provider metadata"
            );
            None
        }
        Ok(_) => Some(raw),
        Err(e) => {
            tracing::debug!(error = %e, "dropping unserializable raw provider metadata");
            None
        }
    }
}

fn summarize(cases: &[CaseResult]) -> RunSummary {
    let mut s = RunSummary {
        total: cases.len() as u64,
        ..Default::default()
    };
    let mut cost = 0.0;
    let mut any_cost = false;
    for c in cases {
        match c.status {
            CaseStatus::Pass => s.passed += 1,
            CaseStatus::Fail => s.failed += 1,
            CaseStatus::Error => s.errored += 1,
            CaseStatus::Skip => s.skipped += 1,
        }
        if let Some(u) = &c.usage {
            s.prompt_tokens += u.input_tokens;
            s.completion_tokens += u.output_tokens;
        }
        if let Some(cst) = c.cost_usd {
            cost += cst;
            any_cost = true;
        }
        if c.cached {
            s.cache_hits += 1;
        } else {
            s.cache_misses += 1;
        }
    }
    s.cost_usd = any_cost.then_some(cost);
    s
}

fn entry_to_response(entry: CacheEntry) -> ProviderResponse {
    ProviderResponse {
        output: entry.output,
        usage: entry.usage,
        cost_usd: entry.cost_usd,
        stop_reason: entry.stop_reason,
        raw: None,
    }
}

fn response_to_entry(provider: &dyn Provider, response: &ProviderResponse) -> CacheEntry {
    CacheEntry {
        created_at: Utc::now(),
        provider_fingerprint: provider.fingerprint(),
        output: response.output.clone(),
        usage: response.usage.clone(),
        cost_usd: response.cost_usd,
        stop_reason: response.stop_reason.clone(),
        domarinn_version: crate::VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{CacheError, CacheStats, PurgeFilter};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A cache that never hits and never fails — the retry path under test uses
    /// `CacheMode::Disabled`, so these are inert, but the signature requires one.
    struct NoopCache;

    #[async_trait]
    impl CacheBackend for NoopCache {
        async fn get(
            &self,
            _key: &crate::cache::CacheKey,
        ) -> Result<Option<CacheEntry>, CacheError> {
            Ok(None)
        }
        async fn put(
            &self,
            _key: &crate::cache::CacheKey,
            _entry: &CacheEntry,
        ) -> Result<(), CacheError> {
            Ok(())
        }
        async fn stats(&self) -> Result<CacheStats, CacheError> {
            Ok(CacheStats::default())
        }
        async fn purge(&self, _filter: &PurgeFilter) -> Result<u64, CacheError> {
            Ok(0)
        }
    }

    /// A provider that fails retriably on its first call, then succeeds — enough
    /// to fire exactly one retry warning.
    struct FlakyProvider {
        calls: AtomicU32,
    }

    #[async_trait]
    impl Provider for FlakyProvider {
        fn id(&self) -> &str {
            "flaky"
        }
        fn fingerprint(&self) -> Json {
            serde_json::json!({ "type": "flaky" })
        }
        async fn call(
            &self,
            _req: &ProviderRequest,
            _ctx: &CallCtx,
        ) -> Result<ProviderResponse, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err(ProviderError::Retriable {
                    source: anyhow::anyhow!("boom"),
                    retry_after: None,
                })
            } else {
                Ok(ProviderResponse::text("ok"))
            }
        }
    }

    /// A `MakeWriter` that appends every line into a shared buffer.
    #[derive(Clone)]
    struct BufWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for BufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufWriter {
        type Writer = BufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The retry warning must carry `attempt` and `delay_ms` as structured
    /// fields (not just interpolated into the message), so a `-vv` / JSON log can
    /// filter on them. Scoped capture subscriber; inert for every other test.
    #[test]
    fn retry_warn_carries_structured_attempt_and_delay_fields() {
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(BufWriter(buf.clone()))
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap();
            rt.block_on(async {
                let provider = FlakyProvider {
                    calls: AtomicU32::new(0),
                };
                let req = ProviderRequest {
                    prompt: None,
                    vars: std::collections::BTreeMap::new(),
                    params: serde_json::Map::new(),
                    test: TestMeta::default(),
                };
                let ctx = CallCtx::default();
                let cache = NoopCache;
                // initial_ms = 1 keeps the single backoff sleep sub-millisecond.
                let retry_cfg = RetryConfig {
                    max: 1,
                    initial_ms: 1,
                    max_ms: 1,
                };
                let (_resp, cached, attempts) = call_with_cache(
                    &provider,
                    &req,
                    &ctx,
                    &cache,
                    CacheMode::Disabled,
                    0,
                    &retry_cfg,
                )
                .await
                .expect("second attempt succeeds");
                assert!(!cached);
                assert_eq!(attempts, 2);
            });
        });

        let logged = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            logged.contains("\"attempt\""),
            "retry warn must record an `attempt` field; got: {logged}"
        );
        assert!(
            logged.contains("\"delay_ms\""),
            "retry warn must record a `delay_ms` field; got: {logged}"
        );
    }

    #[test]
    fn backoff_grows_and_caps() {
        let cfg = RetryConfig {
            max: 5,
            initial_ms: 100,
            max_ms: 1000,
        };
        assert_eq!(cfg.backoff(1, None), Duration::from_millis(100));
        assert_eq!(cfg.backoff(2, None), Duration::from_millis(200));
        assert_eq!(cfg.backoff(3, None), Duration::from_millis(400));
        // Caps at max_ms.
        assert_eq!(cfg.backoff(10, None), Duration::from_millis(1000));
    }

    #[test]
    fn backoff_honors_retry_after_hint() {
        let cfg = RetryConfig {
            max: 5,
            initial_ms: 100,
            max_ms: 1000,
        };
        assert_eq!(
            cfg.backoff(1, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
    }
}
