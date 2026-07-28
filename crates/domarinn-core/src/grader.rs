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

use crate::assertion::AssertOutcome;
use crate::config::{Assert, AssertKind, Grader, ProviderKind};
use crate::errors::GraderError;
use crate::exec::run_exec_json;
use crate::exec_protocol::{AssertReq, AssertResp, Envelope, Kind, ProviderRef, TestRef};
use crate::net::{api_key, http_client};
use crate::runner::AssertGrader;
use crate::template::TemplateEngine;
use crate::types::Output;

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
        DefaultGrader {
            default_grader,
            embeddings: None,
            client: http_client(GRADER_TIMEOUT),
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
        vars: &Json,
        engine: &TemplateEngine,
        working_dir: Option<&std::path::Path>,
    ) -> Result<AssertOutcome, GraderError> {
        let outcome = match &assert.kind {
            AssertKind::Exec { command, config } => {
                self.grade_exec(command, config.as_ref(), output, vars, working_dir)
                    .await
            }
            AssertKind::LlmRubric {
                value,
                grader,
                threshold,
                ..
            } => {
                self.grade_llm_rubric(value, grader.as_ref(), *threshold, output, vars, engine)
                    .await
            }
            AssertKind::Similar { value, threshold } => {
                self.grade_similar(value, *threshold, output, vars, engine)
                    .await
            }
            _ => Err(GraderError::Internal("local assert routed to grader")),
        };
        outcome.map(|o| o.negated(assert.negate))
    }
}

impl DefaultGrader {
    async fn grade_exec(
        &self,
        command: &[String],
        config: Option<&Json>,
        output: &Output,
        vars: &Json,
        working_dir: Option<&std::path::Path>,
    ) -> Result<AssertOutcome, GraderError> {
        let request = AssertReq {
            envelope: Envelope::new(Kind::Assert),
            output: output_to_json(output),
            test: TestRef {
                id: String::new(),
                tags: vec![],
            },
            prompt: None,
            provider: ProviderRef { id: String::new() },
            config: config.cloned().unwrap_or(Json::Null),
        };
        let _ = vars;
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
        Ok(AssertOutcome {
            score,
            passed: resp.pass,
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
    ) -> Result<AssertOutcome, GraderError> {
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
        let sim = crate::embeddings::cosine(&a, &b);
        let threshold = threshold.unwrap_or(0.8);
        let score = ((sim + 1.0) / 2.0).clamp(0.0, 1.0);
        Ok(AssertOutcome {
            score,
            passed: sim >= threshold,
            reason: if sim >= threshold {
                format!("cosine similarity {sim:.3} >= {threshold:.3}")
            } else {
                format!("cosine similarity {sim:.3} < {threshold:.3}")
            },
            details: None,
        })
    }

    async fn grade_llm_rubric(
        &self,
        rubric_template: &str,
        assert_grader: Option<&Grader>,
        threshold: Option<f64>,
        output: &Output,
        vars: &Json,
        engine: &TemplateEngine,
    ) -> Result<AssertOutcome, GraderError> {
        // The variant that motivated the whole type: nothing ran, and the fix is
        // to add a `grader:` block — not to retry.
        let grader = assert_grader
            .or(self.default_grader.as_ref())
            .ok_or(GraderError::Unconfigured { kind: "llm-rubric" })?;
        let rubric = engine
            .render_str(rubric_template, vars)
            .map_err(|e| GraderError::Misconfigured(format!("rendering rubric: {e}")))?;
        let output_text = output.as_text();
        let user = format!("RUBRIC:\n{rubric}\n\nASSISTANT OUTPUT:\n{output_text}");

        let verdict = match &grader.provider {
            ProviderKind::Anthropic {
                model,
                base_url,
                api_key_env,
                params,
            } => {
                self.anthropic_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_deref(),
                    params.as_ref(),
                    &user,
                )
                .await
            }
            ProviderKind::Openai {
                model,
                base_url,
                api_key_env,
                params,
            } => {
                self.openai_verdict(
                    model,
                    base_url.as_deref(),
                    api_key_env.as_deref(),
                    params.as_ref(),
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
        let passed = match threshold {
            Some(t) => v.score >= t,
            None => v.pass,
        };
        Ok(AssertOutcome {
            score: v.score,
            passed,
            reason: v.reasoning,
            details: None,
        })
    }

    async fn anthropic_verdict(
        &self,
        model: &str,
        base_url: Option<&str>,
        api_key_env: Option<&str>,
        params: Option<&crate::config::ParamMap>,
        user: &str,
    ) -> Result<Verdict, GraderError> {
        reject_thinking(params)?;
        let key = api_key(api_key_env.unwrap_or("ANTHROPIC_API_KEY"))
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
        api_key_env: Option<&str>,
        params: Option<&crate::config::ParamMap>,
        user: &str,
    ) -> Result<Verdict, GraderError> {
        let key = api_key(api_key_env.unwrap_or("OPENAI_API_KEY"))
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
            },
            template: None,
            verdict_mode: None,
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
                &json!({}),
                &TemplateEngine::new(),
                None,
            )
            .await
            .unwrap();
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
                &json!({}),
                &TemplateEngine::new(),
                None,
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
                &json!({}),
                &TemplateEngine::new(),
                None,
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
            },
            template: None,
            verdict_mode: None,
        };
        let outcome = DefaultGrader::new(Some(grader))
            .grade(
                &rubric_assert(),
                &Output::Text("x".into()),
                &json!({}),
                &TemplateEngine::new(),
                None,
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
            },
        };
        let outcome = DefaultGrader::new(None)
            .grade(
                &assert,
                &Output::Text("x".into()),
                &json!({}),
                &TemplateEngine::new(),
                None,
            )
            .await
            .unwrap();
        assert!(outcome.passed);
    }
}
