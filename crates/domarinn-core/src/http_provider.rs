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
use crate::provider::{
    http_request_preview, CallCtx, Provider, ProviderError, ProviderRequest, ProviderResponse,
};
use crate::request_cfg::{headers_digest, warn_on_runtime_env};
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
        let mut sources = vec![url.clone()];
        sources.extend(headers.values().cloned());
        sources.extend(body.iter().map(|b| b.to_string()));
        warn_on_runtime_env(&id, sources);
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

    /// Merge the [`crate::request_cfg::GLOBAL_HEADERS_ENV`] injection into this
    /// provider's declared headers.
    ///
    /// Applies here as well as to the vendor providers because an egress proxy
    /// does not care which provider type the traffic came from. The suite's own
    /// headers win by name — the environment supplies a default, and a suite that
    /// named the header meant it.
    ///
    /// A builder rather than part of [`Self::new`] so a provider constructed
    /// directly stays free of ambient environment, and so the one fallible step
    /// (the variable is malformed JSON) does not put a `Result` on every call
    /// site that never sets it.
    pub fn with_global_headers(mut self) -> Result<Self, crate::request_cfg::RequestError> {
        let global = crate::request_cfg::global_headers()?;
        if global.is_empty() {
            return Ok(self);
        }
        let mut headers = global;
        headers.extend(self.headers.clone());
        self.headers_digest = headers_digest(&headers);
        self.headers = headers;
        Ok(self)
    }

    /// Render this provider's templates against `req` and `env` into the request
    /// that would go on the wire.
    ///
    /// The `env` object is a parameter rather than read here because it is the
    /// one axis on which the sent request and the *keyed* one differ:
    /// [`Provider::call`] passes [`crate::render::env_object`], while
    /// [`Provider::canonical_request`] passes
    /// [`crate::render::env_placeholder_object`]. Everything else — the vars,
    /// the prompt, the order the templates are rendered in, the error each
    /// failure produces — is shared, so the two cannot drift.
    fn build_request(
        &self,
        req: &ProviderRequest,
        env: &Json,
    ) -> Result<BuiltHttpRequest, ProviderError> {
        let engine = TemplateEngine::new();
        let render_ctx = render_context(req, env);

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

        let mut headers = BTreeMap::new();
        for (name, template) in &self.headers {
            let value = engine.render_str(template, &render_ctx).map_err(|e| {
                ProviderError::fatal(
                    ErrorClass::RENDER_FAILED,
                    anyhow::anyhow!("rendering header {name}: {e}"),
                )
            })?;
            headers.insert(name.clone(), value);
        }

        let body = match &self.body {
            Some(body) => Some(
                engine
                    .render_val(&Val::Tpl(body.clone()), &render_ctx)
                    .map_err(|e| {
                        ProviderError::fatal(
                            ErrorClass::RENDER_FAILED,
                            anyhow::anyhow!("rendering body: {e}"),
                        )
                    })?,
            ),
            None => None,
        };

        Ok(BuiltHttpRequest {
            url,
            method,
            headers,
            body,
        })
    }
}

/// One rendered outgoing request: what [`Provider::call`] sends, and — rendered
/// against placeholder `env` — what the cache keys and stores.
struct BuiltHttpRequest {
    url: String,
    method: reqwest::Method,
    headers: BTreeMap<String, String>,
    /// `None` when the provider declares no body template, which is not the
    /// same as declaring an empty one.
    body: Option<Json>,
}

#[async_trait]
impl Provider for HttpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    /// What selects this endpoint: where the request goes, how it is shaped, and
    /// how the answer is read back out.
    ///
    /// Frozen history since 0.5.0, per [`Provider::fingerprint`] — the live key
    /// hashes [`Self::canonical_request`] instead. What is written below in the
    /// present tense describes the ≤0.4.x key this shape produced, which
    /// `cache_migrate` still recomputes to adopt entries under it.
    ///
    /// `headers` are in it as a digest. Without them, two providers wrapping one
    /// endpoint and differing only in a header — `X-Model: gpt-5` against
    /// `X-Model: claude-opus-5`, or one tenant's key against another's — shared
    /// every entry, so the second column of a comparison replayed the first's
    /// answers and the run reported a difference of zero it never measured. This
    /// is the same collision `env` closes for `exec` providers. The canonical
    /// request closes it the same way, with a digest of the *rendered* headers.
    ///
    /// The templates are hashed unrendered, deliberately: see
    /// [`crate::request_cfg::headers_digest`]
    /// for why that separates a model selector without partitioning a shared
    /// cache by whose credential was used. The same reasoning is why `url` and
    /// `body` are published unrendered.
    ///
    /// The member is inserted **only when a header is declared**, under the
    /// discipline [`crate::cache_migrate::legacy_provider_key`] spells out:
    /// canonical JSON emits every member that is present, so a `null` one hashes
    /// differently from an absent one. Adding it unconditionally would re-key
    /// every `http` provider that sets no headers — which is most of them — to
    /// fix a collision none of them can have.
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
        let built = self.build_request(req, &crate::render::env_object())?;

        let mut request = self.client.request(built.method, &built.url);
        for (name, value) in &built.headers {
            request = request.header(name, value);
        }
        if let Some(body) = &built.body {
            request = request.json(body);
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
                let value = TemplateEngine::new().eval_value(expr, &ctx).map_err(|e| {
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

    /// The resolved request, rendered against placeholder `env`.
    ///
    /// The double render *is* the redaction pass this preview was withheld for.
    /// This provider's url, headers, and body are templates rendered against
    /// `env`, which is the documented way to supply a black-box system under
    /// test's credentials — `{"api_key": "{{ env.SUT_TOKEN }}"}` — and a preview
    /// is persisted into `CaseResult` and uploaded by `--share`. Rendering
    /// against [`crate::render::env_placeholder_object`] resolves the vars and
    /// the prompt for real, while `env` read by *this provider's own templates*
    /// comes out as the literal `${env:NAME}` that names it.
    ///
    /// The redaction is that one hop, deliberately. A case var that reads the
    /// environment itself — `vars: {token: "{{ env.SUT_TOKEN }}"}`, then
    /// `url: "…?key={{ token }}"` — is already resolved by the time this
    /// provider sees it, and lands here in the clear. That is by design: vars are
    /// case data, they have always been cache-key members, and `CaseResult.vars`
    /// publishes them. So a credential belongs in this provider's templates as
    /// `{{ env.X }}` and must never be routed through a case var.
    ///
    /// Headers are absent, as in every provider's preview envelope: that is
    /// where the credential sits even when it is a pasted literal rather than an
    /// `env` reference.
    fn request_preview(&self, req: &ProviderRequest) -> Option<Json> {
        let built = self
            .build_request(req, &crate::render::env_placeholder_object())
            .ok()?;
        Some(http_request_preview(
            built.method.as_str(),
            &built.url,
            built.body.unwrap_or(Json::Null),
        ))
    }

    /// The same placeholder-rendered request, keyed rather than displayed.
    ///
    /// Three members are inserted only when the provider declares them, so the
    /// envelope says nothing about a body, headers, or a projection that do not
    /// exist:
    ///
    /// - `body`, when a body template is configured.
    /// - `headers_digest`, when any header is. A digest rather than the values
    ///   because a canonical request is persisted into every cache entry and
    ///   entries travel to shared stores, and a header is exactly where a
    ///   pasted-literal secret would sit. It digests the *rendered* headers, so
    ///   a header reading a case var separates two cases while one reading
    ///   `{{ env.TOKEN }}` does not — see [`crate::request_cfg::headers_digest`] for the same
    ///   argument applied to the unrendered templates in the fingerprint.
    /// - `output_expr`, when one is declared.
    ///
    /// `output_expr` is the deliberate asymmetry between this document and
    /// [`Self::request_preview`]: it **keys** but does not **preview**. It keys
    /// because an entry stores the already-projected output, so the expression
    /// decides what the stored answer *means* — two providers reading different
    /// fields out of one endpoint's response are not asking the same question,
    /// and editing one must throw its own answers away. That is the same
    /// argument the `anthropic` API-version header is keyed on: suite-authored,
    /// non-secret configuration that changes the meaning of a response. It does
    /// not preview because a preview is what goes on the wire, and response
    /// processing never does.
    ///
    /// Keyed as configured rather than rendered, matching the shape the ≤0.4.x
    /// fingerprint published: it is a minijinja *expression* evaluated against
    /// the response (see [`Provider::call`]), not a template rendered against
    /// the case — there is nothing to render it with at request time.
    ///
    /// A render failure yields `None`: an unrenderable request has no identity
    /// to key on, and the live call surfaces the `RENDER_FAILED` itself.
    fn canonical_request(&self, req: &ProviderRequest) -> Option<Json> {
        let built = self
            .build_request(req, &crate::render::env_placeholder_object())
            .ok()?;
        let mut canonical = json!({
            "transport": "http",
            "method": built.method.as_str(),
            "url": built.url,
        });
        let members = canonical.as_object_mut().expect("json! object literal");
        if let Some(body) = built.body {
            members.insert("body".to_string(), body);
        }
        if let Some(digest) = headers_digest(&built.headers) {
            members.insert("headers_digest".to_string(), Json::String(digest));
        }
        if let Some(expr) = &self.output_expr {
            members.insert("output_expr".to_string(), Json::String(expr.clone()));
        }
        Some(canonical)
    }
}

/// The template context for one call: the rendered vars, the prompt, and `env`.
///
/// `env` is a parameter because the caller decides whether its *values* are
/// visible — see [`HttpProvider::build_request`]. Request vars intentionally
/// omit the environment (so it stays out of the cache key); this is where
/// `{{ env.X }}` in url/headers/body gets it.
fn render_context(req: &ProviderRequest, env: &Json) -> Json {
    let mut obj = req
        .vars
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<serde_json::Map<_, _>>();
    obj.insert("env".to_string(), env.clone());
    if let Some(prompt) = &req.prompt {
        let text = match prompt {
            RenderedPrompt::Text(t) => t.clone(),
            // Prose only, deliberately: this string is lossy by construction
            // (it already discards message boundaries), and inventing a textual
            // rendering for a tool call is exactly the imitation hazard the
            // structured `tool_calls` field exists to avoid. A tool-bearing
            // transcript is visible in `{{ messages }}` below.
            RenderedPrompt::Messages(msgs) => msgs
                .iter()
                .map(|m| format!("{}: {}", m.role.as_str(), m.content.text()))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        obj.insert("prompt".into(), Json::String(text));
        // The same turns, structurally: `{{ messages | tojson }}` or a
        // `{% for %}` loop lets a body template forward a real conversation. A
        // `Text` prompt appears as the single user turn it becomes on the wire
        // (the shape `openai::to_messages` produces).
        //
        // A test var named `messages` wins: this key arrived after suites that
        // forwarded hand-rolled conversations under that very name, and
        // overwriting the var would change their rendered request — and move
        // its cache key — on upgrade. (`prompt` overwrites vars, but it has
        // done so since this provider existed; that ship has sailed.)
        if !obj.contains_key("messages") {
            let messages = match prompt {
                RenderedPrompt::Text(t) => {
                    serde_json::json!([{"role": "user", "content": t}])
                }
                RenderedPrompt::Messages(msgs) => {
                    serde_json::to_value(msgs).unwrap_or(Json::Array(Vec::new()))
                }
            };
            obj.insert("messages".into(), messages);
        }
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

    /// Body templates get the turns structurally as `{{ messages }}`, not just
    /// the flattened `{{ prompt }}` string — the only way a caller-authored
    /// HTTP body can forward a real conversation to an OpenAI-shaped API.
    #[test]
    fn body_templates_get_structured_messages() {
        use crate::types::{ChatMessage, ChatRole};
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            None,
            BTreeMap::new(),
            Some(json!({"transcript": "{{ messages | tojson }}", "flat": "{{ prompt }}"})),
            None,
        );
        let mut req = request_with_var("doc", "hi");
        req.prompt = Some(RenderedPrompt::Messages(vec![
            ChatMessage::text(ChatRole::User, "hi"),
            ChatMessage::text(ChatRole::Assistant, "hello"),
        ]));
        let canonical = p.canonical_request(&req).unwrap();
        let transcript: Json =
            serde_json::from_str(canonical["body"]["transcript"].as_str().unwrap()).unwrap();
        assert_eq!(
            transcript,
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ])
        );
        // `{{ prompt }}` keeps its documented flattening, unchanged.
        assert_eq!(
            canonical["body"]["flat"],
            json!("user: hi\nassistant: hello")
        );
    }

    /// A suite that already had a var named `messages` (say, a hand-rolled
    /// conversation forwarded as JSON text — exactly the workaround the
    /// structured context supersedes) must keep rendering the var: anything
    /// else silently changes the request sent to the SUT and moves its cache
    /// key on upgrade.
    #[test]
    fn a_var_named_messages_is_not_clobbered() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            None,
            BTreeMap::new(),
            Some(json!({"payload": "{{ messages }}"})),
            None,
        );
        let req = request_with_var("messages", "[hand-rolled json]");
        let canonical = p.canonical_request(&req).unwrap();
        assert_eq!(canonical["body"]["payload"], json!("[hand-rolled json]"));
    }

    /// A `template:` prompt reaches `{{ messages }}` as the single user turn it
    /// becomes on the wire — the same shape `openai::to_messages` produces.
    #[test]
    fn a_text_prompt_is_a_single_user_turn_in_messages() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            None,
            BTreeMap::new(),
            Some(json!({"transcript": "{{ messages | tojson }}"})),
            None,
        );
        let req = request_with_var("doc", "hi");
        let canonical = p.canonical_request(&req).unwrap();
        let transcript: Json =
            serde_json::from_str(canonical["body"]["transcript"].as_str().unwrap()).unwrap();
        assert_eq!(
            transcript,
            json!([{"role": "user", "content": "summarize this"}])
        );
    }

    /// The redaction pass this provider's preview was withheld for. `env` is the
    /// documented way to authenticate a black-box system under test, and both
    /// documents are persisted — the canonical request into every cache entry,
    /// the preview into a `--share`d run document — so the credential must be
    /// replaced by the placeholder that names it, everywhere it appears.
    #[test]
    fn a_call_time_credential_is_replaced_by_its_placeholder_in_both_documents() {
        std::env::set_var("SUT_TOKEN", "SENTINEL-SECRET");
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1?key={{ env.SUT_TOKEN }}",
            None,
            BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer {{ env.SUT_TOKEN }}".to_string(),
            )]),
            Some(json!({"api_key": "{{ env.SUT_TOKEN }}", "q": "{{ doc }}"})),
            None,
        );
        let req = request_with_var("doc", "hi");

        let canonical = p.canonical_request(&req).unwrap();
        let preview = p.request_preview(&req).unwrap();
        for document in [&canonical, &preview] {
            let text = crate::cache::canonical_json(document);
            assert!(!text.contains("SENTINEL-SECRET"), "{text}");
            assert!(text.contains("${env:SUT_TOKEN}"), "{text}");
            assert_eq!(document["body"]["q"], json!("hi"), "vars render for real");
        }
        // Headers are digested rather than published in the keyed document, and
        // absent from the previewed one: a header is where a pasted-literal
        // secret would sit, and entries travel to shared stores.
        assert!(canonical.get("headers").is_none());
        assert!(canonical["headers_digest"]
            .as_str()
            .is_some_and(|d| d.starts_with("blake3:")));
        assert!(preview.get("headers_digest").is_none());
        std::env::remove_var("SUT_TOKEN");
    }

    /// A literal header selects what answers and must key; a credential header
    /// must not, or a shared cache is partitioned by whose token was used.
    #[test]
    fn a_literal_header_keys_where_a_credential_header_does_not() {
        let with_header = |name: &str, value: &str| {
            HttpProvider::new(
                "h",
                "https://sut.example/v1",
                None,
                BTreeMap::from([(name.to_string(), value.to_string())]),
                None,
                None,
            )
            .canonical_request(&request_with_var("doc", "hi"))
            .unwrap()
        };
        assert_ne!(
            with_header("X-Model", "a")["headers_digest"],
            with_header("X-Model", "b")["headers_digest"]
        );

        let under_token = |token: &str| {
            std::env::set_var("HTTP_CANONICAL_TOKEN", token);
            with_header("Authorization", "Bearer {{ env.HTTP_CANONICAL_TOKEN }}")
        };
        assert_eq!(under_token("token-a"), under_token("token-b"));
        std::env::remove_var("HTTP_CANONICAL_TOKEN");
    }

    /// Only `env` is placeholdered. A header reading a case var renders for
    /// real, so two cases asking different questions do not share an entry.
    #[test]
    fn a_header_that_reads_a_case_var_separates_the_requests() {
        let for_region = |region: &str| {
            HttpProvider::new(
                "h",
                "https://sut.example/v1",
                None,
                BTreeMap::from([("X-Region".to_string(), "{{ region }}".to_string())]),
                None,
                None,
            )
            .canonical_request(&request_with_var("region", region))
            .unwrap()
        };
        assert_ne!(
            for_region("eu")["headers_digest"],
            for_region("us")["headers_digest"]
        );
    }

    /// Both optional members are inserted only when the provider declares them,
    /// the same discipline `fingerprint` follows.
    #[test]
    fn the_canonical_envelope_omits_what_was_never_declared() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            Some(HttpMethod::Get),
            BTreeMap::new(),
            None,
            None,
        );
        assert_eq!(
            crate::cache::canonical_json(
                &p.canonical_request(&request_with_var("x", "y")).unwrap()
            ),
            r#"{"method":"GET","transport":"http","url":"https://sut.example/v1"}"#
        );
    }

    /// `output_expr` keys, even though it is never sent.
    ///
    /// The entry stores the *projected* output, so two providers reading one
    /// endpoint's response differently are not asking the same question — and
    /// the second must not be served the first's answer.
    #[test]
    fn output_expr_separates_providers_that_send_the_same_request() {
        let projecting = |expr: &str| {
            HttpProvider::new(
                "h",
                "https://sut.example/v1",
                None,
                BTreeMap::new(),
                Some(json!({"q": "{{ doc }}"})),
                Some(expr.to_string()),
            )
            .canonical_request(&request_with_var("doc", "hi"))
            .unwrap()
        };
        let (a, b) = (
            projecting("response.json.reply"),
            projecting("response.json.answer"),
        );
        assert_eq!(a["body"], b["body"], "the same request goes on the wire");
        assert_ne!(a, b, "…but the projections are different questions");
        assert_eq!(a["output_expr"], json!("response.json.reply"));
    }

    /// …under the same conditional-insert discipline as `body` and
    /// `headers_digest`: a provider that declares no expression keys exactly as
    /// it did before the member existed. An unconditional `null` would re-key
    /// every `http` provider that projects nothing.
    #[test]
    fn a_provider_that_declares_no_output_expr_keys_as_it_did_before() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            None,
            BTreeMap::new(),
            Some(json!({"q": "{{ doc }}"})),
            None,
        );
        assert_eq!(
            crate::cache::canonical_json(
                &p.canonical_request(&request_with_var("doc", "hi")).unwrap()
            ),
            r#"{"body":{"q":"hi"},"method":"POST","transport":"http","url":"https://sut.example/v1"}"#
        );
    }

    /// The preview is what goes on the wire, and `output_expr` never does — so
    /// the keyed document carries it and the previewed one must not.
    #[test]
    fn output_expr_is_keyed_but_never_previewed() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/v1",
            None,
            BTreeMap::new(),
            None,
            Some("response.json.reply".into()),
        );
        let req = request_with_var("doc", "hi");
        assert!(p
            .canonical_request(&req)
            .unwrap()
            .get("output_expr")
            .is_some());
        assert!(p
            .request_preview(&req)
            .unwrap()
            .get("output_expr")
            .is_none());
    }

    /// A request that cannot be rendered has no identity to key on. The live
    /// call surfaces the same failure as `RENDER_FAILED`.
    #[test]
    fn a_request_that_cannot_render_is_uncacheable() {
        let p = HttpProvider::new(
            "h",
            "https://sut.example/{{ region }}",
            None,
            BTreeMap::new(),
            None,
            None,
        );
        let req = request_with_var("unrelated", "y");
        assert!(p.canonical_request(&req).is_none());
        assert!(p.request_preview(&req).is_none());
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

    /// The deliberate asymmetry between the two prompt views, pinned so a
    /// future contributor cannot "fix" the `{{ prompt }}` half by adding a
    /// textual rendering of a tool call — which is exactly what invites a
    /// tool-eager model to imitate that syntax as text.
    #[test]
    fn a_tool_bearing_transcript_is_in_messages_and_not_in_prompt() {
        let mut req = request_with_var("doc", "hi");
        req.prompt = Some(RenderedPrompt::Messages(vec![
            crate::types::ChatMessage::text(crate::types::ChatRole::User, "where is 1042?"),
            crate::types::ChatMessage {
                role: crate::types::ChatRole::Assistant,
                content: crate::types::MessageContent::Text(String::new()),
                tool_calls: vec![crate::result::ToolCall {
                    id: None,
                    name: "lookup_order".into(),
                    arguments: json!({"order_id": 1042}),
                }],
                tool_call_id: None,
            },
        ]));
        let ctx = render_context(&req, &json!({}));

        let prompt = ctx["prompt"].as_str().expect("prompt is a string");
        assert!(
            !prompt.contains("lookup_order"),
            "the flattened view carries prose only, got: {prompt}"
        );
        assert!(prompt.contains("where is 1042?"));

        let messages = &ctx["messages"];
        assert_eq!(messages[1]["tool_calls"][0]["name"], "lookup_order");
        assert_eq!(messages[1]["tool_calls"][0]["arguments"]["order_id"], 1042);
    }
}
