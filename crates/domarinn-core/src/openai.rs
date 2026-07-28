//! OpenAI-compatible chat-completions provider.
//!
//! Works against the OpenAI API and any compatible gateway (vLLM, LiteLLM,
//! Together, Ollama, ...). Parameters pass through verbatim.

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
use crate::types::{Output, RenderedPrompt, TokenUsage};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OpenAiProvider {
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

impl OpenAiProvider {
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
        OpenAiProvider {
            id,
            model,
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "OPENAI_API_KEY".into()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
            rate,
        }
    }

    fn build_body(&self, prompt: &RenderedPrompt) -> Json {
        let messages = to_messages(prompt);
        let mut body = serde_json::Map::new();
        for (k, v) in &self.params {
            body.insert(k.clone(), v.clone());
        }
        body.insert("model".into(), json!(self.model));
        body.insert("messages".into(), json!(messages));
        Json::Object(body)
    }

    /// The chat-completions endpoint, trimmed the same way `call` trims it.
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for OpenAiProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn fingerprint(&self) -> Json {
        json!({
            "type": "openai",
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
                anyhow::anyhow!("openai provider requires a prompt"),
            )
        })?;
        let key = api_key(&self.api_key_env)?;
        let body = self.build_body(prompt);
        let url = self.endpoint();

        let response = self
            .client
            .post(&url)
            .bearer_auth(key)
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
        parse_completion_response(&payload, self.rate.as_ref())
    }

    fn request_preview(&self, req: &ProviderRequest) -> Option<Json> {
        let prompt = req.prompt.as_ref()?;
        Some(http_request_preview(
            "POST",
            &self.endpoint(),
            self.build_body(prompt),
        ))
    }
}

fn to_messages(prompt: &RenderedPrompt) -> Vec<Json> {
    match prompt {
        RenderedPrompt::Text(text) => vec![json!({"role": "user", "content": text})],
        RenderedPrompt::Messages(msgs) => msgs
            .iter()
            .map(|m| json!({"role": m.role, "content": m.content}))
            .collect(),
    }
}

/// Fields a reasoning model may use instead of `content` when it exposes its
/// chain of thought. `reasoning` is what ollama emits; `reasoning_content` is
/// the DeepSeek / vLLM spelling.
const REASONING_FIELDS: [&str; 2] = ["reasoning", "reasoning_content"];

/// The billable tokens in a chat-completions response.
///
/// The vendors disagree about what "input tokens" counts, and getting this
/// backwards double-charges every cached call at the full rate.
///
/// Anthropic's `input_tokens` *excludes* its cache counters. OpenAI's
/// `prompt_tokens` *includes* `cached_tokens`. [`TokenUsage`] follows
/// Anthropic's shape — the fields sum, and `input_tokens` means "tokens billed
/// at the full input rate" — so the cached span is subtracted out here rather
/// than counted twice.
///
/// Shared with the llm-rubric grader, which calls the same endpoint. That is
/// the point of it being a function: this subtraction is exactly the thing a
/// second implementation would forget.
pub(crate) fn usage_from_payload(payload: &Json) -> Option<TokenUsage> {
    payload.get("usage").map(|u| {
        let prompt_tokens = u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
        let cache_read_tokens = u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|v| v.as_u64());
        TokenUsage {
            // Saturating because the subtraction is over numbers a server sent
            // us: a provider reporting `cached_tokens > prompt_tokens` should
            // not underflow.
            input_tokens: prompt_tokens.saturating_sub(cache_read_tokens.unwrap_or(0)),
            output_tokens: u
                .get("completion_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            cache_read_tokens,
            // OpenAI's prompt caching is automatic and has no write step to
            // bill for, so there is nothing to report rather than a zero.
            cache_write_tokens: None,
            cache_write_1h_tokens: None,
        }
    })
}

fn parse_completion_response(
    payload: &Json,
    rate: Option<&crate::pricing::ModelRate>,
) -> Result<ProviderResponse, ProviderError> {
    let choice = payload
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first());
    let message = choice.and_then(|c| c.get("message"));

    let content = message
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .filter(|s| !s.trim().is_empty());

    // Reasoning models routinely return their whole answer in `reasoning` and
    // leave `content` empty — especially when `max_tokens` cuts them off before
    // they emit the final message. Scoring the empty string then fails every
    // assertion with nothing on any screen explaining why, so fall back to the
    // reasoning text rather than silently evaluating "".
    let reasoning = message.and_then(|m| {
        REASONING_FIELDS
            .iter()
            .find_map(|f| m.get(*f))
            .and_then(|r| r.as_str())
            .filter(|s| !s.trim().is_empty())
    });

    let text = content.or(reasoning).unwrap_or_default().to_string();

    let stop_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|s| s.as_str())
        .map(String::from);

    let usage = usage_from_payload(payload);

    // Record which field the scored text came from, so the UI can label an
    // answer that is really exposed reasoning instead of presenting it as the
    // model's final output.
    let mut raw = payload.clone();
    if content.is_none() && reasoning.is_some() {
        if let Some(obj) = raw.as_object_mut() {
            obj.insert(
                "domarinn_output_source".into(),
                Json::String("reasoning".into()),
            );
        }
    }

    let finish_reason = payload
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|c| c.first())
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str());

    let empty_reason = if text.trim().is_empty() {
        let mut candidates = Vec::new();
        // Shared with every other provider, including exec children: one
        // vocabulary rather than a hand-rolled match per call site, so a
        // reason added for one provider is understood by all of them.
        candidates.extend(finish_reason.and_then(crate::empty::from_stop_reason));
        if message.is_none() {
            candidates.push(EmptyReason::new(EmptyReason::NO_CONTENT_BLOCKS));
        } else if reasoning.is_some() {
            candidates.push(EmptyReason::new(EmptyReason::THINKING_ONLY));
        }
        candidates.push(EmptyReason::new(EmptyReason::BLANK));
        crate::empty::most_specific(&candidates)
    } else {
        None
    };

    // Costed here, in the parse path, rather than in the runner. A cache hit
    // replays `cost_usd` from its entry, so costing downstream would re-price a
    // replayed hit against today's rate table — and a run's cost would then
    // depend on when you read it. Computed once, with the rates in effect at
    // the moment of the call, and replayed verbatim forever.
    let cost_usd = rate
        .and_then(|r| usage.as_ref().and_then(|u| crate::pricing::cost_of(u, r)))
        .map(|c| c.to_usd());

    Ok(ProviderResponse {
        output: Output::Text(text),
        usage,
        cost_usd,
        stop_reason,
        raw: Some(raw),
        reasoning: reasoning.map(str::to_string),
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// See the matching test in `anthropic.rs` for why this is load-bearing:
    /// the fingerprint feeds every cache key, so an unconditional change here
    /// invalidates every cached entry in every store.
    /// The vendor trap: OpenAI's `prompt_tokens` *includes* `cached_tokens`,
    /// where Anthropic's `input_tokens` excludes its cache counters. Without
    /// the subtraction the cached span is billed at the full input rate on top
    /// of the discounted one.
    #[test]
    fn cached_prompt_tokens_are_not_double_counted() {
        let resp = parse_completion_response(
            &json!({
                "choices": [{"message": {"content": "hi"}}],
                "usage": {
                    "prompt_tokens": 1000,
                    "completion_tokens": 10,
                    "prompt_tokens_details": {"cached_tokens": 800}
                }
            }),
            None,
        )
        .unwrap();
        let usage = resp.usage.unwrap();
        assert_eq!(
            usage.input_tokens, 200,
            "full-rate input excludes the cached span"
        );
        assert_eq!(usage.cache_read_tokens, Some(800));
        // Everything the provider billed for still adds back up to what it sent.
        assert_eq!(usage.billable_total(), 1010);
    }

    #[test]
    fn fingerprint_is_stable_for_default_config() {
        let p = OpenAiProvider::new("p", "gpt-x", None, None, None, None);
        assert_eq!(
            crate::cache::canonical_json(&p.fingerprint()),
            r#"{"base_url":"https://api.openai.com/v1","model":"gpt-x","params":{},"type":"openai"}"#
        );
    }

    #[test]
    fn prefers_content_when_present() {
        let resp = parse_completion_response(
            &json!({
                "choices": [{"message": {"content": "the answer", "reasoning": "thinking…"}}]
            }),
            None,
        )
        .unwrap();
        assert_eq!(resp.output, Output::Text("the answer".into()));
        assert!(resp.raw.unwrap().get("domarinn_output_source").is_none());
    }

    #[test]
    fn falls_back_to_reasoning_when_content_is_empty() {
        // ollama's reasoning models put the whole answer here and leave
        // `content` empty, especially when cut off by max_tokens.
        let resp = parse_completion_response(
            &json!({
                "choices": [{
                    "message": {"content": "", "reasoning": "Thinking: the capital is Paris."},
                    "finish_reason": "length"
                }]
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            resp.output,
            Output::Text("Thinking: the capital is Paris.".into())
        );
        assert_eq!(
            resp.raw.unwrap().get("domarinn_output_source"),
            Some(&json!("reasoning"))
        );
    }

    #[test]
    fn falls_back_to_reasoning_content_spelling() {
        let resp = parse_completion_response(
            &json!({
                "choices": [{"message": {"reasoning_content": "deepseek style"}}]
            }),
            None,
        )
        .unwrap();
        assert_eq!(resp.output, Output::Text("deepseek style".into()));
    }

    #[test]
    fn treats_whitespace_only_content_as_absent() {
        let resp = parse_completion_response(
            &json!({
                "choices": [{"message": {"content": "   \n", "reasoning": "real text"}}]
            }),
            None,
        )
        .unwrap();
        assert_eq!(resp.output, Output::Text("real text".into()));
    }

    #[test]
    fn yields_empty_text_when_the_message_carries_nothing() {
        let resp = parse_completion_response(
            &json!({
                "choices": [{"message": {}}]
            }),
            None,
        )
        .unwrap();
        assert_eq!(resp.output, Output::Text(String::new()));
        assert!(resp.raw.unwrap().get("domarinn_output_source").is_none());
    }

    fn text_request() -> ProviderRequest {
        ProviderRequest {
            prompt: Some(RenderedPrompt::Text("hi".into())),
            vars: BTreeMap::new(),
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        }
    }

    #[test]
    fn request_preview_reports_the_exact_body_and_endpoint() {
        let mut params = ParamMap::new();
        params.insert("temperature".into(), json!(0.2));
        params.insert("max_tokens".into(), json!(256));
        let p = OpenAiProvider::new(
            "g",
            "gpt-x",
            // Trailing slash on purpose: the preview must trim it the same way
            // `call` does, or the two disagree about the URL.
            Some("https://gw.example/v1/".into()),
            None,
            Some(params),
            None,
        );

        let preview = p.request_preview(&text_request()).unwrap();
        assert_eq!(preview["transport"], json!("http"));
        assert_eq!(preview["method"], json!("POST"));
        assert_eq!(
            preview["url"],
            json!("https://gw.example/v1/chat/completions")
        );

        let body = &preview["body"];
        assert_eq!(body["model"], json!("gpt-x"));
        // Sampling params pass through verbatim — this is the whole point: a
        // `max_tokens` visible next to a `length` stop reason explains a
        // truncated case without leaving the drawer.
        assert_eq!(body["temperature"], json!(0.2));
        assert_eq!(body["max_tokens"], json!(256));
        // A text prompt is folded into a single user message, exactly as sent.
        assert_eq!(body["messages"], json!([{"role": "user", "content": "hi"}]));
        // Never headers: that is where the API key lives.
        assert!(preview.get("headers").is_none());
    }

    #[test]
    fn request_preview_matches_the_body_actually_built() {
        let p = OpenAiProvider::new("g", "m", None, None, None, None);
        let req = text_request();
        let preview = p.request_preview(&req).unwrap();
        assert_eq!(preview["body"], p.build_body(req.prompt.as_ref().unwrap()));
    }

    #[test]
    fn request_preview_is_absent_without_a_prompt() {
        let p = OpenAiProvider::new("g", "m", None, None, None, None);
        let req = ProviderRequest::default();
        assert!(p.request_preview(&req).is_none());
    }

    #[tokio::test]
    async fn calls_the_api_and_parses_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{"message": {"content": "hello"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 3, "completion_tokens": 1}
            })))
            .mount(&server)
            .await;
        std::env::set_var("OPENAI_TEST_KEY", "sk-test");
        let p = OpenAiProvider::new(
            "g",
            "gpt-x",
            Some(server.uri()),
            Some("OPENAI_TEST_KEY".into()),
            None,
            None,
        );
        let resp = p.call(&text_request(), &CallCtx::default()).await.unwrap();
        assert_eq!(resp.output, Output::Text("hello".into()));
        assert_eq!(resp.usage.unwrap().input_tokens, 3);
        assert_eq!(resp.stop_reason.as_deref(), Some("stop"));
    }

    #[tokio::test]
    async fn rate_limit_is_retriable_with_retry_after() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "2"))
            .mount(&server)
            .await;
        std::env::set_var("OPENAI_TEST_KEY2", "sk-test");
        let p = OpenAiProvider::new(
            "g",
            "gpt-x",
            Some(server.uri()),
            Some("OPENAI_TEST_KEY2".into()),
            None,
            None,
        );
        match p.call(&text_request(), &CallCtx::default()).await {
            Err(ProviderError::Retriable { retry_after, .. }) => {
                assert_eq!(retry_after, Some(Duration::from_secs(2)));
            }
            other => panic!("expected retriable, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod empty_classification_tests {
    use super::*;

    fn reason_of(payload: Json) -> Option<String> {
        parse_completion_response(&payload, None)
            .unwrap()
            .empty_reason
            .map(|r| r.as_str().to_string())
    }

    #[test]
    fn a_length_finish_is_reported_as_truncation() {
        let payload = json!({
            "choices": [{"message": {"content": ""}, "finish_reason": "length"}]
        });
        assert_eq!(reason_of(payload).as_deref(), Some(EmptyReason::TRUNCATED));
    }

    #[test]
    fn a_content_filter_finish_is_named_as_such() {
        let payload = json!({
            "choices": [{"message": {"content": ""}, "finish_reason": "content_filter"}]
        });
        assert_eq!(
            reason_of(payload).as_deref(),
            Some(EmptyReason::CONTENT_FILTER)
        );
    }

    #[test]
    fn a_missing_choices_array_is_a_protocol_fault() {
        assert_eq!(
            reason_of(json!({})).as_deref(),
            Some(EmptyReason::NO_CONTENT_BLOCKS)
        );
    }

    /// Reasoning is now a first-class field, not only a substitution into
    /// `output` recorded via a marker buried in `raw`.
    #[test]
    fn reasoning_is_captured_as_a_field_even_when_substituted_into_output() {
        let payload = json!({
            "choices": [{"message": {"content": "", "reasoning": "thinking aloud"}}]
        });
        let resp = parse_completion_response(&payload, None).unwrap();
        assert_eq!(resp.reasoning.as_deref(), Some("thinking aloud"));
        // Existing substitution behavior is preserved, so nothing regresses.
        assert_eq!(resp.output, Output::Text("thinking aloud".into()));
    }

    #[test]
    fn a_normal_answer_has_no_empty_reason() {
        let payload = json!({
            "choices": [{"message": {"content": "42"}, "finish_reason": "stop"}]
        });
        assert!(parse_completion_response(&payload, None)
            .unwrap()
            .empty_reason
            .is_none());
    }
}
