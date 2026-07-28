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

use crate::cache::{Graded, GradedVerdict};
use crate::config::{Assert, AssertKind, Grader, ProviderKind};
use crate::errors::GraderError;
use crate::exec::run_exec_json;
use crate::exec_protocol::{AssertReq, AssertResp, Envelope, Kind, ProviderRef, TestRef};
use crate::net::http_client;
use crate::runner::{AssertGrader, GradeCtx};
use crate::template::TemplateEngine;
use crate::types::{Output, TokenUsage};

/// Default ceiling on a grading call. Overridable per suite via
/// `grader.timeout_ms`, because a reasoning grader given a generous
/// `max_tokens` can legitimately outlast a fixed constant — and when it does,
/// the timeout reads as a transport fault rather than the budget problem it is.
const GRADER_TIMEOUT: Duration = Duration::from_secs(120);
const DEFAULT_GRADER_MAX_TOKENS: u64 = 4096;

/// The built-in grading system prompt.
const SYSTEM_PROMPT: &str = "You are a strict evaluator. Grade the ASSISTANT OUTPUT against the \
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
        let GradeCtx {
            vars,
            engine,
            working_dir,
            ..
        } = ctx;
        let outcome = match &assert.kind {
            AssertKind::Exec {
                command,
                config,
                // Cacheability only; it never reaches the child.
                cache_salt: _,
            } => {
                self.grade_exec(command, config.as_ref(), output, vars, *working_dir, ctx)
                    .await
            }
            AssertKind::LlmRubric {
                value,
                grader,
                threshold,
                params,
            } => {
                self.grade_llm_rubric(
                    value,
                    grader.as_deref(),
                    *threshold,
                    output,
                    params.as_ref(),
                    ctx,
                )
                .await
            }
            AssertKind::Similar { value, threshold } => {
                self.grade_similar(value, *threshold, output, vars, engine)
                    .await
            }
            _ => Err(GraderError::Internal("local assert routed to grader")),
        };
        // `negate` is applied when the verdict becomes an outcome, not here:
        // caching a *negated* verdict would key the cache on the assertion's
        // polarity, so flipping `negate` would re-pay the judge for the same
        // answer.
        outcome
    }

    /// The grader's identity for this assertion, or `None` to skip caching it.
    fn grading_fingerprint(&self, assert: &Assert) -> Option<Json> {
        grading_fingerprint(
            self.default_grader.as_ref(),
            self.embeddings.as_ref().map(|e| e.identity()),
            assert,
        )
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

/// A stable identity for the grading `assert` will perform.
///
/// Everything that can move a verdict, and nothing that cannot. Notably absent:
///
/// - **`threshold`** — a decision *about* a verdict, not part of one. Excluding
///   it is what makes editing a threshold re-score from cache instead of
///   re-paying the judge, and it is structurally absent rather than merely
///   omitted: [`GradedVerdict`] has no threshold to include.
/// - **the API key env var** — a secret, same rule as `Provider::fingerprint`.
/// - **`vars`** — the rubric is rendered *before* it is hashed and the output is
///   in the key, so any var that can move a verdict already moves one of them.
///
/// [`SYSTEM_PROMPT`] is hashed in: it is a literal in this file that shapes
/// every verdict, and nothing else in the key would notice an edit to it.
///
/// `template` and `verdict_mode` are included at their *effective* values even
/// though the first is now read and the second is rejected — so wiring either
/// up further needs no cache-version bump.
fn grading_fingerprint(
    default_grader: Option<&Grader>,
    embeddings: Option<Json>,
    assert: &Assert,
) -> Option<Json> {
    fn provider_identity(kind: &ProviderKind) -> Option<Json> {
        match kind {
            ProviderKind::Anthropic {
                model,
                base_url,
                params,
                ..
            } => Some(
                json!({"type": "anthropic", "model": model, "base_url": base_url, "params": params}),
            ),
            ProviderKind::Openai {
                model,
                base_url,
                params,
                ..
            } => Some(
                json!({"type": "openai", "model": model, "base_url": base_url, "params": params}),
            ),
            ProviderKind::Embeddings {
                model,
                base_url,
                params,
                ..
            } => Some(
                json!({"type": "embeddings", "model": model, "base_url": base_url, "params": params}),
            ),
            _ => None,
        }
    }

    let system_digest = format!("{}", blake3::hash(SYSTEM_PROMPT.as_bytes()).to_hex());

    match &assert.kind {
        AssertKind::LlmRubric { grader, params, .. } => {
            let g = grader.as_deref().or(default_grader)?;
            Some(json!({
                "assert": "llm-rubric",
                "provider": provider_identity(&g.provider)?,
                "template": g.template,
                "verdict_mode": g.verdict_mode.unwrap_or_default(),
                "assert_params": params,
                "system_prompt": system_digest,
            }))
        }
        // The embeddings client's identity, not merely "one exists": a cosine
        // value is a property of the model that produced the vectors, so a key
        // that omitted the model would replay one embedder's answers after the
        // suite switched to another.
        AssertKind::Similar { .. } => {
            embeddings.map(|e| json!({"assert": "similar", "embeddings": e}))
        }
        // Cached by default, like everything else that can be. `program` is
        // what makes that safe: argv alone does not move when the binary behind
        // it is rebuilt, so a key over `command` would serve stale verdicts
        // after a rebuild — silently, and in CI. `cache_salt` remains the
        // escape hatch for a program the identity cannot see.
        AssertKind::Exec {
            command,
            cache_salt,
            config: _,
        } => Some(json!({
            "assert": "exec",
            "command": command,
            "program": crate::exec::program_identity(command),
            "cache_salt": cache_salt,
        })),
        _ => None,
    }
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

/// Render a `grader.template` override into the grading prompt.
///
/// Another field that was parsed and never read. The contract is two
/// placeholders — `{{rubric}}` and `{{output}}` — substituted literally rather
/// than through the template engine, because the *output* is untrusted model
/// text and running it through minijinja would make a grading prompt an SSTI
/// surface.
fn render_grader_template(spec: &str, rubric: &str, output: &str) -> Result<String, GraderError> {
    let path = spec.strip_prefix("file://").ok_or_else(|| {
        GraderError::Misconfigured(format!(
            "grader.template must be a `file://` reference, got `{spec}`"
        ))
    })?;
    let text = std::fs::read_to_string(path).map_err(|e| {
        GraderError::Misconfigured(format!("reading grader.template `{path}`: {e}"))
    })?;
    Ok(text
        .replace("{{rubric}}", rubric)
        .replace("{{output}}", output))
}

impl DefaultGrader {
    async fn grade_exec(
        &self,
        command: &[String],
        config: Option<&Json>,
        output: &Output,
        vars: &Json,
        working_dir: Option<&std::path::Path>,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
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
            config: config.cloned().unwrap_or(Json::Null),
            vars: vars.clone(),
        };
        let request = serde_json::to_value(&request)
            .map_err(|e| GraderError::InvalidVerdict(format!("serializing assert request: {e}")))?;
        let value = run_exec_json(
            command,
            &BTreeMap::new(),
            working_dir,
            self.timeout,
            &request,
        )
        .await
        .map_err(|e| GraderError::Transport(format!("exec assert failed: {e}")))?;
        let resp: AssertResp = serde_json::from_value(value)
            .map_err(|e| GraderError::InvalidVerdict(format!("bad assert response: {e}")))?;
        let score = resp.score.unwrap_or(if resp.pass { 1.0 } else { 0.0 });
        // Unpriced: the child spends whatever it spends against whatever
        // endpoint it chose, and the protocol gives it no way to say so. A
        // zero here would claim custom grading is free.
        Ok(Graded::unpriced(GradedVerdict::Exec {
            pass: resp.pass,
            score,
            reason: resp.reason.unwrap_or_default(),
            details: resp.details,
        }))
    }

    async fn grade_similar(
        &self,
        reference: &crate::val::Val,
        threshold: Option<f64>,
        output: &Output,
        vars: &Json,
        engine: &TemplateEngine,
    ) -> Result<Graded, GraderError> {
        let embeddings = self
            .embeddings
            .as_ref()
            .ok_or(GraderError::Unconfigured { kind: "similar" })?;
        let reference = engine
            .render_val(reference, vars)
            .map_err(|e| GraderError::Misconfigured(format!("rendering reference: {e}")))?;
        let reference = reference
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| reference.to_string());
        let output_text = output.as_text();
        let (a, b) = tokio::try_join!(embeddings.embed(&output_text), embeddings.embed(&reference))
            .map_err(GraderError::Transport)?;
        // The threshold is applied by `to_outcome`, so the cached verdict is the
        // raw similarity and changing a threshold costs nothing.
        let _ = threshold;
        // Two calls, so both halves are summed. Either half being unpriced
        // makes the pair unpriced — half a cost presented as a whole one is the
        // same lie the rate table refuses to tell elsewhere.
        let cost_usd = a
            .cost
            .zip(b.cost)
            .map(|(a, b)| a.saturating_add(b).to_usd());
        Ok(Graded {
            verdict: GradedVerdict::Similarity {
                cosine: crate::embeddings::cosine(&a.vector, &b.vector),
            },
            usage: sum_usage(a.usage, b.usage),
            cost_usd,
            model: None,
        })
    }

    async fn grade_llm_rubric(
        &self,
        rubric_template: &str,
        assert_grader: Option<&Grader>,
        threshold: Option<f64>,
        output: &Output,
        assert_params: Option<&crate::config::ParamMap>,
        ctx: &GradeCtx<'_>,
    ) -> Result<Graded, GraderError> {
        let (vars, engine) = (ctx.vars, ctx.engine);
        // The variant that motivated the whole type: nothing ran, and the fix is
        // to add a `grader:` block — not to retry.
        let grader = assert_grader
            .or(self.default_grader.as_ref())
            .ok_or(GraderError::Unconfigured { kind: "llm-rubric" })?;
        let rubric = engine
            .render_str(rubric_template, vars)
            .map_err(|e| GraderError::Misconfigured(format!("rendering rubric: {e}")))?;
        let output_text = output.as_text();
        // The grading prompt. `grader.template` replaces the built-in framing
        // when set; the two placeholders are the whole contract.
        let user = match &grader.template {
            Some(spec) => render_grader_template(spec, &rubric, &output_text)?,
            None => format!("RUBRIC:\n{rubric}\n\nASSISTANT OUTPUT:\n{output_text}"),
        };

        // A judge's cost is not a case's cost — no assertion grades it, and
        // folding it into `cost_usd` would make a `cost:` budget depend on which
        // model you picked to score with. It is still real money, so it is
        // priced here and reported separately.
        let verdict = match &grader.provider {
            ProviderKind::Anthropic {
                model,
                base_url,
                api_key_env,
                params,
                pricing,
            } => {
                self.anthropic_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_ref(),
                    merge_params(params.as_ref(), assert_params).as_ref(),
                    self.judge_rate(model, pricing.as_deref()).as_ref(),
                    &user,
                )
                .await
            }
            ProviderKind::Openai {
                model,
                base_url,
                api_key_env,
                params,
                pricing,
            } => {
                self.openai_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_ref(),
                    merge_params(params.as_ref(), assert_params).as_ref(),
                    self.judge_rate(model, pricing.as_deref()).as_ref(),
                    &user,
                )
                .await
            }
            other => Err(GraderError::Unsupported {
                provider: format!("{other:?}"),
                kind: "llm-rubric",
            }),
        };

        // Fail closed: any grader problem is an error, surfaced to the runner.
        // Passed through unwrapped — re-wrapping it in prose here is exactly
        // what erased the distinction between "unconfigured" and "broke".
        let v = verdict?;
        let _ = threshold;
        Ok(Graded {
            verdict: GradedVerdict::Rubric {
                score: v.score,
                pass: v.pass,
                reasoning: v.reasoning,
            },
            usage: v.usage,
            cost_usd: v.cost_usd,
            model: v.model,
        })
    }
}

#[cfg(test)]
mod tests {
    /// Identity for a cell under test. The values are arbitrary; what matters
    /// is that they are no longer empty strings on the wire.
    fn grade_ctx<'a>(vars: &'a Json, engine: &'a TemplateEngine) -> GradeCtx<'a> {
        GradeCtx {
            vars,
            engine,
            working_dir: None,
            provider_id: "p",
            test_id: "t",
            test_tags: &[],
        }
    }

    use super::*;
    use crate::config::Grader;
    use crate::error_class::ErrorClass;
    use crate::errors::Classify;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn anthropic_grader(uri: &str) -> Grader {
        Grader {
            provider: ProviderKind::Anthropic {
                model: "claude-x".into(),
                base_url: Some(uri.to_string()),
                api_key_env: Some("GRADER_TEST_KEY".into()),
                params: None,
                pricing: None,
            },
            template: None,
            verdict_mode: None,
            timeout_ms: None,
        }
    }

    fn rubric_assert() -> Assert {
        Assert {
            weight: 1.0,
            negate: false,
            kind: AssertKind::LlmRubric {
                value: "declines the task".into(),
                grader: None,
                threshold: None,
                params: None,
            },
        }
    }

    #[tokio::test]
    async fn anthropic_tool_use_verdict_passes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stop_reason": "tool_use",
                "content": [{
                    "type": "tool_use", "name": "submit_verdict",
                    "input": {"reasoning": "it declines", "pass": true, "score": 0.9}
                }]
            })))
            .mount(&server)
            .await;
        std::env::set_var("GRADER_TEST_KEY", "sk-test");
        let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
        let outcome = grader
            .grade(
                &rubric_assert(),
                &Output::Text("I cannot help with that".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await
            .unwrap()
            .verdict
            .to_outcome(None);
        assert!(outcome.passed);
        assert!((outcome.score - 0.9).abs() < 1e-9);
    }

    /// The judge's own bill. `pricing:` on a `grader.provider` used to be parsed
    /// and ignored, so the model doing the scoring was the one part of a run
    /// that cost nothing according to the run itself.
    #[tokio::test]
    async fn a_judge_call_is_priced_by_the_graders_own_pricing_block() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stop_reason": "tool_use",
                "model": "claude-x-20260101",
                "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000},
                "content": [{
                    "type": "tool_use", "name": "submit_verdict",
                    "input": {"reasoning": "ok", "pass": true, "score": 1.0}
                }]
            })))
            .mount(&server)
            .await;
        std::env::set_var("GRADER_TEST_KEY", "sk-test");

        // `claude-x` is not in the built-in table, so without the override this
        // is unpriced — which is the second half of what this pins.
        let mut cfg = anthropic_grader(&server.uri());
        let unpriced = DefaultGrader::new(Some(cfg.clone()))
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await
            .unwrap();
        assert!(unpriced.cost_usd.is_none(), "unknown model prices nothing");

        if let ProviderKind::Anthropic { pricing, .. } = &mut cfg.provider {
            *pricing = Some(Box::new(crate::config::PricingCfg {
                input_per_mtok: Some(3.0),
                output_per_mtok: Some(15.0),
                ..Default::default()
            }));
        }
        let graded = DefaultGrader::new(Some(cfg))
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await
            .unwrap();
        assert_eq!(graded.cost_usd, Some(18.0));
        // The model the judge reported, not the one configured.
        assert_eq!(graded.model.as_deref(), Some("claude-x-20260101"));
    }

    /// A cosine value belongs to the model that produced the vectors, so the
    /// verdict key must move when the embedder does. It did not: the key was the
    /// constant `{"assert": "similar"}`, so switching embedding models replayed
    /// the previous one's answers.
    #[test]
    fn a_similar_verdict_key_moves_with_the_embedding_model() {
        let assert = Assert {
            weight: 1.0,
            negate: false,
            kind: AssertKind::Similar {
                value: crate::val::Val::Tpl(json!("hello")),
                threshold: None,
            },
        };
        let with = |model: &str| {
            DefaultGrader::new(None)
                .with_embeddings(crate::embeddings::EmbeddingsProvider::new(
                    "e", model, None, None, None, None,
                ))
                .grading_fingerprint(&assert)
        };
        assert_ne!(
            with("text-embedding-3-small"),
            with("text-embedding-3-large")
        );
        // And with no embeddings provider there is nothing to key on, so the
        // assertion opts out of caching entirely rather than caching an error.
        assert!(DefaultGrader::new(None)
            .grading_fingerprint(&assert)
            .is_none());
    }

    #[tokio::test]
    async fn truncated_verdict_fails_closed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "stop_reason": "max_tokens",
                "content": []
            })))
            .mount(&server)
            .await;
        std::env::set_var("GRADER_TEST_KEY", "sk-test");
        let grader = DefaultGrader::new(Some(anthropic_grader(&server.uri())));
        let outcome = grader
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await;
        let err = outcome.unwrap_err();
        // Asserting on the variant, not a substring: a truncated verdict must
        // fail closed *and* be identifiable as that specific problem, since the
        // fix (raise the grader's max_tokens) is unique to it.
        assert!(
            matches!(err, GraderError::TruncatedVerdict { .. }),
            "truncated verdict must fail closed as its own kind: {err}"
        );
    }

    #[tokio::test]
    async fn no_grader_configured_fails_closed() {
        let grader = DefaultGrader::new(None);
        let outcome = grader
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await;
        let err = outcome.unwrap_err();
        assert!(
            matches!(err, GraderError::Unconfigured { kind: "llm-rubric" }),
            "an unconfigured grader is the suite author's problem, not a failure: {err}"
        );
        assert_eq!(err.class().as_str(), ErrorClass::GRADER_MISSING);
    }

    #[tokio::test]
    async fn thinking_params_are_rejected() {
        let server = MockServer::start().await;
        std::env::set_var("GRADER_TEST_KEY", "sk-test");
        let mut params = serde_json::Map::new();
        params.insert("thinking".into(), json!({"type": "enabled"}));
        let grader = Grader {
            provider: ProviderKind::Anthropic {
                model: "claude-x".into(),
                base_url: Some(server.uri()),
                api_key_env: Some("GRADER_TEST_KEY".into()),
                params: Some(params),
                pricing: None,
            },
            template: None,
            verdict_mode: None,
            timeout_ms: None,
        };
        let outcome = DefaultGrader::new(Some(grader))
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await;
        let err = outcome.unwrap_err();
        assert!(matches!(err, GraderError::Misconfigured(_)), "{err}");
        assert!(err.to_string().contains("thinking"), "{err}");
    }

    #[tokio::test]
    async fn exec_assert_grades() {
        let assert = Assert {
            weight: 1.0,
            negate: false,
            kind: AssertKind::Exec {
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    "cat >/dev/null; printf '{\"pass\":true,\"score\":1.0,\"reason\":\"ok\"}'"
                        .into(),
                ],
                config: None,
                cache_salt: None,
            },
        };
        let outcome = DefaultGrader::new(None)
            .grade(
                &assert,
                &Output::Text("x".into()),
                &grade_ctx(&json!({}), &TemplateEngine::new()),
            )
            .await
            .unwrap()
            .verdict
            .to_outcome(None);
        assert!(outcome.passed);
    }
}

#[cfg(test)]
mod inert_field_tests {
    //! The three fields that were parsed, schema'd, documented, and never read.

    use super::*;
    use crate::config::ParamMap;

    fn params(pairs: &[(&str, serde_json::Value)]) -> ParamMap {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    /// `LlmRubric.params` is the more specific of the two, so it wins — a suite
    /// sets it precisely to deviate from the shared grader default.
    #[test]
    fn assert_params_override_the_grader_provider_params() {
        let merged = merge_params(
            Some(&params(&[
                ("temperature", serde_json::json!(0)),
                ("max_tokens", serde_json::json!(1024)),
            ])),
            Some(&params(&[("max_tokens", serde_json::json!(8192))])),
        )
        .expect("merged");
        assert_eq!(merged.get("max_tokens"), Some(&serde_json::json!(8192)));
        // Keys the assertion did not mention are inherited, not dropped.
        assert_eq!(merged.get("temperature"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn merging_is_identity_when_only_one_side_is_set() {
        let only_provider = params(&[("a", serde_json::json!(1))]);
        assert_eq!(
            merge_params(Some(&only_provider), None).as_ref(),
            Some(&only_provider)
        );
        assert_eq!(
            merge_params(None, Some(&only_provider)).as_ref(),
            Some(&only_provider)
        );
        assert!(merge_params(None, None).is_none());
    }

    #[test]
    fn a_grader_template_substitutes_both_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.txt");
        std::fs::write(&path, "Judge this.\n<r>{{rubric}}</r>\n<o>{{output}}</o>").unwrap();
        let rendered = render_grader_template(
            &format!("file://{}", path.display()),
            "be concise",
            "a model answer",
        )
        .unwrap();
        assert!(rendered.contains("<r>be concise</r>"));
        assert!(rendered.contains("<o>a model answer</o>"));
    }

    /// The output is untrusted model text. Substituting literally rather than
    /// rendering keeps a grading prompt from becoming a template-injection
    /// surface — a model that emits `{{ ... }}` must not have it evaluated.
    #[test]
    fn a_grader_template_does_not_evaluate_the_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.txt");
        std::fs::write(&path, "{{output}}").unwrap();
        let rendered =
            render_grader_template(&format!("file://{}", path.display()), "r", "{{ 7 * 7 }}")
                .unwrap();
        assert_eq!(rendered, "{{ 7 * 7 }}", "must not evaluate to 49");
    }

    #[test]
    fn a_non_file_template_is_a_config_error() {
        let err = render_grader_template("./relative.txt", "r", "o").unwrap_err();
        assert!(err.to_string().contains("file://"));
    }
}
