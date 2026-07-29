//! The two built-in llm-rubric judges, and the verdict they return.
//!
//! Split out of `grader.rs` so each file stays under the per-file line ratchet
//! (`tests/file_length.rs`), following the seam `runner_asserts.rs` set. The
//! division is by concern rather than by size: `grader.rs` decides *which*
//! grading an assertion needs and caches it, and this file is the HTTP mechanics
//! of asking a model for a verdict. Included as a private child module of
//! `grader`, so both halves still see `DefaultGrader`'s fields.
//!
//! Each judge is three separable pieces — build the request, post it, parse the
//! payload — rather than one function that does all three. That is what lets the
//! request be *keyed* without being sent (a `--cache-only` run never posts, and
//! never reads the credential) and the payload be parsed identically whether it
//! came off the wire or out of the store.

use serde_json::{json, Value as Json};

use crate::errors::GraderError;
use crate::net::api_key;
use crate::types::TokenUsage;

use super::{DefaultGrader, DEFAULT_GRADER_MAX_TOKENS, SYSTEM_PROMPT};

/// Which of the two built-in judges is answering.
///
/// An enum rather than two parallel call sites, so `grade_llm_rubric` has one
/// code path: build, cache, parse. The vendor differences — where the system
/// prompt goes, how a structured verdict is forced, which header carries the
/// key, which envelope reports usage — are all here.
#[derive(Clone, Copy)]
pub(super) enum Judge {
    Anthropic,
    Openai,
}

impl Judge {
    /// The url and body this judge would post. Pure: no credential, no clock,
    /// no filesystem — the request is the cache key, so it must be a function of
    /// its inputs and nothing else.
    ///
    /// Everything the deleted `grading_fingerprint` enumerated by hand is in
    /// here: the model, the endpoint, the merged params, the system prompt, and
    /// (inside `user`) the rendered rubric, the graded output, and the contents
    /// of any `grader.template`. Editing the template file changes the body and
    /// therefore the key, with no separate digest to keep in step.
    pub(super) fn request(
        &self,
        model: &str,
        base_url: Option<&str>,
        params: Option<&crate::config::ParamMap>,
        user: &str,
    ) -> (String, Json) {
        let mut body = serde_json::Map::new();
        if let Some(p) = params {
            for (k, v) in p {
                body.insert(k.clone(), v.clone());
            }
        }
        body.insert("model".into(), json!(model));
        match self {
            Judge::Anthropic => {
                let base = base_url
                    .unwrap_or("https://api.anthropic.com")
                    .trim_end_matches('/');
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
                (format!("{base}/v1/messages"), Json::Object(body))
            }
            Judge::Openai => {
                let base = base_url
                    .unwrap_or("https://api.openai.com/v1")
                    .trim_end_matches('/');
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
                (format!("{base}/chat/completions"), Json::Object(body))
            }
        }
    }

    /// The env var this judge reads its credential from, absent an override.
    fn default_key_env(&self) -> crate::config::EnvNames {
        match self {
            Judge::Anthropic => crate::config::EnvNames::One("ANTHROPIC_API_KEY".into()),
            Judge::Openai => crate::config::EnvNames::One("OPENAI_API_KEY".into()),
        }
    }

    /// Turn a judge payload into a priced verdict.
    ///
    /// Runs on a cache hit as well as a live call, so a verdict is never
    /// replayed from a shape this code would reject today, and `rate` is
    /// today's rate — a warm suite reports current prices rather than the ones
    /// in force when the call was made.
    pub(super) fn parse(
        &self,
        payload: &Json,
        rate: Option<&crate::pricing::ModelRate>,
    ) -> Result<Verdict, GraderError> {
        let (input, usage) = match self {
            Judge::Anthropic => {
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
                    .cloned()
                    .ok_or_else(|| {
                        GraderError::InvalidVerdict("no tool_use verdict in response".into())
                    })?;
                (input, crate::anthropic::usage_from_payload(payload))
            }
            Judge::Openai => {
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
                let input: Json = serde_json::from_str(content)
                    .map_err(|e| GraderError::InvalidVerdict(format!("verdict not JSON: {e}")))?;
                (input, crate::openai::usage_from_payload(payload))
            }
        };
        Ok(Verdict::from_json(&input)?.priced(payload, usage, rate))
    }
}

impl DefaultGrader {
    /// Post a built judge request and return the raw payload.
    ///
    /// The only step that touches a credential, which is what keeps a
    /// `--cache-only` run from demanding one it will never read: the key is
    /// derived from [`Judge::request`], and this runs only on a miss.
    pub(super) async fn post_judge(
        &self,
        judge: Judge,
        url: &str,
        body: &Json,
        api_key_env: Option<&crate::config::EnvNames>,
    ) -> Result<Json, GraderError> {
        let default_env = judge.default_key_env();
        let key = api_key(api_key_env.unwrap_or(&default_env))
            .map_err(|e| GraderError::Transport(e.to_string()))?;
        let request = self.client.post(url).json(body);
        let request = match judge {
            Judge::Anthropic => request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01"),
            Judge::Openai => request.bearer_auth(key),
        };
        let resp = request
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
        resp.json()
            .await
            .map_err(|e| GraderError::Transport(e.to_string()))
    }
}

/// The reject-list for grader params incompatible with forced tool use.
pub(super) fn reject_thinking(params: Option<&crate::config::ParamMap>) -> Result<(), GraderError> {
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
    /// Costed here, at parse time, and therefore re-costed on every cache hit:
    /// the payload is what is stored, so the price a replay reports is the one
    /// in today's rate table rather than the one that was current when the call
    /// was made. The figure the entry recorded is the fallback for a model
    /// today's table cannot price.
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
