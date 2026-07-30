//! The default grader for the non-local assert kinds (`exec`, `llm-rubric`).
//!
//! The llm-rubric path never parses a verdict out of prose: anthropic uses a
//! forced `submit_verdict` tool call, openai-compatible endpoints use a strict
//! `json_schema` response. Everything fails closed — a missing, unparseable, or
//! truncated verdict is a failure, never a silent pass.

#[path = "grader_llm.rs"]
mod grader_llm;

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::cache::{EntryKind, Graded, GradedVerdict};
use crate::config::{Assert, AssertKind, Grader, ProviderKind};
use crate::errors::GraderError;
use crate::exec::run_exec_json;
use crate::exec_protocol::{AssertReq, AssertResp, Envelope, Kind, ProviderRef, TestRef};
use crate::net::http_client;
use crate::request_cache::{cached_exchange, EntryMeta, Exchange, LegacyVerdict, Served};
use crate::runner::{AssertGrader, GradeCtx};
use crate::types::{Output, TokenUsage};

use grader_llm::Judge;

/// Default ceiling on a grading call. Overridable per suite via
/// `grader.timeout_ms`, because a reasoning grader given a generous
/// `max_tokens` can legitimately outlast a fixed constant — and when it does,
/// the timeout reads as a transport fault rather than the budget problem it is.
const GRADER_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_GRADER_MAX_TOKENS: u64 = 4096;

/// The built-in grading system prompt.
///
/// `pub` because it is in the judge's request body and therefore in the cache
/// key: an embedder replicating a key, or a test seeding a ≤0.4.x verdict entry
/// to be adopted, needs the same bytes rather than a copy that can drift.
pub const SYSTEM_PROMPT: &str =
    "You are a strict evaluator. Grade the ASSISTANT OUTPUT against the \
RUBRIC. Return a boolean `pass`, a `score` in [0,1], and brief `reasoning`. Judge only what the \
rubric asks; do not reward effort.";

/// The default grader.
pub struct DefaultGrader {
    default_grader: Option<Grader>,
    embeddings: Option<crate::embeddings::EmbeddingsProvider>,
    /// Resolved judge rates, memoized per `(model, pricing)`.
    ///
    /// The chat providers resolve their rate once at construction, which is what
    /// makes the unknown-model warning fire once per run. A grader cannot: a
    /// per-assert `grader:` block is only known when that assertion is graded.
    /// This memo restores the same property without process-global state — one
    /// map per grader, so a shared test binary cannot leak a warning between
    /// runs. The mutex is held for a map lookup, never across a request.
    rates: std::sync::Mutex<BTreeMap<String, Option<crate::pricing::ModelRate>>>,
    /// The resolved per-call ceiling. Kept alongside the client that already
    /// carries it, because an `exec` assert has no client to carry it — without
    /// this, `grader.timeout_ms` silently applied to the HTTP judges only.
    timeout: Duration,
    client: reqwest::Client,
}

impl DefaultGrader {
    /// Construct with an optional default grader and an optional embeddings
    /// provider (required for `similar` assertions).
    pub fn new(default_grader: Option<Grader>) -> Self {
        let timeout = default_grader
            .as_ref()
            .and_then(|g| g.timeout_ms)
            .map(Duration::from_millis)
            .unwrap_or(GRADER_TIMEOUT);
        DefaultGrader {
            default_grader,
            embeddings: None,
            rates: std::sync::Mutex::new(BTreeMap::new()),
            timeout,
            client: http_client(timeout),
        }
    }

    /// The rate for a judge model, resolved once per distinct `(model, pricing)`
    /// pair in this run.
    fn judge_rate(
        &self,
        model: &str,
        pricing: Option<&crate::config::PricingCfg>,
    ) -> Option<crate::pricing::ModelRate> {
        let key = crate::cache::canonical_json(&json!({"model": model, "pricing": pricing}));
        let mut rates = match self.rates.lock() {
            Ok(guard) => guard,
            // A poisoned mutex means another thread panicked while resolving a
            // rate. Cost is instrumentation, so recover the guard and carry on
            // rather than propagating a panic into every remaining grading.
            Err(poisoned) => poisoned.into_inner(),
        };
        rates
            .entry(key)
            .or_insert_with(|| crate::pricing::resolve_rate("grader", model, pricing))
            .clone()
    }

    /// Attach an embeddings provider (enables `similar` assertions).
    pub fn with_embeddings(mut self, embeddings: crate::embeddings::EmbeddingsProvider) -> Self {
        self.embeddings = Some(embeddings);
        self
    }
}

#[async_trait]
impl AssertGrader for DefaultGrader {
    #[tracing::instrument(name = "grade", skip_all, fields(kind = assert.kind.name().as_str()))]
    async fn grade(
        &self,
        assert: &Assert,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
        // `negate` is applied when the verdict becomes an outcome, not here:
        // caching a *negated* verdict would key the cache on the assertion's
        // polarity, so flipping `negate` would re-pay the judge for the same
        // answer. Likewise `threshold` — it is read by
        // `GradedVerdict::to_outcome`, so editing one re-scores from cache
        // instead of re-grading.
        match &assert.kind {
            AssertKind::Exec { .. } => self.grade_exec(assert, output, ctx).await,
            AssertKind::LlmRubric { .. } => self.grade_llm_rubric(assert, output, ctx).await,
            AssertKind::Similar { value, .. } => self.grade_similar(value, output, ctx).await,
            _ => Err(GraderError::Internal("local assert routed to grader")),
        }
    }
}

fn output_to_json(output: &Output) -> Json {
    match output {
        Output::Text(s) => Json::String(s.clone()),
        Output::Json(v) => v.clone(),
    }
}

/// Add two token counts, treating "neither reported" as nothing reported.
///
/// `None + Some(u)` is `Some(u)`: one call reporting usage and the other not is
/// a partial count, but it is the only count there is, and reporting nothing
/// would lose it entirely. The *cost* is stricter — see `grade_similar`.
fn sum_usage(a: Option<TokenUsage>, b: Option<TokenUsage>) -> Option<TokenUsage> {
    fn add(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        match (a, b) {
            (None, None) => None,
            (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
        }
    }
    match (a, b) {
        (None, None) => None,
        (a, b) => {
            let (a, b) = (a.unwrap_or_default(), b.unwrap_or_default());
            Some(TokenUsage {
                input_tokens: a.input_tokens.saturating_add(b.input_tokens),
                output_tokens: a.output_tokens.saturating_add(b.output_tokens),
                cache_read_tokens: add(a.cache_read_tokens, b.cache_read_tokens),
                cache_write_tokens: add(a.cache_write_tokens, b.cache_write_tokens),
                cache_write_1h_tokens: add(a.cache_write_1h_tokens, b.cache_write_1h_tokens),
            })
        }
    }
}

/// The ≤0.4.x verdict key's two halves for this grading, when it has history.
///
/// The one place the retired key space is still computed, and it is computed
/// from [`crate::cache_migrate`]'s frozen copies rather than from anything live:
/// see that module for why the shapes are literals and when they are deleted.
/// `None` means nothing to probe — a `similar` assertion, or a rubric with no
/// grader resolved — and the exchange goes straight to the strict-miss or live
/// branch.
fn legacy_verdict(
    assert: &Assert,
    default_grader: Option<&Grader>,
    graded: &crate::cache_migrate::LegacyGraded<'_>,
    base_dir: Option<&std::path::Path>,
) -> Option<LegacyVerdict> {
    let fingerprint = crate::cache_migrate::legacy_grading_fingerprint(
        assert,
        default_grader,
        SYSTEM_PROMPT,
        base_dir,
    )?;
    let graded = crate::cache_migrate::legacy_graded_payload(assert, graded)?;
    Some(LegacyVerdict {
        fingerprint,
        graded,
    })
}

/// The grader provider's params with the assertion's own merged over them.
///
/// `AssertKind::LlmRubric.params` was parsed, schema'd, documented and never
/// read — so a per-assertion `temperature` or `max_tokens` silently did
/// nothing. Assertion wins on a key collision: it is the more specific of the
/// two, and a suite sets it precisely to deviate from the shared default.
fn merge_params(
    provider: Option<&crate::config::ParamMap>,
    assert: Option<&crate::config::ParamMap>,
) -> Option<crate::config::ParamMap> {
    match (provider, assert) {
        (None, None) => None,
        (Some(p), None) => Some(p.clone()),
        (None, Some(a)) => Some(a.clone()),
        (Some(p), Some(a)) => {
            let mut merged = p.clone();
            merged.extend(a.iter().map(|(k, v)| (k.clone(), v.clone())));
            Some(merged)
        }
    }
}

/// Read the file behind a `grader.template` spec, relative to the suite.
///
/// Relative to `base_dir` and through [`crate::sandbox`], like every other
/// `file://` reference a suite can write. Reading it against the *process* cwd
/// instead meant `template: "file://prompts/judge.md"` worked only when the
/// suite happened to be run from its own directory — and, being outside the
/// sandbox, that `file://../../etc/passwd` was a path the loader accepted.
fn read_grader_template(
    spec: &str,
    base_dir: Option<&std::path::Path>,
) -> Result<String, GraderError> {
    let rel = spec.strip_prefix("file://").ok_or_else(|| {
        GraderError::Misconfigured(format!(
            "grader.template must be a `file://` reference, got `{spec}`"
        ))
    })?;
    let path = match base_dir {
        Some(dir) => crate::sandbox::resolve_within(dir, rel)
            .map_err(|e| GraderError::Misconfigured(e.to_string()))?,
        None => std::path::PathBuf::from(rel),
    };
    std::fs::read_to_string(&path)
        .map_err(|e| GraderError::Misconfigured(format!("reading grader.template `{rel}`: {e}")))
}

const TOOL_CALLS_PLACEHOLDER: &str = "{{tool_calls}}";

/// The tool calls as the judge sees them: a JSON array of `{name, arguments}`.
///
/// The vendor's call id is deliberately dropped. It is a fresh random token on
/// every live response (`toolu_…`), so carrying it would put a different string
/// in the judge's request body — and therefore a different cache key — for a
/// decision the model made identically twice. Nothing a rubric can sensibly ask
/// is answered by it.
///
/// An empty slice pretty-prints to `[]`, which is the point rather than a case
/// to special-case: the section has to be able to say "it called nothing".
fn format_tool_calls(calls: &[crate::result::ToolCall]) -> String {
    let view: Vec<Json> = calls
        .iter()
        .map(|c| json!({"name": c.name, "arguments": c.arguments}))
        .collect();
    serde_json::to_string_pretty(&view).unwrap_or_else(|_| "[]".to_string())
}

/// Substitute placeholders in ONE left-to-right pass.
///
/// Chained `str::replace` rescans: whatever the first pass substituted is
/// visible to the second, so a rubric containing the literal `{{output}}` had
/// the model's answer spliced into it. With three placeholders there is no
/// ordering that avoids it either — a single pass is the only shape where a
/// substituted value is never itself a placeholder.
fn substitute_once(template: &str, replacements: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    loop {
        let next = replacements
            .iter()
            .filter_map(|(key, value)| rest.find(key).map(|at| (at, *key, *value)))
            .min_by_key(|(at, _, _)| *at);
        match next {
            Some((at, key, value)) => {
                out.push_str(&rest[..at]);
                out.push_str(value);
                rest = &rest[at + key.len()..];
            }
            None => {
                out.push_str(rest);
                return out;
            }
        }
    }
}

/// Render a `grader.template` override into the grading prompt.
///
/// Another field that was parsed and never read. The contract is three
/// placeholders — `{{rubric}}`, `{{output}}` and `{{tool_calls}}` — substituted
/// literally rather than through the template engine, because the output *and*
/// the tool arguments are untrusted model text and running either through
/// minijinja would make a grading prompt an SSTI surface.
///
/// `tool_calls` is `Some` exactly when `grader.include_tool_calls` is on. The
/// two settings come out of the same authored `grader:` block, so disagreeing
/// is an internal contradiction rather than a default to apply: a placeholder
/// with the flag off would be sent to the judge verbatim, and the flag on with
/// no placeholder is a suite that asked for something it never receives.
/// Missing `{{rubric}}` or `{{output}}` stays tolerated — a template is allowed
/// to grade on less than everything.
fn render_grader_template(
    spec: &str,
    base_dir: Option<&std::path::Path>,
    rubric: &str,
    output: &str,
    tool_calls: Option<&str>,
) -> Result<String, GraderError> {
    let text = read_grader_template(spec, base_dir)?;
    match (tool_calls, text.contains(TOOL_CALLS_PLACEHOLDER)) {
        (None, true) => {
            return Err(GraderError::Misconfigured(format!(
                "grader.template uses `{TOOL_CALLS_PLACEHOLDER}` but \
                 grader.include_tool_calls is not set"
            )))
        }
        (Some(_), false) => {
            return Err(GraderError::Misconfigured(format!(
                "grader.include_tool_calls is set but grader.template has no \
                 `{TOOL_CALLS_PLACEHOLDER}` to put them in"
            )))
        }
        _ => {}
    }
    let mut replacements = vec![("{{rubric}}", rubric), ("{{output}}", output)];
    if let Some(calls) = tool_calls {
        replacements.push((TOOL_CALLS_PLACEHOLDER, calls));
    }
    Ok(substitute_once(&text, &replacements))
}

/// The judge's user message: the whole grading prompt, and the whole of what
/// separates one judge cache entry from another.
///
/// Split out of `grade_llm_rubric` so the bytes are testable without a judge.
/// With `include_tool_calls` unset this is exactly the string the built-in
/// framing produced before the flag existed, down to the byte — the flag is
/// opt-in precisely so that stays true and no warm entry is re-graded.
fn grading_user_message(
    grader: &Grader,
    working_dir: Option<&std::path::Path>,
    rubric: &str,
    output_text: &str,
    tool_calls: &[crate::result::ToolCall],
) -> Result<String, GraderError> {
    let calls = grader
        .include_tool_calls
        .unwrap_or(false)
        .then(|| format_tool_calls(tool_calls));
    match &grader.template {
        Some(spec) => {
            render_grader_template(spec, working_dir, rubric, output_text, calls.as_deref())
        }
        // The framing lives here rather than in `SYSTEM_PROMPT`: that constant
        // is in every judge request body and is a published adoption contract,
        // so a section that only some runs have cannot be described in it.
        None => Ok(match &calls {
            Some(json) => format!(
                "RUBRIC:\n{rubric}\n\nASSISTANT OUTPUT:\n{output_text}\n\n\
                 TOOL CALLS (the tool calls the assistant made, in order, as JSON):\n{json}"
            ),
            None => format!("RUBRIC:\n{rubric}\n\nASSISTANT OUTPUT:\n{output_text}"),
        }),
    }
}

impl DefaultGrader {
    async fn grade_exec(
        &self,
        assert: &Assert,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
        let AssertKind::Exec {
            command,
            config,
            cache_salt,
        } = &assert.kind
        else {
            return Err(GraderError::Internal("grade_exec on a non-exec assert"));
        };
        // `test` and `provider` used to be sent as empty strings, and `vars`
        // was discarded outright — three fields the wire format declares as
        // populated, so a child written against `docs/protocol.md` received
        // stubs. `meta` carries the real values; `vars` is a protocol addition.
        let request = AssertReq {
            envelope: Envelope::new(Kind::Assert),
            output: output_to_json(output),
            test: TestRef {
                id: ctx.test_id.to_string(),
                tags: ctx.test_tags.to_vec(),
            },
            prompt: None,
            provider: ProviderRef {
                id: ctx.provider_id.to_string(),
            },
            config: config.clone().unwrap_or(Json::Null),
            vars: ctx.vars.clone(),
            // No flag on this side: the field is `skip_serializing_if`, so a
            // tool-less cell writes the same bytes it always did and a child
            // that never reads it cannot tell the difference.
            tool_calls: ctx
                .tool_calls
                .iter()
                .map(|c| crate::exec_protocol::ToolCall {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                })
                .collect(),
        };
        let request = serde_json::to_value(&request)
            .map_err(|e| GraderError::InvalidVerdict(format!("serializing assert request: {e}")))?;

        let served = cached_exchange(
            ctx.cache.as_ref(),
            Exchange {
                canonical: exec_assert_canonical(command, &request),
                kind: EntryKind::new(EntryKind::EXEC_ASSERT),
                case_salt: cache_salt.as_deref(),
                // Nothing to adopt once there are calls to send: a ≤0.4.x child
                // was handed a request without them and judged whatever it
                // could see, so its verdict does not answer this question.
                legacy: if ctx.tool_calls.is_empty() {
                    legacy_verdict(
                        assert,
                        self.default_grader.as_ref(),
                        &crate::cache_migrate::LegacyGraded {
                            output,
                            rubric: "",
                            vars: ctx.vars,
                            test_id: ctx.test_id,
                            test_tags: ctx.test_tags,
                            provider_id: ctx.provider_id,
                        },
                        ctx.working_dir,
                    )
                } else {
                    None
                },
            },
            |payload| {
                let resp: AssertResp = serde_json::from_value(payload.clone()).map_err(|e| {
                    GraderError::InvalidVerdict(format!("bad assert response: {e}"))
                })?;
                let score = resp.score.unwrap_or(if resp.pass { 1.0 } else { 0.0 });
                Ok(GradedVerdict::Exec {
                    pass: resp.pass,
                    score,
                    reason: resp.reason.unwrap_or_default(),
                    details: resp.details,
                })
            },
            |verdict| EntryMeta {
                output: Output::Text(verdict.to_outcome(None).reason),
                usage: None,
                cost_usd: None,
                model: None,
            },
            |key| {
                GraderError::Transport(format!(
                    "cache-only: miss for the exec assert `{}` on this case ({key})",
                    command.join(" ")
                ))
            },
            async {
                let value = run_exec_json(
                    command,
                    &BTreeMap::new(),
                    ctx.working_dir,
                    self.timeout,
                    &request,
                )
                .await
                .map_err(|e| GraderError::Transport(format!("exec assert failed: {e}")))?;
                Ok(value)
            },
        )
        .await?;

        Ok(match served {
            Served::Verdict(graded) => *graded,
            // Unpriced: the child spends whatever it spends against whatever
            // endpoint it chose, and the protocol gives it no way to say so. A
            // zero here would claim custom grading is free.
            Served::Parsed(parsed) => Graded {
                cached: parsed.cached,
                ..Graded::unpriced(parsed.value)
            },
        })
    }

    async fn grade_similar(
        &self,
        reference: &crate::val::Val,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
        let embeddings = self
            .embeddings
            .as_ref()
            .ok_or(GraderError::Unconfigured { kind: "similar" })?;
        let reference = ctx
            .engine
            .render_val(reference, ctx.vars)
            .map_err(|e| GraderError::Misconfigured(format!("rendering reference: {e}")))?;
        let reference = reference
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| reference.to_string());
        let output_text = output.as_text();
        // Two exchanges rather than one, each keyed on its own embed request.
        // A vector is reusable by any later comparison — the same output
        // measured against a second reference re-embeds only the reference —
        // which is what caching the *requests* buys over caching the cosine.
        //
        // Still concurrent, as it was before the cache existed: the common case
        // is two different texts and therefore two different keys. Two identical
        // texts do miss together and pay twice on a cold run, which is a wasted
        // call rather than a wrong answer — `CacheBackend::put` is
        // first-write-wins — and serializing every `similar` assertion to avoid
        // it would cost a round trip on all of them for the sake of the one
        // where the cosine is trivially 1.
        let (a, b) = tokio::try_join!(
            self.embed_cached(embeddings, &output_text, ctx),
            self.embed_cached(embeddings, &reference, ctx)
        )?;
        // Two calls, so both halves are summed. Either half being unpriced
        // makes the pair unpriced — half a cost presented as a whole one is the
        // same lie the rate table refuses to tell elsewhere.
        let cost_usd = a
            .value
            .cost
            .zip(b.value.cost)
            .map(|(a, b)| a.saturating_add(b).to_usd());
        Ok(Graded {
            // The threshold is applied by `to_outcome`, so what is cached is the
            // raw similarity and changing a threshold costs nothing.
            verdict: GradedVerdict::Similarity {
                cosine: crate::embeddings::cosine(&a.value.vector, &b.value.vector),
            },
            usage: sum_usage(a.value.usage, b.value.usage),
            cost_usd,
            model: None,
            // Both halves, or the assertion still paid an embedder this run.
            cached: a.cached && b.cached,
        })
    }

    /// One embedding, through the cache.
    ///
    /// No legacy ingredients: a ≤0.4.x `similar` entry holds a cosine, which
    /// decomposes into neither vector — see
    /// [`crate::cache_migrate::legacy_graded_payload`] for why re-embedding once
    /// is the better trade.
    async fn embed_cached(
        &self,
        embeddings: &crate::embeddings::EmbeddingsProvider,
        text: &str,
        ctx: &GradeCtx<'_>,
    ) -> Result<crate::request_cache::Parsed<crate::embeddings::Embedded>, GraderError> {
        let (url, body) = embeddings.request(text);
        cached_exchange(
            ctx.cache.as_ref(),
            Exchange {
                canonical: crate::provider::http_request_preview("POST", &url, body.clone()),
                kind: EntryKind::new(EntryKind::EMBEDDING),
                case_salt: None,
                legacy: None,
            },
            |payload| {
                embeddings
                    .parse(payload)
                    .map_err(GraderError::InvalidVerdict)
            },
            |embedded| EntryMeta {
                output: Output::Json(json!({"dims": embedded.vector.len()})),
                usage: embedded.usage.clone(),
                cost_usd: embedded.cost.map(|c| c.to_usd()),
                model: None,
            },
            |key| GraderError::Transport(format!("cache-only: miss for an embedding ({key})")),
            async {
                embeddings
                    .post(&url, &body)
                    .await
                    .map_err(GraderError::Transport)
            },
        )
        .await?
        .parsed_only()
    }

    async fn grade_llm_rubric(
        &self,
        assert: &Assert,
        output: &Output,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
        let AssertKind::LlmRubric {
            value: rubric_template,
            grader: assert_grader,
            params: assert_params,
            ..
        } = &assert.kind
        else {
            return Err(GraderError::Internal("grade_llm_rubric on a non-rubric"));
        };
        // The variant that motivated the whole type: nothing ran, and the fix is
        // to add a `grader:` block — not to retry.
        let grader = assert_grader
            .as_deref()
            .or(self.default_grader.as_ref())
            .ok_or(GraderError::Unconfigured { kind: "llm-rubric" })?;
        let rubric = ctx
            .engine
            .render_str(rubric_template, ctx.vars)
            .map_err(|e| GraderError::Misconfigured(format!("rendering rubric: {e}")))?;
        let output_text = output.as_text();
        // The grading prompt. `grader.template` replaces the built-in framing
        // when set; its placeholders are the whole contract. That file's
        // *contents* land here, which is why editing it busts the key with no
        // separate digest to keep in step.
        let user = grading_user_message(
            grader,
            ctx.working_dir,
            &rubric,
            &output_text,
            ctx.tool_calls,
        )?;

        let (judge, model, base_url, api_key_env, params, pricing) = match &grader.provider {
            ProviderKind::Anthropic {
                model,
                base_url,
                api_key_env,
                params,
                pricing,
            } => (
                Judge::Anthropic,
                model,
                base_url,
                api_key_env,
                params,
                pricing,
            ),
            ProviderKind::Openai {
                model,
                base_url,
                api_key_env,
                params,
                pricing,
            } => (Judge::Openai, model, base_url, api_key_env, params, pricing),
            other => {
                return Err(GraderError::Unsupported {
                    provider: format!("{other:?}"),
                    kind: "llm-rubric",
                })
            }
        };
        let params = merge_params(params.as_ref(), assert_params.as_ref());
        // Before the cache, not inside the live branch: a suite that asks for
        // extended thinking is misconfigured whether or not a verdict happens to
        // be warm, and finding that out only on a cold cache is worse.
        if matches!(judge, Judge::Anthropic) {
            grader_llm::reject_thinking(params.as_ref())?;
        }
        // A judge's cost is not a case's cost — no assertion grades it, and
        // folding it into `cost_usd` would make a `cost:` budget depend on which
        // model you picked to score with. It is still real money, so it is
        // priced here and reported separately.
        let rate = self.judge_rate(model, pricing.as_deref());
        let (url, body) = judge.request(model, base_url.as_deref(), params.as_ref(), &user);

        let served = cached_exchange(
            ctx.cache.as_ref(),
            Exchange {
                // Absorbs everything the deleted `grading_fingerprint`
                // enumerated: model, endpoint, merged params, the system prompt,
                // the rendered rubric, the graded output, and the template's
                // bytes. Credentials are headers, so the envelope excludes them
                // structurally.
                canonical: crate::provider::http_request_preview("POST", &url, body.clone()),
                kind: EntryKind::new(EntryKind::JUDGE),
                case_salt: None,
                // No adoption when the judge is being shown tool calls: a
                // ≤0.4.x verdict was reached without them, so replaying it here
                // would answer a question the old judge was never asked. The
                // frozen key space cannot express the difference, so the only
                // honest option is to re-grade.
                legacy: if grader.include_tool_calls.unwrap_or(false) {
                    None
                } else {
                    legacy_verdict(
                        assert,
                        self.default_grader.as_ref(),
                        &crate::cache_migrate::LegacyGraded {
                            output,
                            rubric: &rubric,
                            vars: ctx.vars,
                            test_id: ctx.test_id,
                            test_tags: ctx.test_tags,
                            provider_id: ctx.provider_id,
                        },
                        ctx.working_dir,
                    )
                },
            },
            // Fail closed on both paths: a truncated or unparseable verdict is
            // an error whether it arrived just now or a month ago.
            |payload| judge.parse(payload, rate.as_ref()),
            |verdict| EntryMeta {
                // The judge's reasoning, so `domarinn cache` inspection shows
                // something a human can read.
                output: Output::Text(verdict.reasoning.clone()),
                usage: verdict.usage.clone(),
                cost_usd: verdict.cost_usd,
                model: verdict.model.clone(),
            },
            |key| {
                GraderError::Transport(format!(
                    "cache-only: miss for the `{model}` judge on this rubric ({key})"
                ))
            },
            async {
                self.post_judge(judge, &url, &body, api_key_env.as_ref())
                    .await
            },
        )
        .await?;

        Ok(match served {
            Served::Verdict(graded) => *graded,
            Served::Parsed(parsed) => {
                let v = &parsed.value;
                Graded {
                    verdict: GradedVerdict::Rubric {
                        score: v.score,
                        pass: v.pass,
                        reasoning: v.reasoning.clone(),
                    },
                    usage: v.usage.clone(),
                    // Re-derived from the replayed payload at today's rate, with
                    // what the entry recorded as the fallback.
                    cost_usd: v.cost_usd.or(parsed.stored_cost_usd),
                    model: v.model.clone(),
                    cached: parsed.cached,
                }
            }
        })
    }
}

/// The keying view of an `exec` assert's request.
///
/// Two members of the sent document are dropped, for the two reasons the
/// provider side drops things:
///
/// - **`test`** — a test id and its tags are correlation metadata, like a
///   request id. Two cases asking the same thing of the same child must keep
///   sharing an entry; a per-assert `cache_salt` is how a suite says otherwise.
///   Same documented exception as
///   [`crate::provider::Provider::canonical_request`].
/// - **`vars.env`** — an assert's `vars` is the *render context*, which carries
///   a snapshot of the whole process environment so `{{ env.X }}` resolves in
///   sibling assertions. Keying it would make every entry a property of one
///   machine's environment, and — since the canonical request is persisted into
///   the entry — would write that machine's secrets into a shared store. The
///   provider path already excludes `env` from its request identity for exactly
///   this reason; this is the same rule, applied where the same context leaks
///   in. The child still receives it: this is a keying view, not a second
///   version of the request.
///
/// The `domarinn` envelope is **deliberately kept**, so `PROTOCOL_VERSION` is a
/// key member here as it is for the `exec` provider. A bump re-keys every
/// exec-assert entry in every store and must ship with the protocol-1 shape
/// frozen in [`crate::cache_migrate`] — see
/// `bumping_the_exec_protocol_version_re_keys_every_exec_entry` in
/// `cache_key.rs`, the tripwire that says so at the moment of the bump.
///
/// No `env_digest` member: an `exec` assert declares no environment of its own
/// (the child is spawned with an empty map), so there is nothing to digest.
fn exec_assert_canonical(command: &[String], request: &Json) -> Json {
    let mut stdin = request.clone();
    if let Some(members) = stdin.as_object_mut() {
        members.remove("test");
        if let Some(vars) = members.get_mut("vars").and_then(|v| v.as_object_mut()) {
            vars.remove("env");
        }
    }
    let (program, args) = command
        .split_first()
        .map(|(p, a)| (p.as_str(), a))
        .unwrap_or(("", &[]));
    crate::provider::exec_request_preview(program, args, stdin)
}

#[cfg(test)]
#[path = "grader_tests.rs"]
mod tests;
