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

use crate::config::HttpMethod;
use crate::error_class::ErrorClass;
use crate::net::{http_client, parse_retry_after, status_error, transport_error};
use crate::provider::{CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse};
use crate::template::TemplateEngine;
use crate::types::{Output, RenderedPrompt};
use crate::val::Val;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

pub struct HttpProvider {
    id: String,
    url: String,
    method: HttpMethod,
    headers: BTreeMap<String, String>,
    body: Option<Json>,
    output_expr: Option<String>,
    /// Digest of `headers`, so two providers differing only there stop sharing a
    /// key. See [`Self::fingerprint`]; `None` when no header is set.
    headers_digest: Option<String>,
    client: reqwest::Client,
}

impl HttpProvider {
    pub fn new(
        id: impl Into<String>,
        url: impl Into<String>,
        method: Option<HttpMethod>,
        headers: BTreeMap<String, String>,
        body: Option<Json>,
        output_expr: Option<String>,
    ) -> Self {
        let id = id.into();
        let url = url.into();
        warn_on_runtime_env(&id, &url, &headers, body.as_ref());
        HttpProvider {
            headers_digest: headers_digest(&headers),
            id,
            url,
            method: method.unwrap_or_default(),
            headers,
            body,
            output_expr,
            client: http_client(DEFAULT_TIMEOUT),
        }
    }
}

/// A digest of the provider's declared headers, or `None` when it sets none.
///
/// `None` rather than the digest of an empty map so a provider that declares no
/// header keeps the fingerprint — and therefore every cache entry — it had
/// before this member existed.
///
/// The *unrendered* templates are hashed, which is what makes this both safe and
/// useful. `X-Model: gpt-5` and `X-Model: claude-opus-5` separate, because the
/// templates differ. `Authorization: Bearer {{ env.TOKEN }}` does not, because
/// the template is the same for two teammates holding different tokens — and
/// keying a credential would partition a shared cache by who ran it. A digest
/// rather than the values themselves because a fingerprint is persisted into
/// every cache entry, and a header is where a literal secret would sit.
fn headers_digest(headers: &BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    let canonical = crate::cache::canonical_json(&serde_json::json!(headers));
    Some(format!(
        "blake3:{}",
        blake3::hash(canonical.as_bytes()).to_hex()
    ))
}

/// Warn when this provider's templates read the environment at call time.
///
/// `{{ env.VAR }}` is rendered per request, so its *value* never reaches the
/// fingerprint — only the template text does. That is correct for a credential
/// and wrong for anything else: `url: ".../{{ env.MODEL }}"` gives two models
/// one cache key, and the second silently replays the first's answers.
///
/// domarinn cannot tell which is which. A credential must not separate two
/// teammates' entries; a model selector must separate them. So it says what it
/// sees and names the alternative: `${env:VAR}` is resolved at load time, before
/// the provider is built, so the substituted value is in the fingerprint.
///
/// Fired once at construction rather than per call, and only for statically
/// visible accesses — a dynamic `env[key]` lookup warns without naming a
/// variable, which is still the useful half of the message.
fn warn_on_runtime_env(
    id: &str,
    url: &str,
    headers: &BTreeMap<String, String>,
    body: Option<&Json>,
) {
    let mut sources = vec![url.to_string()];
    sources.extend(headers.values().cloned());
    if let Some(body) = body {
        sources.push(body.to_string());
    }
    let mut names: Vec<String> = sources
        .iter()
        .flat_map(|s| runtime_env_refs(s))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if names.is_empty() {
        return;
    }
    names.sort();
    tracing::warn!(
        provider = %id,
        vars = ?names,
        "this provider's url/headers/body read the environment at call time via \
         `{{{{ env.X }}}}`, which is rendered per request and so is NOT part of the \
         cache key. That is right for a credential and wrong for anything that \
         changes the answer: two values would share one cache entry, and the second \
         would replay the first's responses. Use `${{env:X}}` instead for those — it \
         resolves at load time and is keyed."
    );
}

/// Variable names reached through `env.` in a minijinja template.
///
/// Recognises `env.NAME` and `env['NAME']` / `env["NAME"]`, the two spellings
/// the docs use. Anything else that touches `env` — a computed lookup — yields
/// the placeholder `*`, so the warning still fires without inventing a name.
fn runtime_env_refs(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (idx, _) in template.match_indices("env") {
        // Reject `SOME_env` / `envelope`: this must be the whole identifier.
        let before = template[..idx].chars().next_back();
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let rest = &template[idx + 3..];
        let name: String = match rest.chars().next() {
            Some('.') => rest[1..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect(),
            Some('[') => rest[1..]
                .trim_start()
                .trim_start_matches(['\'', '"'])
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect(),
            // A bare `env` handed to a filter or iterated over.
            _ => continue,
        };
        out.push(if name.is_empty() {
            "*".to_string()
        } else {
            name
        });
    }
    out
}

#[async_trait]
impl Provider for HttpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    /// What selects this endpoint: where the request goes, how it is shaped, and
    /// how the answer is read back out.
    ///
    /// `headers` are in it as a digest. Without them, two providers wrapping one
    /// endpoint and differing only in a header — `X-Model: gpt-5` against
    /// `X-Model: claude-opus-5`, or one tenant's key against another's — shared
    /// every entry, so the second column of a comparison replayed the first's
    /// answers and the run reported a difference of zero it never measured. This
    /// is the same collision `env` closes for `exec` providers.
    ///
    /// The templates are hashed unrendered, deliberately: see [`headers_digest`]
    /// for why that separates a model selector without partitioning a shared
    /// cache by whose credential was used. The same reasoning is why `url` and
    /// `body` are published unrendered.
    ///
    /// The member is inserted **only when a header is declared**, under the
    /// discipline [`crate::cache_key`] spells out: canonical JSON emits every
    /// member that is present, so a `null` one hashes differently from an absent
    /// one. Adding it unconditionally would re-key every `http` provider that
    /// sets no headers — which is most of them — to fix a collision none of them
    /// can have.
    fn fingerprint(&self) -> Json {
        let mut fp = json!({
            "type": "http",
            "url": self.url,
            "method": self.method,
            "body": self.body,
            "output_expr": self.output_expr,
        });
        if let Some(digest) = &self.headers_digest {
            fp.as_object_mut()
                .expect("json! object literal")
                .insert("headers".to_string(), Json::String(digest.clone()));
        }
        fp
    }

    async fn call(
        &self,
        req: &ProviderRequest,
        _ctx: &CallCtx,
    ) -> Result<ProviderResponse, ProviderError> {
        let engine = TemplateEngine::new();
        let render_ctx = render_context(req);

        let url = engine.render_str(&self.url, &render_ctx).map_err(|e| {
            ProviderError::fatal(
                ErrorClass::RENDER_FAILED,
                anyhow::anyhow!("rendering url: {e}"),
            )
        })?;
        // Infallible: `HttpMethod` is a closed set validated at config parse
        // time, so there is no invalid-method error path left to handle here.
        let method = match self.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Delete => reqwest::Method::DELETE,
            HttpMethod::Head => reqwest::Method::HEAD,
        };

        let mut request = self.client.request(method, &url);
        for (name, template) in &self.headers {
            let value = engine.render_str(template, &render_ctx).map_err(|e| {
                ProviderError::fatal(
                    ErrorClass::RENDER_FAILED,
                    anyhow::anyhow!("rendering header {name}: {e}"),
                )
            })?;
            request = request.header(name, value);
        }
        if let Some(body) = &self.body {
            let rendered = engine
                .render_val(&Val::Tpl(body.clone()), &render_ctx)
                .map_err(|e| {
                    ProviderError::fatal(
                        ErrorClass::RENDER_FAILED,
                        anyhow::anyhow!("rendering body: {e}"),
                    )
                })?;
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
                    ProviderError::fatal(
                        ErrorClass::PROVIDER_PROTOCOL,
                        anyhow::anyhow!("output_expr `{expr}`: {e}"),
                    )
                })?;
                match value {
                    Json::String(s) => Output::Text(s),
                    other => Output::Json(other),
                }
            }
            None => Output::Text(text),
        };

        Ok(ProviderResponse {
            // A generic HTTP endpoint has no tool-call convention to read, so
            // there is nothing to report rather than an empty claim.
            tool_calls: Vec::new(),
            output,
            usage: None,
            cost_usd: None,
            // An HTTP provider addresses an arbitrary endpoint; there is no
            // model concept to report.
            model: None,
            stop_reason: None,
            reasoning: None,
            empty_reason: None,
            raw: json_body,
        })
    }

    // `request_preview` is deliberately left at its `None` default.
    //
    // This provider's url, headers, and body are templates rendered against
    // `env` (see `render_context`), which is exactly how a suite supplies its
    // credentials — `{"api_key": "{{ env.SUT_TOKEN }}"}` is the documented way
    // to authenticate a black-box system under test. A rendered preview would
    // bake that secret into `CaseResult`, which is persisted and uploaded by
    // `--share`. The same reasoning is why `fingerprint` publishes the *unrendered*
    // `self.body` and never the rendered one.
    //
    // If this is ever implemented, it needs a redaction pass over the rendered
    // values, not a straight `render_val` — the drawer falls back to showing the
    // rendered prompt, which leaks nothing, and that is the correct trade here.
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
                .map(|m| format!("{}: {}", m.role.as_str(), m.content))
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

    /// See the matching test in `anthropic.rs`: the fingerprint feeds every
    /// cache key, so an unconditional change invalidates every cached entry.
    #[test]
    fn fingerprint_is_stable_for_default_config() {
        let p = HttpProvider::new(
            "p",
            "https://example.test/v1",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        assert_eq!(
            crate::cache::canonical_json(&p.fingerprint()),
            r#"{"body":null,"method":"POST","output_expr":null,"type":"http","url":"https://example.test/v1"}"#
        );
    }

    fn request_with_var(key: &str, value: &str) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert(key.to_string(), Json::String(value.to_string()));
        ProviderRequest {
            tools: Vec::new(),
            prompt: Some(RenderedPrompt::Text("summarize this".into())),
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        }
    }

    #[test]
    fn request_preview_is_withheld_because_templates_can_carry_secrets() {
        // A regression guard for a deliberate omission, not a TODO. This
        // provider's url/headers/body are rendered against `env`, which is the
        // documented way to authenticate a black-box system under test — so a
        // captured preview would persist that credential into a `--share`d run
        // document. Implementing this needs a redaction pass first.
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1?key={{ env.SUT_TOKEN }}",
            None,
            BTreeMap::new(),
            Some(serde_json::json!({"api_key": "{{ env.SUT_TOKEN }}"})),
            None,
        );
        assert!(p.request_preview(&request_with_var("q", "hi")).is_none());
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
            Some(HttpMethod::Post),
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
            Some(HttpMethod::Get),
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

    #[tokio::test]
    async fn missing_method_defaults_to_post_on_the_wire_and_in_the_fingerprint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;
        let p = HttpProvider::new("gw", server.uri(), None, BTreeMap::new(), None, None);
        assert_eq!(p.fingerprint()["method"], json!("POST"));
        let resp = p
            .call(&request_with_var("x", "y"), &CallCtx::default())
            .await
            .unwrap();
        assert_eq!(resp.output, Output::Text("ok".into()));
    }
}
