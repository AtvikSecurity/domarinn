//! Native Anthropic Messages API provider.
//!
//! A thin hand-rolled client so parameters pass through verbatim: no forced
//! `temperature`, no hidden overrides. `max_tokens` is required by the API, so
//! it defaults only when absent.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::config::ParamMap;
use crate::empty::EmptyReason;
use crate::error_class::ErrorClass;
use crate::net::{api_key, http_client, parse_retry_after, status_error, transport_error};
use crate::provider::{
    http_request_preview, CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse,
};
use crate::types::{ChatRole, Output, RenderedPrompt, TokenUsage};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 4096;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct AnthropicProvider {
    id: String,
    model: String,
    base_url: String,
    api_key_env: crate::config::EnvNames,
    params: ParamMap,
    client: reqwest::Client,
    /// The effective rate for `model`, resolved once at construction. `None`
    /// means this provider's calls cannot be priced, so `cost_usd` stays absent.
    rate: Option<crate::pricing::ModelRate>,
}

impl AnthropicProvider {
    pub fn new(
        id: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key_env: Option<crate::config::EnvNames>,
        params: Option<ParamMap>,
        pricing: Option<crate::config::PricingCfg>,
    ) -> Self {
        let model = model.into();
        let id = id.into();
        // Resolved here, not per call: `build_provider` runs once per provider
        // per run, so the unknown-model warning fires exactly once per run per
        // id with no global state — and `validate`/`list`, which never build a
        // provider, stay silent.
        let rate = crate::pricing::resolve_rate(&id, &model, pricing.as_ref());
        AnthropicProvider {
            id,
            model,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "ANTHROPIC_API_KEY".into()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
            rate,
        }
    }

    fn build_body(&self, prompt: &RenderedPrompt, tools: &[crate::config::ToolDef]) -> Json {
        let (system, messages) = to_messages(prompt);
        let mut body = serde_json::Map::new();
        // Caller params first, so model/messages below are authoritative but
        // temperature/top_p/etc. pass through untouched.
        for (k, v) in &self.params {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), json!(self.model));
        body.insert("messages".into(), json!(messages));
        if let Some(system) = system {
            body.insert("system".into(), json!(system));
        }
        body.entry("max_tokens")
            .or_insert_with(|| json!(DEFAULT_MAX_TOKENS));
        // The suite's declarations, in this vendor's own shape — `ToolDef`
        // borrows its field names, so the mapping is a rename of nothing. Only
        // written when the suite declared tools, so a body with none is
        // byte-identical to what it was before tools existed, and so is every
        // cache entry keyed on it.
        if !tools.is_empty() {
            body.insert(
                "tools".into(),
                Json::Array(
                    tools
                        .iter()
                        .map(|t| {
                            json!({
                                "name": t.name,
                                "description": t.description.clone().unwrap_or_default(),
                                "input_schema": t.input_schema.clone()
                                    .unwrap_or_else(|| json!({"type": "object"})),
                            })
                        })
                        .collect(),
                ),
            );
        }
        Json::Object(body)
    }

    /// The Messages endpoint, trimmed the same way `call` trims it.
    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn fingerprint(&self) -> Json {
        json!({
            "type": "anthropic",
            "model": self.model,
            "base_url": self.base_url,
            "params": self.params,
        })
    }

    async fn call(
        &self,
        req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let prompt = req.prompt.as_ref().ok_or_else(|| {
            ProviderError::fatal(
                ErrorClass::PROVIDER_REQUEST,
                anyhow::anyhow!("anthropic provider requires a prompt"),
            )
        })?;
        let key = api_key(&self.api_key_env)?;
        let body = self.build_body(prompt, &req.tools);
        let url = self.endpoint();

        let response = self
            .client
            .post(&url)
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if !status.is_success() {
            let retry = parse_retry_after(response.headers());
            let text = response.text().await.unwrap_or_default();
            return Err(status_error(status, retry, text));
        }

        let payload: Json = response.json().await.map_err(transport_error)?;
        parse_messages_response(&payload, self.rate.as_ref())
    }

    fn request_preview(&self, req: &ProviderRequest) -> Option<Json> {
        let prompt = req.prompt.as_ref()?;
        Some(http_request_preview(
            "POST",
            &self.endpoint(),
            self.build_body(prompt, &req.tools),
        ))
    }
}

/// Convert a rendered prompt into (system, messages) for the Messages API.
fn to_messages(prompt: &RenderedPrompt) -> (Option<String>, Vec<Json>) {
    match prompt {
        RenderedPrompt::Text(text) => (None, vec![json!({"role": "user", "content": text})]),
        RenderedPrompt::Messages(msgs) => {
            let mut system: Vec<String> = Vec::new();
            let mut out = Vec::new();
            for m in msgs {
                if m.role == ChatRole::System {
                    system.push(m.content.clone());
                } else {
                    out.push(json!({"role": m.role, "content": m.content}));
                }
            }
            let system = (!system.is_empty()).then(|| system.join("\n\n"));
            (system, out)
        }
    }
}

/// Join every block of `kind`, reading its same-named payload field.
fn join_blocks(blocks: &[Json], kind: &str) -> String {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some(kind))
        .filter_map(|b| b.get(kind).and_then(|t| t.as_str()))
        .collect::<Vec<_>>()
        .join("")
}

/// The `tool_use` blocks in a Messages response, in the order the model emitted
/// them.
///
/// `input` is already a decoded object here — Anthropic sends it as JSON, not as
/// a string, which is the half of the vendor split `openai.rs` has to undo.
fn tool_calls_from_blocks(blocks: &[Json]) -> Vec<domarinn_types::result::ToolCall> {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
        .filter_map(|b| {
            Some(domarinn_types::result::ToolCall {
                id: b.get("id").and_then(|v| v.as_str()).map(str::to_string),
                // A block with no name is not a call we can attribute, and
                // inventing a name would make a `tool-call` assertion match
                // something that never happened.
                name: b.get("name").and_then(|v| v.as_str())?.to_string(),
                arguments: b.get("input").cloned().unwrap_or(Json::Null),
            })
        })
        .collect()
}

fn has_block(blocks: &[Json], kind: &str) -> bool {
    blocks
        .iter()
        .any(|b| b.get("type").and_then(|t| t.as_str()) == Some(kind))
}

/// The billable tokens in a Messages API response.
///
/// Shared with the llm-rubric grader, which calls the same endpoint: a second
/// hand-rolled copy would be one refactor away from disagreeing with this one
/// about which counters `input_tokens` already excludes.
///
/// Here it excludes *both* cache counters, so the three fields sum cleanly.
/// See [`crate::openai::usage_from_payload`] for the vendor that does not.
pub(crate) fn usage_from_payload(payload: &Json) -> Option<TokenUsage> {
    payload.get("usage").map(|u| TokenUsage {
        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_read_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()),
        cache_write_tokens: u
            .get("cache_creation_input_tokens")
            .and_then(|v| v.as_u64()),
        // The per-TTL split, when the API reports it. Absent is not zero-ish
        // guesswork: the default TTL is the short one, so "no split reported"
        // and "all at the short TTL" are the same statement.
        cache_write_1h_tokens: u
            .get("cache_creation")
            .and_then(|c| c.get("ephemeral_1h_input_tokens"))
            .and_then(|v| v.as_u64()),
    })
}

fn parse_messages_response(
    payload: &Json,
    rate: Option<&crate::pricing::ModelRate>,
) -> Result<ProviderResponse, ProviderError> {
    let blocks = payload.get("content").and_then(|c| c.as_array());

    let text = blocks.map(|b| join_blocks(b, "text")).unwrap_or_default();

    // `content` is a heterogeneous block array, not a string: a response made
    // entirely of `thinking` blocks joins to "" here and returns Ok. Capturing
    // the thinking separately is what turns that from an unexplained zero into
    // a diagnosis.
    let reasoning = blocks
        .map(|b| join_blocks(b, "thinking"))
        .filter(|s| !s.trim().is_empty());
    // `redacted_thinking` carries no readable text — only its presence is
    // information.
    let redacted = blocks
        .map(|b| has_block(b, "redacted_thinking"))
        .unwrap_or(false);

    let stop_reason = payload
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .map(String::from);

    let empty_reason = if text.trim().is_empty() {
        let mut candidates = Vec::new();
        // Shared with every other provider, including exec children: one
        // vocabulary rather than a hand-rolled match per call site, so a
        // reason added for one provider is understood by all of them.
        candidates.extend(
            stop_reason
                .as_deref()
                .and_then(crate::empty::from_stop_reason),
        );
        match blocks {
            None => candidates.push(EmptyReason::new(EmptyReason::NO_CONTENT_BLOCKS)),
            Some(b) if b.is_empty() => {
                candidates.push(EmptyReason::new(EmptyReason::NO_CONTENT_BLOCKS))
            }
            Some(b) => {
                if has_block(b, "tool_use") {
                    candidates.push(EmptyReason::new(EmptyReason::TOOL_USE_ONLY));
                }
                if reasoning.is_some() || redacted {
                    candidates.push(EmptyReason::new(EmptyReason::THINKING_ONLY));
                }
            }
        }
        candidates.push(EmptyReason::new(EmptyReason::BLANK));
        crate::empty::most_specific(&candidates)
    } else {
        None
    };

    let usage = usage_from_payload(payload);
    // Costed here, in the parse path, rather than in the runner. A cache hit
    // replays `cost_usd` from its entry, so costing downstream would re-price a
    // replayed hit against today's rate table — and a run's cost would then
    // depend on when you read it. Computed once, with the rates in effect at
    // the moment of the call, and replayed verbatim forever.
    let cost_usd = rate
        .and_then(|r| usage.as_ref().and_then(|u| crate::pricing::cost_of(u, r)))
        .map(|c| c.to_usd());

    Ok(ProviderResponse {
        tool_calls: blocks
            .map(|b| tool_calls_from_blocks(b))
            .unwrap_or_default(),
        output: Output::Text(text),
        usage,
        cost_usd,
        stop_reason,
        raw: Some(payload.clone()),
        reasoning,
        empty_reason,
        // The model the API says it served, not the one configured. An alias
        // like a floating snapshot pointer silently repointing is exactly the
        // drift this exists to make visible.
        model: payload
            .get("model")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use std::collections::BTreeMap;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The fingerprint is a member of every cache key (`cache_key.rs:42`), so
    /// changing it invalidates **every** entry in every disk, S3, and server
    /// store at once — the failure `cache_key.rs:10-12` warns about, one level
    /// up. A test that pins only `provider_cache_key` cannot catch this,
    /// because it holds the fingerprint fixed.
    ///
    /// If this fails, you have a cache migration to plan. New members belong
    /// here **conditionally**, only when configured, mirroring the `case_salt`
    /// discipline at `cache_key.rs:48-55`.
    /// The regression this whole subsystem exists for: `cost_usd` was
    /// hardcoded `None`, so `AssertKind::Cost` took its "not reported" branch
    /// and every budget assertion passed no matter what the call cost.
    #[test]
    fn a_priced_model_reports_a_cost() {
        let p = AnthropicProvider::new("p", "claude-haiku-4-5", None, None, None, None);
        let resp = parse_messages_response(
            &json!({
                "content": [{"type": "text", "text": "hi"}],
                "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000}
            }),
            p.rate.as_ref(),
        )
        .unwrap();
        // 1M in at $1 + 1M out at $5.
        assert_eq!(resp.cost_usd, Some(6.0));
    }

    /// An unknown model must report nothing rather than a guess, so the
    /// assertion keeps honestly saying "not reported".
    #[test]
    fn an_unpriced_model_reports_no_cost() {
        let p = AnthropicProvider::new("p", "claude-from-2030", None, None, None, None);
        assert!(p.rate.is_none());
        let resp = parse_messages_response(
            &json!({
                "content": [{"type": "text", "text": "hi"}],
                "usage": {"input_tokens": 10, "output_tokens": 10}
            }),
            p.rate.as_ref(),
        )
        .unwrap();
        assert_eq!(resp.cost_usd, None);
    }

    /// A `pricing:` block prices a model the table has never heard of.
    #[test]
    fn a_pricing_override_prices_an_unknown_model() {
        let cfg = crate::config::PricingCfg {
            input_per_mtok: Some(2.0),
            output_per_mtok: Some(4.0),
            cache_read_per_mtok: None,
            cache_write_per_mtok: None,
            cache_write_1h_per_mtok: None,
        };
        let p = AnthropicProvider::new("p", "private-model", None, None, None, Some(cfg));
        let resp = parse_messages_response(
            &json!({
                "content": [{"type": "text", "text": "hi"}],
                "usage": {"input_tokens": 1_000_000, "output_tokens": 1_000_000}
            }),
            p.rate.as_ref(),
        )
        .unwrap();
        assert_eq!(resp.cost_usd, Some(6.0));
    }

    /// The load-bearing guarantee for the override: cost is not request
    /// identity, so configuring a rate must not invalidate a single cache entry.
    #[test]
    fn pricing_does_not_reach_the_fingerprint() {
        let plain = AnthropicProvider::new("p", "claude-x", None, None, None, None);
        let priced = AnthropicProvider::new(
            "p",
            "claude-x",
            None,
            None,
            None,
            Some(crate::config::PricingCfg {
                input_per_mtok: Some(99.0),
                output_per_mtok: Some(99.0),
                cache_read_per_mtok: None,
                cache_write_per_mtok: None,
                cache_write_1h_per_mtok: None,
            }),
        );
        assert_eq!(
            crate::cache::canonical_json(&plain.fingerprint()),
            crate::cache::canonical_json(&priced.fingerprint())
        );
    }

    #[test]
    fn fingerprint_is_stable_for_default_config() {
        let p = AnthropicProvider::new("p", "claude-x", None, None, None, None);
        assert_eq!(
            crate::cache::canonical_json(&p.fingerprint()),
            r#"{"base_url":"https://api.anthropic.com","model":"claude-x","params":{},"type":"anthropic"}"#
        );
    }

    fn text_request() -> ProviderRequest {
        ProviderRequest {
            tools: Vec::new(),
            prompt: Some(RenderedPrompt::Text("hi".into())),
            vars: BTreeMap::new(),
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        }
    }

    #[test]
    fn body_defaults_max_tokens_but_keeps_params() {
        let mut params = serde_json::Map::new();
        params.insert("temperature".into(), json!(0.5));
        let p = AnthropicProvider::new("c", "claude-x", None, None, Some(params), None);
        let body = p.build_body(&RenderedPrompt::Text("hi".into()), &[]);
        assert_eq!(body["max_tokens"], json!(DEFAULT_MAX_TOKENS));
        assert_eq!(body["temperature"], json!(0.5));
        assert_eq!(body["model"], json!("claude-x"));
    }

    #[test]
    fn system_messages_are_extracted() {
        let prompt = RenderedPrompt::Messages(vec![
            crate::types::ChatMessage {
                role: ChatRole::System,
                content: "be nice".into(),
            },
            crate::types::ChatMessage {
                role: ChatRole::User,
                content: "hi".into(),
            },
        ]);
        let (system, messages) = to_messages(&prompt);
        assert_eq!(system.as_deref(), Some("be nice"));
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn request_preview_reflects_the_anthropic_body_shape() {
        // The load-bearing case for capturing this server-side: Anthropic lifts
        // `system` out of the message list into a top-level field, so a preview
        // reconstructed from the stored `RenderedPrompt` in the browser would
        // show a system *message* that was never sent as one.
        let p = AnthropicProvider::new("c", "claude-x", None, None, None, None);
        let req = ProviderRequest {
            tools: Vec::new(),
            prompt: Some(RenderedPrompt::Messages(vec![
                crate::types::ChatMessage {
                    role: ChatRole::System,
                    content: "be nice".into(),
                },
                crate::types::ChatMessage {
                    role: ChatRole::User,
                    content: "hi".into(),
                },
            ])),
            ..Default::default()
        };

        let preview = p.request_preview(&req).unwrap();
        assert_eq!(preview["transport"], json!("http"));
        assert_eq!(
            preview["url"],
            json!("https://api.anthropic.com/v1/messages")
        );

        let body = &preview["body"];
        assert_eq!(body["system"], json!("be nice"));
        assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
        // The API-required default is visible, not implied.
        assert_eq!(body["max_tokens"], json!(DEFAULT_MAX_TOKENS));
        assert!(preview.get("headers").is_none());
    }

    #[test]
    fn request_preview_matches_the_body_actually_built() {
        let p = AnthropicProvider::new("c", "claude-x", None, None, None, None);
        let req = text_request();
        let preview = p.request_preview(&req).unwrap();
        assert_eq!(
            preview["body"],
            p.build_body(req.prompt.as_ref().unwrap(), &req.tools)
        );
    }

    #[tokio::test]
    async fn calls_the_api_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("anthropic-version", ANTHROPIC_VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "hello"}],
                "usage": {"input_tokens": 3, "output_tokens": 1},
                "stop_reason": "end_turn"
            })))
            .mount(&server)
            .await;
        std::env::set_var("ANTHROPIC_TEST_KEY", "sk-test");
        let p = AnthropicProvider::new(
            "c",
            "claude-x",
            Some(server.uri()),
            Some("ANTHROPIC_TEST_KEY".into()),
            None,
            None,
        );
        let resp = p.call(&text_request(), &CallCtx::default()).await.unwrap();
        assert_eq!(resp.output, Output::Text("hello".into()));
        assert_eq!(resp.usage.unwrap().total(), 4);
    }

    #[tokio::test]
    async fn server_error_is_retriable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        std::env::set_var("ANTHROPIC_TEST_KEY2", "sk-test");
        let p = AnthropicProvider::new(
            "c",
            "claude-x",
            Some(server.uri()),
            Some("ANTHROPIC_TEST_KEY2".into()),
            None,
            None,
        );
        let err = p
            .call(&text_request(), &CallCtx::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Retriable { .. }));
    }

    #[tokio::test]
    async fn missing_prompt_is_fatal() {
        std::env::set_var("ANTHROPIC_TEST_KEY3", "sk-test");
        let p = AnthropicProvider::new(
            "c",
            "m",
            None,
            Some("ANTHROPIC_TEST_KEY3".into()),
            None,
            None,
        );
        let req = ProviderRequest {
            tools: Vec::new(),
            prompt: None,
            vars: BTreeMap::new(),
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        };
        assert!(matches!(
            p.call(&req, &CallCtx::default()).await,
            Err(ProviderError::Fatal { .. })
        ));
    }
}

#[cfg(test)]
mod empty_classification_tests {
    use super::*;

    fn reason_of(payload: Json) -> Option<String> {
        parse_messages_response(&payload, None)
            .unwrap()
            .empty_reason
            .map(|r| r.as_str().to_string())
    }

    /// The bug that started this: `content` is a block array, so a response made
    /// only of `thinking` joins to "" and returns Ok. It used to score 0 with
    /// nothing anywhere explaining why.
    #[test]
    fn a_thinking_only_response_is_diagnosed_and_its_reasoning_captured() {
        let payload = json!({
            "content": [{"type": "thinking", "thinking": "let me work through this"}],
            "stop_reason": "end_turn"
        });
        let resp = parse_messages_response(&payload, None).unwrap();

        assert_eq!(resp.output, Output::Text(String::new()));
        assert_eq!(resp.reasoning.as_deref(), Some("let me work through this"));
        assert_eq!(
            resp.empty_reason.unwrap().as_str(),
            EmptyReason::THINKING_ONLY
        );
    }

    /// Truncation outranks thinking-only: they point at opposite fixes ("raise
    /// max_tokens" vs "capture the reasoning"), and truncation explains *why*
    /// only thinking came back.
    #[test]
    fn truncation_outranks_thinking_only() {
        let payload = json!({
            "content": [{"type": "thinking", "thinking": "half a thought"}],
            "stop_reason": "max_tokens"
        });
        assert_eq!(reason_of(payload).as_deref(), Some(EmptyReason::TRUNCATED));
    }

    #[test]
    fn a_refusal_is_named_as_such() {
        let payload = json!({"content": [], "stop_reason": "refusal"});
        assert_eq!(reason_of(payload).as_deref(), Some(EmptyReason::REFUSAL));
    }

    #[test]
    fn a_tool_only_response_is_not_a_blank_answer() {
        let payload = json!({
            "content": [{"type": "tool_use", "name": "search", "input": {}}],
            "stop_reason": "tool_use"
        });
        assert_eq!(
            reason_of(payload).as_deref(),
            Some(EmptyReason::TOOL_USE_ONLY)
        );
    }

    #[test]
    fn redacted_thinking_counts_even_though_it_carries_no_text() {
        let payload = json!({
            "content": [{"type": "redacted_thinking", "data": "opaque"}],
            "stop_reason": "end_turn"
        });
        let resp = parse_messages_response(&payload, None).unwrap();
        assert!(resp.reasoning.is_none(), "there is no readable text");
        assert_eq!(
            resp.empty_reason.unwrap().as_str(),
            EmptyReason::THINKING_ONLY
        );
    }

    #[test]
    fn a_missing_content_array_is_a_protocol_fault_not_a_blank_answer() {
        assert_eq!(
            reason_of(json!({"stop_reason": "end_turn"})).as_deref(),
            Some(EmptyReason::NO_CONTENT_BLOCKS)
        );
    }

    #[test]
    fn a_normal_answer_has_no_empty_reason() {
        let payload = json!({
            "content": [{"type": "text", "text": "the answer is 42"}],
            "stop_reason": "end_turn"
        });
        let resp = parse_messages_response(&payload, None).unwrap();
        assert_eq!(resp.output, Output::Text("the answer is 42".into()));
        assert!(resp.empty_reason.is_none());
    }

    /// Text and thinking together: the answer is graded, the reasoning is kept
    /// alongside it rather than replacing it.
    #[test]
    fn reasoning_is_captured_alongside_a_real_answer() {
        let payload = json!({
            "content": [
                {"type": "thinking", "thinking": "6 times 7"},
                {"type": "text", "text": "42"}
            ],
            "stop_reason": "end_turn"
        });
        let resp = parse_messages_response(&payload, None).unwrap();
        assert_eq!(resp.output, Output::Text("42".into()));
        assert_eq!(resp.reasoning.as_deref(), Some("6 times 7"));
        assert!(resp.empty_reason.is_none());
    }
}

#[cfg(test)]
mod tool_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_use_blocks_become_tool_calls_in_order() {
        let calls = tool_calls_from_blocks(&[
            json!({"type": "text", "text": "let me check"}),
            json!({"type": "tool_use", "id": "a", "name": "first", "input": {"x": 1}}),
            json!({"type": "tool_use", "id": "b", "name": "second", "input": {}}),
        ]);
        assert_eq!(
            calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        // Already an object on this vendor — no decode step, unlike `openai.rs`.
        assert_eq!(calls[0].arguments["x"], 1);
    }

    /// A block with no name is not a call we can attribute, and inventing one
    /// would make a `tool-call` assertion match something that never happened.
    #[test]
    fn a_nameless_block_is_dropped_rather_than_given_a_name() {
        assert!(tool_calls_from_blocks(&[json!({"type": "tool_use", "input": {}})]).is_empty());
    }

    #[test]
    fn a_tool_free_body_is_unchanged_and_a_declared_tool_keeps_its_field_names() {
        let p = AnthropicProvider::new("p", "claude-haiku-4-5", None, None, None, None);
        let prompt = RenderedPrompt::Text("hi".into());
        assert!(p.build_body(&prompt, &[]).get("tools").is_none());

        let body = p.build_body(
            &prompt,
            &[crate::config::ToolDef {
                name: "get_weather".into(),
                description: None,
                input_schema: Some(json!({"type": "object"})),
            }],
        );
        assert_eq!(body["tools"][0]["name"], "get_weather");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }
}
