//! The default grader for the non-local assert kinds (`exec`, `llm-rubric`).
//!
//! The llm-rubric path never parses a verdict out of prose: anthropic uses a
//! forced `submit_verdict` tool call, openai-compatible endpoints use a strict
//! `json_schema` response. Everything fails closed — a missing, unparseable, or
//! truncated verdict is a failure, never a silent pass.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::cache::GradedVerdict;
use crate::config::{Assert, AssertKind, Grader, ProviderKind};
use crate::errors::GraderError;
use crate::exec::run_exec_json;
use crate::exec_protocol::{AssertReq, AssertResp, Envelope, Kind, ProviderRef, TestRef};
use crate::net::{api_key, http_client};
use crate::runner::{AssertGrader, GradeCtx};
use crate::template::TemplateEngine;
use crate::types::Output;

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
            client: http_client(timeout),
        }
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
    ) -> Result<GradedVerdict, GraderError> {
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
            self.embeddings.is_some(),
            assert,
        )
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
    has_embeddings: bool,
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
        AssertKind::Similar { .. } => has_embeddings.then(|| json!({"assert": "similar"})),
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
    ) -> Result<GradedVerdict, GraderError> {
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
            GRADER_TIMEOUT,
            &request,
        )
        .await
        .map_err(|e| GraderError::Transport(format!("exec assert failed: {e}")))?;
        let resp: AssertResp = serde_json::from_value(value)
            .map_err(|e| GraderError::InvalidVerdict(format!("bad assert response: {e}")))?;
        let score = resp.score.unwrap_or(if resp.pass { 1.0 } else { 0.0 });
        Ok(GradedVerdict::Exec {
            pass: resp.pass,
            score,
            reason: resp.reason.unwrap_or_default(),
            details: resp.details,
        })
    }

    async fn grade_similar(
        &self,
        reference: &crate::val::Val,
        threshold: Option<f64>,
        output: &Output,
        vars: &Json,
        engine: &TemplateEngine,
    ) -> Result<GradedVerdict, GraderError> {
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
        Ok(GradedVerdict::Similarity {
            cosine: crate::embeddings::cosine(&a, &b),
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
    ) -> Result<GradedVerdict, GraderError> {
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

        let verdict = match &grader.provider {
            ProviderKind::Anthropic {
                model,
                base_url,
                api_key_env,
                params,
                // A grader's cost is not a case's cost, and no assertion grades
                // it, so a rate here would price nothing.
                pricing: _,
            } => {
                self.anthropic_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_ref(),
                    merge_params(params.as_ref(), assert_params).as_ref(),
                    &user,
                )
                .await
            }
            ProviderKind::Openai {
                model,
                base_url,
                api_key_env,
                params,
                pricing: _,
            } => {
                self.openai_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_ref(),
                    merge_params(params.as_ref(), assert_params).as_ref(),
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
        Ok(GradedVerdict::Rubric {
            score: v.score,
            pass: v.pass,
            reasoning: v.reasoning,
        })
    }

    async fn anthropic_verdict(
        &self,
        model: &str,
        base_url: Option<&str>,
        api_key_env: Option<&crate::config::EnvNames>,
        params: Option<&crate::config::ParamMap>,
        user: &str,
    ) -> Result<Verdict, GraderError> {
        reject_thinking(params)?;
        let key = api_key(
            api_key_env.unwrap_or(&crate::config::EnvNames::One("ANTHROPIC_API_KEY".into())),
        )
        .map_err(|e| GraderError::Transport(e.to_string()))?;
        let base = base_url
            .unwrap_or("https://api.anthropic.com")
            .trim_end_matches('/');

        let mut body = serde_json::Map::new();
        if let Some(p) = params {
            for (k, v) in p {
                body.insert(k.clone(), v.clone());
            }
        }
        body.insert("model".into(), json!(model));
        body.insert("system".into(), json!(SYSTEM_PROMPT));
        body.insert(
            "messages".into(),
            json!([{"role": "user", "content": user}]),
        );
        body.entry("max_tokens")
            .or_insert_with(|| json!(DEFAULT_GRADER_MAX_TOKENS));
        body.insert("tools".into(), json!([verdict_tool()]));
        body.insert(
            "tool_choice".into(),
            json!({"type": "tool", "name": "submit_verdict"}),
        );

        let resp = self
            .client
            .post(format!("{base}/v1/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(&Json::Object(body))
            .send()
            .await
            .map_err(|e| GraderError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            // 401/403 gets its own variant: a rejected credential will reject
            // every remaining call too, so the runner short-circuits rather
            // than erroring the whole suite one case at a time.
            if code == 401 || code == 403 {
                return Err(GraderError::AuthRejected { status: code });
            }
            return Err(GraderError::Transport(format!(
                "HTTP {code}: {}",
                resp.text().await.unwrap_or_default()
            )));
        }
        let payload: Json = resp
            .json()
            .await
            .map_err(|e| GraderError::Transport(e.to_string()))?;
        if payload.get("stop_reason").and_then(|s| s.as_str()) == Some("max_tokens") {
            return Err(GraderError::TruncatedVerdict {
                signal: "stop_reason=max_tokens",
            });
        }
        let input = payload
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
            })
            .and_then(|b| b.get("input"))
            .ok_or_else(|| GraderError::InvalidVerdict("no tool_use verdict in response".into()))?;
        Verdict::from_json(input)
    }

    async fn openai_verdict(
        &self,
        model: &str,
        base_url: Option<&str>,
        api_key_env: Option<&crate::config::EnvNames>,
        params: Option<&crate::config::ParamMap>,
        user: &str,
    ) -> Result<Verdict, GraderError> {
        let key =
            api_key(api_key_env.unwrap_or(&crate::config::EnvNames::One("OPENAI_API_KEY".into())))
                .map_err(|e| GraderError::Transport(e.to_string()))?;
        let base = base_url
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');

        let mut body = serde_json::Map::new();
        if let Some(p) = params {
            for (k, v) in p {
                body.insert(k.clone(), v.clone());
            }
        }
        body.insert("model".into(), json!(model));
        body.insert(
            "messages".into(),
            json!([
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": user}
            ]),
        );
        body.insert(
            "response_format".into(),
            json!({
                "type": "json_schema",
                "json_schema": {"name": "verdict", "strict": true, "schema": verdict_schema()}
            }),
        );

        let resp = self
            .client
            .post(format!("{base}/chat/completions"))
            .bearer_auth(key)
            .json(&Json::Object(body))
            .send()
            .await
            .map_err(|e| GraderError::Transport(e.to_string()))?;
        if !resp.status().is_success() {
            let code = resp.status().as_u16();
            // 401/403 gets its own variant: a rejected credential will reject
            // every remaining call too, so the runner short-circuits rather
            // than erroring the whole suite one case at a time.
            if code == 401 || code == 403 {
                return Err(GraderError::AuthRejected { status: code });
            }
            return Err(GraderError::Transport(format!(
                "HTTP {code}: {}",
                resp.text().await.unwrap_or_default()
            )));
        }
        let payload: Json = resp
            .json()
            .await
            .map_err(|e| GraderError::Transport(e.to_string()))?;
        let choice = payload
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|c| c.first());
        if choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|s| s.as_str())
            == Some("length")
        {
            return Err(GraderError::TruncatedVerdict {
                signal: "finish_reason=length",
            });
        }
        let content = choice
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| GraderError::InvalidVerdict("no content in response".into()))?;
        let value: Json = serde_json::from_str(content)
            .map_err(|e| GraderError::InvalidVerdict(format!("verdict not JSON: {e}")))?;
        Verdict::from_json(&value)
    }
}

/// The reject-list for grader params incompatible with forced tool use.
fn reject_thinking(params: Option<&crate::config::ParamMap>) -> Result<(), GraderError> {
    if let Some(p) = params {
        if p.contains_key("thinking") || p.contains_key("reasoning") {
            return Err(GraderError::Misconfigured(
                "grader params must not enable extended thinking: forced tool use is rejected \
                 when thinking is on. Remove `thinking`/`reasoning`."
                    .into(),
            ));
        }
    }
    Ok(())
}

fn verdict_schema() -> Json {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "reasoning": {"type": "string"},
            "pass": {"type": "boolean"},
            "score": {"type": "number"}
        },
        "required": ["reasoning", "pass", "score"]
    })
}

fn verdict_tool() -> Json {
    json!({
        "name": "submit_verdict",
        "description": "Submit the grading verdict.",
        "input_schema": verdict_schema()
    })
}

fn output_to_json(output: &Output) -> Json {
    match output {
        Output::Text(s) => Json::String(s.clone()),
        Output::Json(v) => v.clone(),
    }
}

/// A parsed grader verdict.
struct Verdict {
    reasoning: String,
    pass: bool,
    score: f64,
}

impl Verdict {
    fn from_json(v: &Json) -> Result<Verdict, GraderError> {
        let pass = v
            .get("pass")
            .and_then(|p| p.as_bool())
            .ok_or_else(|| GraderError::InvalidVerdict("verdict missing `pass`".into()))?;
        let score = v
            .get("score")
            .and_then(|s| s.as_f64())
            .ok_or_else(|| GraderError::InvalidVerdict("verdict missing `score`".into()))?
            .clamp(0.0, 1.0);
        let reasoning = v
            .get("reasoning")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        Ok(Verdict {
            reasoning,
            pass,
            score,
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
            .to_outcome(None);
        assert!(outcome.passed);
        assert!((outcome.score - 0.9).abs() < 1e-9);
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
