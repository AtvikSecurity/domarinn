//! The run orchestrator: expand the matrix, call providers (through the cache),
//! evaluate assertions, and assemble a [`RunResult`].
//!
//! Phase 2 runs sequentially; the cell loop is written so a later phase can
//! parallelize it while preserving deterministic output order (cells carry their
//! index).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value as Json;

use crate::assertion::AssertOutcome;
use crate::asserts::{evaluate_local, is_local, kind_name, MetricCtx};
use crate::cache::{CacheBackend, CacheEntry, CacheMode};
use crate::cache_key::provider_cache_key;
use crate::config::{Assert, AssertKind, Suite, TestCase};
use crate::filter::{Filter, FilterOpts};
use crate::generate::resolve_generators;
use crate::provider::{
    CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse, TestMeta,
};
use crate::provider_factory::build_provider;
use crate::render::{build_context, render_prompt};
use crate::resolve::expand_tests;
use crate::result::{
    AssertResult, AssertStatus, CaseResult, CaseStatus, CellKey, FilterSpec, RunResult, RunSummary,
    RESULT_SCHEMA_VERSION,
};
use crate::scoring::{case_verdict, remaining_can_change_outcome, Scored};
use crate::template::TemplateEngine;
use crate::types::Output;

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
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            filter: FilterOpts::default(),
            repeat: 1,
            cache_mode: CacheMode::ReadWrite,
            concurrency: None,
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
pub async fn run(
    suite: &Suite,
    base_dir: &Path,
    cache: &dyn CacheBackend,
    grader: Option<&dyn AssertGrader>,
    opts: &RunOptions,
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
    let mut slots: Vec<Option<CaseResult>> = (0..total).map(|_| None).collect();
    let completed: Vec<(usize, CaseResult)> = futures::stream::iter(cells.into_iter().enumerate())
        .map(|(i, cell)| {
            let ctx = &ctx;
            let engine = &engine;
            async move {
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
                    &retry_cfg,
                )
                .await;
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
    let config_snapshot = serde_json::to_value(suite).unwrap_or(Json::Null);
    let config_digest = format!(
        "blake3:{}",
        blake3::hash(crate::cache::canonical_json(&config_snapshot).as_bytes()).to_hex()
    );

    Ok(RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: ulid::Ulid::new().to_string(),
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

    // Render context and prompt.
    let var_ctx = match build_context(&test.vars, engine) {
        Ok(c) => c,
        Err(e) => return error_case(cell, case_key, name, test, format!("rendering vars: {e}")),
    };
    let rendered_prompt = match prompt {
        Some(p) => match render_prompt(p, &var_ctx, engine, base_dir) {
            Ok(rp) => Some(rp),
            Err(e) => {
                return error_case(cell, case_key, name, test, format!("rendering prompt: {e}"))
            }
        },
        None => None,
    };

    let req = ProviderRequest {
        prompt: rendered_prompt.clone(),
        vars: json_object(&var_ctx),
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
            return error_case(cell, case_key, name, test, e);
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
            Ok(Some(entry)) => return Ok((entry_to_response(entry), true, 0)),
            Ok(None) => {
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
                            tracing::warn!("cache write failed: {e}");
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
                    "retriable provider error (attempt {attempt}): {source}; retrying in {delay:?}"
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
                        kind_name(&assert.kind)
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
        kind: kind_name(&assert.kind).to_string(),
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
        kind: kind_name(&assert.kind).to_string(),
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
        kind: kind_name(&assert.kind).to_string(),
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
    case_key: String,
    name: Option<String>,
    test: &TestCase,
    error: String,
) -> CaseResult {
    CaseResult {
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        status: CaseStatus::Error,
        score: 0.0,
        output: None,
        asserts: Vec::new(),
        usage: None,
        cost_usd: None,
        latency_ms: 0,
        cached: false,
        attempts: 1,
        error: Some(error),
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

fn json_object(value: &Json) -> BTreeMap<String, Json> {
    value
        .as_object()
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
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
        measurellm_version: crate::VERSION.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

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
