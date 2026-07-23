//! Native Anthropic Messages API provider.
//!
//! A thin hand-rolled client so parameters pass through verbatim: no forced
//! `temperature`, no hidden overrides. `max_tokens` is required by the API, so
//! it defaults only when absent.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::config::ParamMap;
use crate::net::{api_key, http_client, parse_retry_after, status_error, transport_error};
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::types::{ChatRole, Output, RenderedPrompt, TokenUsage};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MAX_TOKENS: u64 = 4096;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct AnthropicProvider {
    id: String,
    model: String,
    base_url: String,
    api_key_env: String,
    params: ParamMap,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(
        id: impl Into<String>,
        model: impl Into<String>,
        base_url: Option<String>,
        api_key_env: Option<String>,
        params: Option<ParamMap>,
    ) -> Self {
        AnthropicProvider {
            id: id.into(),
            model: model.into(),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string()),
            api_key_env: api_key_env.unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string()),
            params: params.unwrap_or_default(),
            client: http_client(DEFAULT_TIMEOUT),
        }
    }

    fn build_body(&self, prompt: &RenderedPrompt) -> Json {
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
        Json::Object(body)
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
            ProviderError::Fatal(anyhow::anyhow!("anthropic provider requires a prompt"))
        })?;
        let key = api_key(&self.api_key_env)?;
        let body = self.build_body(prompt);
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));

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
        parse_messages_response(&payload)
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

fn parse_messages_response(payload: &Json) -> Result<ProviderResponse, ProviderError> {
    let text = payload
        .get("content")
        .and_then(|c| c.as_array())
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    let usage = payload.get("usage").map(|u| TokenUsage {
        input_tokens: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        output_tokens: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        cache_read_tokens: u.get("cache_read_input_tokens").and_then(|v| v.as_u64()),
    });
    let stop_reason = payload
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .map(String::from);

    Ok(ProviderResponse {
        output: Output::Text(text),
        usage,
        cost_usd: None,
        stop_reason,
        raw: Some(payload.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use std::collections::BTreeMap;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

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
    fn body_defaults_max_tokens_but_keeps_params() {
        let mut params = serde_json::Map::new();
        params.insert("temperature".into(), json!(0.5));
        let p = AnthropicProvider::new("c", "claude-x", None, None, Some(params));
        let body = p.build_body(&RenderedPrompt::Text("hi".into()));
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
        let p = AnthropicProvider::new("c", "m", None, Some("ANTHROPIC_TEST_KEY3".into()), None);
        let req = ProviderRequest {
            prompt: None,
            vars: BTreeMap::new(),
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        };
        assert!(matches!(
            p.call(&req, &CallCtx::default()).await,
            Err(ProviderError::Fatal(_))
        ));
    }
}
