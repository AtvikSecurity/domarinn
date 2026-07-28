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
use crate::asserts::MetricCtx;
use crate::cache::{CacheBackend, CacheEntry, CacheMode};
use crate::cache_key::provider_cache_key;
use crate::config::{Assert, Suite, TestCase};
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
    CaseResult, CaseStatus, CellKey, FilterSpec, RunResult, RunSummary, RESULT_SCHEMA_VERSION,
};
use crate::retry::{with_retry, RetryPolicy, RetryStats};
use crate::scoring::case_verdict;
use crate::template::TemplateEngine;
use crate::types::{Output, RenderedPrompt};

#[path = "runner_asserts.rs"]
mod runner_asserts;
use runner_asserts::{assert_error_message, evaluate_asserts, has_latency_assert};

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
    /// Retry budget override. `None` uses the suite's `runner.retries.max`.
    ///
    /// A run option rather than a suite mutation on purpose: `config_digest` is
    /// derived from the serialized suite, so editing the suite here would show a
    /// spurious config drift in every `--against` comparison.
    pub retries: Option<u32>,
    /// Persist the provider's raw response metadata *and* the captured provider
    /// request in each `CaseResult`. Default `true`; disabled by `--no-raw` to
    /// keep result documents small. Both are bulky provenance rather than
    /// results, and both are dropped whole above [`RAW_MAX_BYTES`].
    pub include_raw: bool,
    /// What to record about who and where this run came from. A run option
    /// rather than a suite field for the same reason as `retries` above.
    pub provenance: crate::provenance::ProvenanceOptions,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            filter: FilterOpts::default(),
            repeat: 1,
            cache_mode: CacheMode::ReadWrite,
            concurrency: None,
            retries: None,
            include_raw: true,
            provenance: crate::provenance::ProvenanceOptions::default(),
        }
    }
}

/// A grader for the non-local assert kinds (`exec`, `llm-rubric`, `similar`).
///
/// `Ok(outcome)` is a real verdict (pass or fail). `Err(reason)` is a grader
/// problem — a missing/unconfigured grader, a transport error, or a truncated
/// verdict — which the runner records as an `Error` (fail closed), distinct from
/// a graded-and-failed assertion. When no grader is provided at all, deferred
/// asserts likewise fail closed as errors.
///
/// **Verdicts are not cached.** Note the absence of a [`CacheBackend`] here:
/// grading runs live on every call, so an LLM-graded suite re-pays its grader on
/// every run even when every provider response was a cache hit. Only provider
/// responses are cached. Do not assume otherwise — the caching happens in
/// `call_with_cache`, which this trait is never routed through, and
/// `DefaultGrader` talks to its endpoint over its own client. Adding verdict
/// caching means threading a backend into `grade` and deriving a key from the
/// grader fingerprint, the rendered rubric, and the output; pinned by
/// `grader_verdicts_are_not_cached_today` in `tests/cache_integration.rs`.
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
    let mut generated = resolve_generators(&expanded.deferred_generators, base_dir).await?;
    // Generators resolve after `expand_tests`, so their cases miss the defaults
    // merge it performs. Apply it here or `defaults:` silently skips them.
    if let Some(defaults) = &suite.defaults {
        crate::resolve::apply_defaults(&mut generated, defaults);
    }
    tests.extend(generated);

    // A per-case `cache_salt` only chooses the cache key; it never makes a
    // provider cacheable. Warn when a suite sets one against a provider that
    // does not cache at all, where the salt silently does nothing.
    if tests.iter().any(|t| t.cache_salt.is_some()) {
        let uncacheable: Vec<&str> = providers
            .iter()
            .filter(|p| !p.cacheable())
            .map(|p| p.id())
            .collect();
        if !uncacheable.is_empty() {
            tracing::warn!(
                providers = %uncacheable.join(", "),
                "tests set `cache_salt`, but these providers are not cacheable \
                 (an `exec` provider needs its own `cache_salt` to be cached at \
                 all) — the per-case salt has no effect for them"
            );
        }
    }

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
    let mut retry_cfg = RetryPolicy::from_suite(suite);
    if let Some(max) = opts.retries {
        retry_cfg.max = max;
    }
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

    // A run with no explicit `--note` inherits the suite's `description`, which
    // is otherwise parsed and read by nothing. Collected after the work so a
    // long run records the dirty state it actually finished with.
    let mut provenance_opts = opts.provenance.clone();
    if provenance_opts.note.is_none() {
        provenance_opts.note = suite.description.clone();
    }
    let provenance = crate::provenance::collect(&provenance_opts, base_dir);

    // Over the WHOLE expanded test set, before the cell-loop filter: otherwise
    // `--tag smoke` and a full run of an identical suite disagree and every
    // filtered CI job reads as "the tests changed".
    let digests = crate::digests::config_digests(suite, &tests);

    Ok(RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: RunId::generate(),
        project: suite.project.clone(),
        suite: suite.suite.clone(),
        started_at,
        finished_at,
        config_digest,
        config_snapshot,
        git: provenance.git,
        ci: provenance.ci,
        origin: provenance.origin,
        digests: Some(digests),
        share_url: None,
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
    retry_cfg: &RetryPolicy,
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
                CallFailure::before_any_attempt(format!("rendering vars: {e}")),
                0,
                CaseInputs::default(),
            )
        }
    };
    // Persisted verbatim into `CaseResult.vars` (the UI's Input view). Cloned
    // because `rendered_vars` is moved into the provider request below.
    let case_vars = rendered_vars.clone();
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
                    CallFailure::before_any_attempt(format!("rendering prompt: {e}")),
                    0,
                    CaseInputs {
                        vars: case_vars,
                        ..Default::default()
                    },
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
        // Keys this case's cache entry only; never reaches the provider.
        case_salt: test.cache_salt.clone(),
    };

    // Computed here, not inside `call_with_cache`'s `use_cache` gate: the cache
    // key is skipped entirely under `--no-cache`, for a case with a `latency`
    // assert, and for an unsalted `exec` provider — which is exactly the set of
    // runs a CI comparison cares about. Identity must not depend on caching.
    let prompt_digest = Some(crate::digests::prompt_digest(&req));
    let provider_digest = Some(crate::digests::provider_digest(&provider.fingerprint()));

    // What this provider will actually send, built by the provider itself from
    // the same code path as the call. Captured *before* the call so a failed
    // case still carries it — which is where it earns its keep: an HTTP 404
    // explains itself the moment the model id in the request is visible.
    let request = json_to_persist(include_raw, provider.request_preview(&req), "request");

    // Latency assertions must not observe a cached (near-zero) latency.
    let bypass_cache = has_latency_assert(&test.assert);
    let effective_mode = if bypass_cache {
        CacheMode::Disabled
    } else {
        cache_mode
    };

    let start = Instant::now();
    let outcome = match call_with_cache(
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
        Ok(outcome) => outcome,
        Err(failure) => {
            return error_case(
                cell,
                case_key,
                name,
                test,
                failure,
                start.elapsed().as_millis() as u64,
                CaseInputs {
                    prompt: rendered_prompt,
                    vars: case_vars,
                    request,
                    prompt_digest,
                    provider_digest,
                },
            );
        }
    };
    let CallOutcome {
        response,
        cached,
        attempts,
        provider_latency_ms,
    } = outcome;
    let attempts = attempts.unwrap_or(0);
    let wall_ms = start.elapsed().as_millis() as u64;
    // Provider time, never wall time: `latency` assertions read this, and
    // charging retry backoff to it fails them on a model that answered fast.
    // Entries written before the field existed fall back to wall time, which is
    // what they always reported.
    let latency_ms = provider_latency_ms.unwrap_or(wall_ms);
    // After the cache read, so a replayed hit carries the same diagnosis a
    // fresh call would. Keyed on the reason being present, never on the output
    // looking empty.
    let empty_reason = provider.classify_empty(&response);
    let reasoning = response.reasoning.clone();

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

    // `Some` exactly when at least one assert errored, so it carries both the
    // diagnosis and the "did anything error" verdict input.
    let assert_error = assert_error_message(&assert_results);
    // Computed before the results are moved into the case below.
    let assert_digest = crate::digests::assert_digest(&assert_results);
    let verdict = case_verdict(&scored, test.threshold);
    let status = if assert_error.is_some() {
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
        vars: case_vars,
        status,
        score: verdict.score,
        output: Some(response.output),
        prompt: rendered_prompt,
        request,
        stop_reason: response.stop_reason,
        raw: json_to_persist(include_raw, response.raw, "raw"),
        asserts: assert_results,
        usage: response.usage,
        cost_usd: response.cost_usd,
        latency_ms,
        wall_ms: Some(wall_ms),
        cached,
        attempts,
        prompt_digest,
        provider_digest,
        assert_digest,
        error: assert_error,
        reasoning,
        empty_reason,
    }
}

/// One provider call's result, plus how it was obtained.
struct CallOutcome {
    response: ProviderResponse,
    cached: bool,
    /// `None` only for a cache hit on an entry written before entries recorded
    /// attempts — honest about not knowing, where the old `0` sentinel was not.
    attempts: Option<u32>,
    /// In-flight provider time, excluding retry backoff. On a cache hit this is
    /// the *original* call's latency replayed from the entry, not the cache-read
    /// time.
    provider_latency_ms: Option<u64>,
}

/// A failed provider call. Carries the attempt count so an errored case can
/// report what it actually spent instead of a hardcoded `1`.
#[derive(Debug)]
struct CallFailure {
    message: String,
    attempts: u32,
}

impl CallFailure {
    /// A failure that never reached the provider (cache read error, cache-only
    /// miss) — no attempt was made against the system under test.
    fn before_any_attempt(message: String) -> Self {
        CallFailure {
            message,
            attempts: 0,
        }
    }
}

/// Call a provider, consulting the cache per `mode` and retrying retriable
/// errors with backoff.
#[tracing::instrument(name = "provider_call", skip_all, fields(provider = %provider.id()))]
async fn call_with_cache(
    provider: &dyn Provider,
    req: &ProviderRequest,
    ctx: &CallCtx,
    cache: &dyn CacheBackend,
    mode: CacheMode,
    repeat: u32,
    retry_cfg: &RetryPolicy,
) -> Result<CallOutcome, CallFailure> {
    let use_cache = mode != CacheMode::Disabled && provider.cacheable();
    let key = use_cache.then(|| provider_cache_key(&provider.fingerprint(), req, repeat));

    if let Some(key) = &key {
        match cache.get(key).await {
            Ok(Some(entry)) => {
                tracing::debug!(%key, "cache hit");
                let attempts = entry.attempts;
                let provider_latency_ms = entry.provider_latency_ms;
                return Ok(CallOutcome {
                    response: entry_to_response(entry),
                    cached: true,
                    attempts,
                    provider_latency_ms,
                });
            }
            Ok(None) => {
                tracing::debug!(%key, "cache miss");
                if mode == CacheMode::ReadOnlyStrict {
                    return Err(CallFailure::before_any_attempt(format!(
                        "cache-only: miss for key {key}"
                    )));
                }
            }
            Err(e) => {
                return Err(CallFailure::before_any_attempt(format!(
                    "cache read error: {e}"
                )))
            }
        }
    }

    let (result, stats) = with_retry(retry_cfg, |_attempt| provider.call(req, ctx)).await;

    match result {
        Ok(response) => {
            if let Some(key) = &key {
                if mode == CacheMode::ReadWrite {
                    let entry = response_to_entry(provider, &response, stats);
                    // A cache write failure must not fail the run.
                    if let Err(e) = cache.put(key, &entry).await {
                        tracing::warn!(error = %e, "cache write failed");
                    }
                }
            }
            Ok(CallOutcome {
                response,
                cached: false,
                attempts: Some(stats.attempts),
                provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
            })
        }
        Err(ProviderError::Retriable { source, .. }) => Err(CallFailure {
            message: format!(
                "provider error after {} attempt(s): {source}",
                stats.attempts
            ),
            attempts: stats.attempts,
        }),
        Err(ProviderError::Fatal(e)) => Err(CallFailure {
            message: format!("provider error: {e}"),
            attempts: stats.attempts,
        }),
    }
}

/// Everything a case knows about its own inputs before a provider responds:
/// what was rendered, and what was going to be sent.
///
/// Grouped so `error_case` can hand all of it to a failed case — an errored case
/// that still shows its request is the difference between "HTTP 404" and "HTTP
/// 404, and here is the model id we asked for".
#[derive(Default)]
struct CaseInputs {
    prompt: Option<RenderedPrompt>,
    vars: serde_json::Map<String, serde_json::Value>,
    request: Option<Json>,
    /// Identity of what was going to be sent. Present for every failure after
    /// the request was built; `None` for the two earlier ones (rendering vars,
    /// rendering the prompt), which honestly have no input identity yet.
    prompt_digest: Option<String>,
    provider_digest: Option<String>,
}

fn error_case(
    cell: CellKey,
    case_key: CaseKey,
    name: Option<String>,
    test: &TestCase,
    failure: CallFailure,
    wall_ms: u64,
    inputs: CaseInputs,
) -> CaseResult {
    CaseResult {
        cell,
        case_key,
        name,
        tags: test.tags.clone(),
        vars: inputs.vars,
        status: CaseStatus::Error,
        score: 0.0,
        output: None,
        prompt: inputs.prompt,
        request: inputs.request,
        stop_reason: None,
        raw: None,
        asserts: Vec::new(),
        usage: None,
        cost_usd: None,
        // 0 when the call never reached the provider (a cache-only miss or a
        // cache read error): there is no provider latency to report.
        latency_ms: 0,
        wall_ms: Some(wall_ms),
        cached: false,
        attempts: failure.attempts,
        prompt_digest: inputs.prompt_digest,
        provider_digest: inputs.provider_digest,
        // An errored case never graded anything, so there is no assert
        // definition to identify.
        assert_digest: None,
        error: Some(failure.message),
        reasoning: None,
        empty_reason: None,
    }
}

/// Apply the retention policy for a bulky JSON provenance payload: keep it only
/// when raw persistence is enabled and it fits within [`RAW_MAX_BYTES`];
/// otherwise drop it to `None` (an oversized blob is dropped whole — truncated
/// JSON is useless). `what` names the payload in the drop log.
fn json_to_persist(include_raw: bool, raw: Option<Json>, what: &str) -> Option<Json> {
    if !include_raw {
        return None;
    }
    let raw = raw?;
    match serde_json::to_vec(&raw) {
        Ok(bytes) if bytes.len() > RAW_MAX_BYTES => {
            tracing::debug!(
                raw_bytes = bytes.len(),
                max_bytes = RAW_MAX_BYTES,
                payload = what,
                "dropping oversized provider payload"
            );
            None
        }
        Ok(_) => Some(raw),
        Err(e) => {
            tracing::debug!(error = %e, payload = what, "dropping unserializable provider payload");
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
        // A cached case replays the original attempt count, so counting it here
        // would re-report a retry that happened on some earlier run.
        if !c.cached && c.attempts > 1 {
            s.retried_cases += 1;
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
        raw: entry.raw,
        reasoning: entry.reasoning,
        empty_reason: entry.empty_reason,
    }
}

fn response_to_entry(
    provider: &dyn Provider,
    response: &ProviderResponse,
    stats: RetryStats,
) -> CacheEntry {
    CacheEntry {
        created_at: Utc::now(),
        provider_fingerprint: provider.fingerprint(),
        output: response.output.clone(),
        usage: response.usage.clone(),
        cost_usd: response.cost_usd,
        stop_reason: response.stop_reason.clone(),
        attempts: Some(stats.attempts),
        provider_latency_ms: Some(stats.in_flight.as_millis() as u64),
        reasoning: response.reasoning.clone(),
        empty_reason: response.empty_reason.clone(),
        // Same size cap as persistence, so a pathological payload can't bloat
        // the shared cache. `--no-raw` intentionally does NOT strip the cache
        // copy: a later run without the flag replaying this entry should still
        // get the metadata.
        raw: json_to_persist(true, response.raw.clone(), "raw"),
        domarinn_version: crate::VERSION.to_string(),
    }
}

#[cfg(test)]
#[path = "runner_tests.rs"]
mod tests;
