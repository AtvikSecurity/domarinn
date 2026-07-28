//! The two built-in llm-rubric judges, and the verdict they return.
//!
//! Split out of `grader.rs` so each file stays under the per-file line ratchet
//! (`tests/file_length.rs`), following the seam `runner_asserts.rs` set. The
//! division is by concern rather than by size: `grader.rs` decides *which*
//! grading an assertion needs and what identity it caches under, and this file
//! is the HTTP mechanics of asking a model for a verdict. Included as a private
//! child module of `grader`, so both halves still see `DefaultGrader`'s fields.

use serde_json::{json, Value as Json};

use crate::errors::GraderError;
use crate::net::api_key;
use crate::types::TokenUsage;

use super::{DefaultGrader, DEFAULT_GRADER_MAX_TOKENS, SYSTEM_PROMPT};

impl DefaultGrader {
    pub(super) async fn anthropic_verdict(
        &self,
        model: &str,
        base_url: Option<&str>,
        api_key_env: Option<&crate::config::EnvNames>,
        params: Option<&crate::config::ParamMap>,
        rate: Option<&crate::pricing::ModelRate>,
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
        let usage = crate::anthropic::usage_from_payload(&payload);
        Ok(Verdict::from_json(input)?.priced(&payload, usage, rate))
    }

    pub(super) async fn openai_verdict(
        &self,
        model: &str,
        base_url: Option<&str>,
        api_key_env: Option<&crate::config::EnvNames>,
        params: Option<&crate::config::ParamMap>,
        rate: Option<&crate::pricing::ModelRate>,
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
        let usage = crate::openai::usage_from_payload(&payload);
        Ok(Verdict::from_json(&value)?.priced(&payload, usage, rate))
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

/// A parsed grader verdict, plus what the judge call cost.
pub(super) struct Verdict {
    pub(super) reasoning: String,
    pub(super) pass: bool,
    pub(super) score: f64,
    pub(super) usage: Option<TokenUsage>,
    pub(super) cost_usd: Option<f64>,
    pub(super) model: Option<String>,
}

impl Verdict {
    /// Attach the billing side of the response the verdict was parsed out of.
    ///
    /// Costed here, at parse time, for the same reason a provider response is:
    /// a cached verdict replays this number, so pricing it downstream would
    /// re-price it against whatever the table said later.
    fn priced(
        mut self,
        payload: &Json,
        usage: Option<TokenUsage>,
        rate: Option<&crate::pricing::ModelRate>,
    ) -> Verdict {
        self.cost_usd = rate
            .and_then(|r| usage.as_ref().and_then(|u| crate::pricing::cost_of(u, r)))
            .map(|c| c.to_usd());
        self.usage = usage;
        self.model = payload
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_string);
        self
    }

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
            usage: None,
            cost_usd: None,
            model: None,
        })
    }
}
