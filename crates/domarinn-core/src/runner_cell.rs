//! One matrix cell, from rendered prompt to [`CaseResult`].
//!
//! Split out of `runner` because that file sits at the 1000-line ratchet
//! (`crates/domarinn-core/tests/file_length.rs`) and this is the half that
//! keeps growing: every new thing a case can do — a fallback chain, a new
//! classification — lands here, while the orchestration around it does not
//! change. Sibling module rather than a submodule directory, matching
//! `runner_cache` / `runner_asserts` / `runner_result`.
//!
//! `use super::*` on purpose: this is `runner`'s own body, moved, and it reads
//! against the same imports it was written against.
use super::*;

/// Whether this cell's empty reason is one the suite asked to skip.
///
/// The distinction `skip` exists to draw: a blank output is a *successful*
/// call, so it gets graded and scores zero against every assertion — which
/// reads as a prompt failure whether or not it was one. `skip` says "not
/// gradeable, and that is not a verdict", and is counted separately rather
/// than dragging the pass rate down.
///
/// Compared as a plain string against the configured list, so a reason this
/// build has never heard of still works — `EmptyReason` is open by design and
/// this must not become the one place that closes it.
pub(super) fn reasoning_is_skippable(
    reason: Option<&crate::empty::EmptyReason>,
    skip: &[String],
) -> bool {
    reason.is_some_and(|r| skip.iter().any(|s| s == r.as_str()))
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
pub(super) async fn run_cell<'a>(
    provider: &'a dyn Provider,
    fallbacks: &[&'a dyn Provider],
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
    schemas: &crate::jsonschema_cache::SchemaCache,
    grader_cache: bool,
    aborted: &AbortFlag,
    skip_on_empty_reason: &[String],
    tools: &[crate::config::ToolDef],
    cache_state: &runner_cache::CacheRunState,
    policy: &crate::empty_policy::EmptyPolicy,
    fallback_policy: &super::runner_fallback::FallbackPolicy,
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

    // Checked before the *provider* call, not just before grading: once the
    // grader's credential is known bad, paying a model to produce output that
    // will never be graded is the expensive half of the failure this prevents.
    if let Some(reason) = aborted.reason() {
        return error_case(
            cell,
            case_key,
            name,
            test,
            CallFailure::before_any_attempt(ErrorClass::PROVIDER_AUTH, reason),
            0,
            CaseInputs::default(),
        );
    }

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
                CallFailure::before_any_attempt(
                    ErrorClass::RENDER_FAILED,
                    format!("rendering vars: {e}"),
                ),
                0,
                CaseInputs::default(),
            )
        }
    };
    // Persisted verbatim into `CaseResult.vars` (the UI's Input view). Cloned
    // because `rendered_vars` is moved into the provider request below.
    let case_vars = rendered_vars.clone();
    let var_ctx = render::context_with_env(&rendered_vars);
    // The case's prior turns, rendered against the same context as the prompt.
    // They reach the request only inside `rendered_prompt`, so they join
    // `prompt_digest` and the cache key with no code of their own.
    let history = match &test.history {
        Some(spec) => match render::resolve_history(spec, &var_ctx, engine, base_dir) {
            Ok(h) => h,
            Err(e) => {
                return error_case(
                    cell,
                    case_key,
                    name,
                    test,
                    CallFailure::before_any_attempt(
                        ErrorClass::RENDER_FAILED,
                        format!("rendering history: {e}"),
                    ),
                    0,
                    CaseInputs {
                        vars: case_vars,
                        ..Default::default()
                    },
                )
            }
        },
        None => Vec::new(),
    };
    let rendered_prompt = match prompt {
        Some(p) => {
            match render::render_prompt_with_history(p, &var_ctx, engine, base_dir, &history) {
                Ok(rp) => Some(rp),
                Err(e) => {
                    return error_case(
                        cell,
                        case_key,
                        name,
                        test,
                        CallFailure::before_any_attempt(
                            ErrorClass::RENDER_FAILED,
                            format!("rendering prompt: {e}"),
                        ),
                        0,
                        CaseInputs {
                            vars: case_vars,
                            ..Default::default()
                        },
                    )
                }
            }
        }
        // No `prompts:` block: the history is the whole transcript.
        None if !history.is_empty() => Some(crate::types::RenderedPrompt::Messages(history)),
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
        tools: tools.to_vec(),
    };

    // Computed here, not inside `call_with_cache`'s `use_cache` gate: the cache
    // key is skipped entirely under `--no-cache`, for a case with a `latency`
    // assert, and for an unsalted `exec` provider — which is exactly the set of
    // runs a CI comparison cares about. Identity must not depend on caching.
    let prompt_digest = Some(crate::digests::prompt_digest(&req));
    // The configured provider's, for the error paths that never reach a call.
    // Once a chain has run, the answering link's replaces it.
    let provider_digest = Some(crate::digests::provider_digest(&provider.fingerprint()));

    // What this provider will actually send, built by the provider itself from
    // the same code path as the call. Captured *before* the call so a failed
    // case still carries it — which is where it earns its keep: an HTTP 404
    // explains itself the moment the model id in the request is visible.
    //
    // Built only when it will be persisted. For `http` a preview is a full
    // template render — engine, env snapshot, url/headers/body — per case, and
    // under `--no-raw` every byte of it was being discarded. The cache no longer
    // depends on this call: it keys on `canonical_request`, which the provider
    // builds for itself inside `call_with_cache`.
    // NOT computed here for the ordinary path: `call_chain` renders one per
    // link, because each names a different model at a different address. An
    // `http` preview is a full template render per case, so rendering it twice
    // for the primary would double that cost on every default run. The one
    // early return below that never reaches a call builds its own.

    // Latency assertions must not observe a cached (near-zero) latency.
    let bypass_cache = has_latency_assert(&test.assert);
    // ...but under `--cache-only` that bypass is a live call in the one mode
    // documented as offline — and the mode the credential preflight above is
    // skipped for. Refuse the case instead of quietly reaching the provider.
    // Per case, like a strict-mode miss: the rest of the suite still replays.
    if bypass_cache && cache_mode == CacheMode::ReadOnlyStrict {
        return error_case(
            cell,
            case_key,
            name,
            test,
            CallFailure::before_any_attempt(
                ErrorClass::CACHE_MISS,
                format!(
                    "cache-only: test '{test_id}' has a latency assert, which always \
                     measures a live call; there is nothing honest to replay"
                ),
            ),
            0,
            CaseInputs {
                prompt: rendered_prompt,
                vars: case_vars,
                request: include_raw
                    .then(|| json_to_persist(true, provider.request_preview(&req), "request"))
                    .flatten(),
                prompt_digest,
                provider_digest,
                answered_by_provider_id: None,
                fallback_attempts: Vec::new(),
            },
        );
    }
    let effective_mode = if bypass_cache {
        CacheMode::Disabled
    } else {
        cache_mode
    };

    // One clock for the whole chain, so `wall_ms` honestly covers every handoff
    // rather than only the link that happened to answer.
    let start = Instant::now();
    let chain = super::runner_fallback::call_chain(
        provider,
        fallbacks,
        &req,
        ctx,
        super::runner_fallback::ChainCtx {
            cache: runner_cache::CacheCall {
                probe_legacy: true,
                backend: cache,
                mode: effective_mode,
                repeat,
                state: cache_state,
                policy,
            },
            retry_cfg,
            empty_policy: policy,
            fallback: fallback_policy,
            include_raw,
            aborted,
            has_latency_assert: bypass_cache,
        },
    )
    .await;
    // Recomputed per link, so both name whoever actually answered — the same
    // truthfulness rule `provider_digest` already followed for a single call.
    let provider_digest = chain.provider_digest;
    let request = chain
        .request
        .and_then(|r| json_to_persist(true, Some(r), "request"));
    let answered_by_provider_id =
        (chain.answered_by.id() != cell.provider_id).then(|| chain.answered_by.id().to_string());
    let fallback_attempts = chain.attempted;
    let provider = chain.answered_by;
    let outcome = match chain.outcome {
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
                    answered_by_provider_id,
                    fallback_attempts,
                },
            );
        }
    };
    let CallOutcome {
        response,
        cached,
        attempts,
        provider_latency_ms,
        cache_key,
    } = outcome;
    let attempts = attempts.unwrap_or(0);
    let wall_ms = start.elapsed().as_millis() as u64;
    // Provider time, never wall time: `latency` assertions read this, and
    // charging retry backoff to it fails them on a model that answered fast.
    // Entries predating the field fall back to wall time, as they always did.
    let latency_ms = provider_latency_ms.unwrap_or(wall_ms);
    // After the cache read, so a replayed hit carries the same diagnosis a fresh
    // call would. Keyed on the reason being present, never on the output's shape.
    // Through the run's policy rather than `provider.classify_empty` directly,
    // so a prose refusal caught by `runner.refusal_patterns` is diagnosed the
    // same way here and at the cache write gate. Two classifiers would mean a
    // response graded as an answer and stored as an empty, or the reverse — and
    // the disagreement would only show on the second run.
    let empty_reason = policy.effective_reason(provider, &response);
    let reasoning = response.reasoning.clone();

    let metrics = MetricCtx {
        latency_ms,
        cost_usd: response.cost_usd,
        total_tokens: response.usage.as_ref().map(|u| u.total()),
        billable_tokens: response.usage.as_ref().map(|u| u.billable_total()),
    };

    let (assert_results, scored, assert_error_classes) = evaluate_asserts(
        &AssertCtx {
            provider_id: provider.id(),
            test_id: &test_id,
            test_tags: &test.tags,
            engine,
            grader,
            base_dir,
            schemas,
            cache,
            cache_mode,
            grader_cache,
            migration: &cache_state.migration,
            repeat,
            aborted,
            tool_calls: &response.tool_calls,
            empty_reason: empty_reason.as_ref(),
        },
        &test.assert,
        &response.output,
        &var_ctx,
        &metrics,
        test.threshold,
    )
    .await;

    // `Some` exactly when at least one assert errored, so it carries both the
    // diagnosis and the "did anything error" verdict input.
    let assert_error = assert_error_message(&assert_results);
    // Computed before the results are moved into the case below.
    let assert_digest = crate::digests::assert_digest(&assert_results, test.threshold);
    let assert_error_class = crate::error_class::most_specific(&assert_error_classes);
    let verdict = case_verdict(&scored, test.threshold);
    // `Error` first: a grader that broke is a fact about the run, and outranks
    // any statement about whether the output was gradeable.
    let status = if assert_error.is_some() {
        CaseStatus::Error
    // The *classified* reason — what the case reports — so `["blank"]` can skip a blank output.
    } else if reasoning_is_skippable(empty_reason.as_ref(), skip_on_empty_reason) {
        CaseStatus::Skip
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
        cache_key: cache_key.map(|k| k.0),
        prompt: rendered_prompt,
        request,
        stop_reason: response.stop_reason,
        model: response.model,
        tool_calls: response.tool_calls.clone(),
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
        // A graded case reached the provider, so any error here came from an
        // assertion rather than the call; assert-level detail lives on each
        // AssertResult.
        error_details: None,
        error_class: assert_error_class,
        reasoning,
        empty_reason,
        answered_by_provider_id,
        fallback_attempts,
    }
}
