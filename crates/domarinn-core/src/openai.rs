//! OpenAI-compatible chat-completions provider.
//!
//! Works against the OpenAI API and any compatible gateway (vLLM, LiteLLM,
//! Together, Ollama, ...). Parameters pass through verbatim.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::config::ParamMap;
use crate::net::{api_key, http_client, parse_retry_after, status_error, transport_error};
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::types::{Output, RenderedPrompt, TokenUsage};

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OpenAiProvider {
    id: String,
    model: String,
    base_url: String,
    api_key_env: String,
    params: ParamMap,
    client: reqwest::Client,
}

impl OpenAiProvider {
    pub fn new(
        id: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key_env: Option<String>,
        params: Option<ParamMap>,
    ) -> Self {
        OpenAiProvider {
            id: id.into(),
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "OPENAI_API_KEY".to_string()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
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
            ProviderError::Fatal(anyhow::anyhow!("openai provider requires a prompt"))
        })?;
        let key = api_key(&self.api_key_env)?;
        let body = self.build_body(prompt);
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));

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
        parse_completion_response(&payload)
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

fn parse_completion_response(payload: &Json) -> Result<ProviderResponse, ProviderError> {
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

    let usage = payload.get("usage").map(|u| TokenUsage {
        input_tokens: u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_read_tokens: None,
    });

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

    Ok(ProviderResponse {
        output: Output::Text(text),
        usage,
        cost_usd: None,
        stop_reason,
        raw: Some(raw),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use std::collections::BTreeMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn prefers_content_when_present() {
        let resp = parse_completion_response(&json!({
            "choices": [{"message": {"content": "the answer", "reasoning": "thinking…"}}]
        }))
        .unwrap();
        assert_eq!(resp.output, Output::Text("the answer".into()));
        assert!(resp.raw.unwrap().get("domarinn_output_source").is_none());
    }

    #[test]
    fn falls_back_to_reasoning_when_content_is_empty() {
        // ollama's reasoning models put the whole answer here and leave
        // `content` empty, especially when cut off by max_tokens.
        let resp = parse_completion_response(&json!({
            "choices": [{
                "message": {"content": "", "reasoning": "Thinking: the capital is Paris."},
                "finish_reason": "length"
            }]
        }))
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
        let resp = parse_completion_response(&json!({
            "choices": [{"message": {"reasoning_content": "deepseek style"}}]
        }))
        .unwrap();
        assert_eq!(resp.output, Output::Text("deepseek style".into()));
    }

    #[test]
    fn treats_whitespace_only_content_as_absent() {
        let resp = parse_completion_response(&json!({
            "choices": [{"message": {"content": "   \n", "reasoning": "real text"}}]
        }))
        .unwrap();
        assert_eq!(resp.output, Output::Text("real text".into()));
    }

    #[test]
    fn yields_empty_text_when_the_message_carries_nothing() {
        let resp = parse_completion_response(&json!({
            "choices": [{"message": {}}]
        }))
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
        );
        match p.call(&text_request(), &CallCtx::default()).await {
            Err(ProviderError::Retriable { retry_after, .. }) => {
                assert_eq!(retry_after, Some(Duration::from_secs(2)));
            }
            other => panic!("expected retriable, got {other:?}"),
        }
    }
}
