//! A generic HTTP provider for black-box systems under test.
//!
//! The URL, headers, and body are templated against the test vars (and `env`,
//! and the rendered `prompt`). The response is exposed to an optional
//! `output_expr` (a minijinja expression over `response.{status,text,json}`),
//! which selects the output; without it the raw response text is the output.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value as Json};

use crate::net::{http_client, parse_retry_after, status_error, transport_error};
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::template::TemplateEngine;
use crate::types::{Output, RenderedPrompt};
use crate::val::Val;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct HttpProvider {
    id: String,
    url: String,
    method: String,
    headers: BTreeMap<String, String>,
    body: Option<Json>,
    output_expr: Option<String>,
    client: reqwest::Client,
}

impl HttpProvider {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        method: Option<String>,
        headers: BTreeMap<String, String>,
        body: Option<Json>,
        output_expr: Option<String>,
    ) -> Self {
        HttpProvider {
            id: id.into(),
            url: url.into(),
            method: method.unwrap_or_else(|| "POST".to_string()),
            headers,
            body,
            output_expr,
            client: http_client(DEFAULT_TIMEOUT),
        }
    }
}

#[async_trait]
impl Provider for HttpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn fingerprint(&self) -> Json {
        json!({
            "type": "http",
            "url": self.url,
            "method": self.method,
            "body": self.body,
            "output_expr": self.output_expr,
        })
    }

    async fn call(
        &self,
        req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let engine = TemplateEngine::new();
        let render_ctx = render_context(req);

        let url = engine
            .render_str(&self.url, &render_ctx)
            .map_err(|e| ProviderError::Fatal(anyhow::anyhow!("rendering url: {e}")))?;
        let method = reqwest::Method::from_bytes(self.method.as_bytes())
            .map_err(|e| ProviderError::Fatal(anyhow::anyhow!("bad method: {e}")))?;

        let mut request = self.client.request(method, &url);
        for (name, template) in &self.headers {
            let value = engine.render_str(template, &render_ctx).map_err(|e| {
                ProviderError::Fatal(anyhow::anyhow!("rendering header {name}: {e}"))
            })?;
            request = request.header(name, value);
        }
        if let Some(body) = &self.body {
            let rendered = engine
                .render_val(&Val::Tpl(body.clone()), &render_ctx)
                .map_err(|e| ProviderError::Fatal(anyhow::anyhow!("rendering body: {e}")))?;
            request = request.json(&rendered);
        }

        let response = request.send().await.map_err(transport_error)?;
        let status = response.status();
        let retry = parse_retry_after(response.headers());
        let headers = header_map(response.headers());
        let text = response.text().await.map_err(transport_error)?;

        if !status.is_success() {
            return Err(status_error(status, retry, text));
        }

        let json_body: Option<Json> = serde_json::from_str(&text).ok();
        let output = match &self.output_expr {
            Some(expr) => {
                let ctx = json!({
                    "response": {
                        "status": status.as_u16(),
                        "text": text,
                        "json": json_body,
                        "headers": headers,
                    }
                });
                let value = engine.eval_value(expr, &ctx).map_err(|e| {
                    ProviderError::Fatal(anyhow::anyhow!("output_expr `{expr}`: {e}"))
                })?;
                match value {
                    Json::String(s) => Output::Text(s),
                    other => Output::Json(other),
                }
            }
            None => Output::Text(text),
        };

        Ok(ProviderResponse {
            output,
            usage: None,
            cost_usd: None,
            stop_reason: None,
            raw: json_body,
        })
    }
}

fn render_context(req: &ProviderRequest) -> Json {
    let mut obj = req
        .vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<serde_json::Map<_, _>>();
    // Request vars intentionally omit the environment (so it stays out of the
    // cache key); expose it here for `{{ env.X }}` in url/headers/body.
    obj.insert("env".to_string(), crate::render::env_object());
    if let Some(prompt) = &req.prompt {
        let text = match prompt {
            RenderedPrompt::Text(t) => t.clone(),
            RenderedPrompt::Messages(msgs) => msgs
                .iter()
                .map(|m| format!("{}: {}", m.role, m.content))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        obj.insert("prompt".into(), Json::String(text));
    }
    Json::Object(obj)
}

fn header_map(headers: &reqwest::header::HeaderMap) -> Json {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        if let Ok(v) = value.to_str() {
            map.insert(name.to_string(), Json::String(v.to_string()));
        }
    }
    Json::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn request_with_var(key: &str, value: &str) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert(key.to_string(), Json::String(value.to_string()));
        ProviderRequest {
            prompt: Some(RenderedPrompt::Text("summarize this".into())),
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
        }
    }

    #[tokio::test]
    async fn posts_templated_body_and_extracts_with_output_expr() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/complete"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"completion": "done"})))
            .mount(&server)
            .await;

        let p = HttpProvider::new(
            "gw",
            format!("{}/complete", server.uri()),
            Some("POST".into()),
            BTreeMap::new(),
            Some(json!({"prompt": "{{ prompt }}", "doc": "{{ doc }}"})),
            Some("response.json.completion".into()),
        );
        let resp = p
            .call(&request_with_var("doc", "hello"), &CallCtx::default())
            .await
            .unwrap();
        assert_eq!(resp.output, Output::Text("done".into()));
    }

    #[tokio::test]
    async fn without_output_expr_returns_raw_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("plain body"))
            .mount(&server)
            .await;
        let p = HttpProvider::new(
            "gw",
            server.uri(),
            Some("GET".into()),
            BTreeMap::new(),
            None,
            None,
        );
        let resp = p
            .call(&request_with_var("x", "y"), &CallCtx::default())
            .await
            .unwrap();
        assert_eq!(resp.output, Output::Text("plain body".into()));
    }
}
