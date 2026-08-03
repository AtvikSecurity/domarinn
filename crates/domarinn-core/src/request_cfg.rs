//! Resolving a [`RequestCfg`] into the request a vendor provider actually sends.
//!
//! One module so there is one place header logic lives. Before this there were
//! four: `anthropic`, `openai`, `embeddings`, and the `llm-rubric` judge each
//! hand-rolled their own, and two of them carried their own copy of the
//! `anthropic-version` literal.
//!
//! # The double render
//!
//! [`resolve`] renders every template **twice** — once against the real
//! environment, once against [`crate::render::env_placeholder_object`] — and
//! keeps both. The first is what goes on the wire; the second is what the cache
//! keys and stores. They come out of one function with `env` as a parameter, the
//! same shape [`crate::http_provider`] uses, so the two documents cannot drift:
//! the templates, the order, and the errors are shared, and the environment is
//! the only axis that differs.
//!
//! Without it, `authorization: "Bearer {{ env.TOKEN }}"` would either key on the
//! token — partitioning a shared cache by who ran it — or write it verbatim into
//! every cache entry.
//!
//! # Rendered once, at construction
//!
//! These templates see `env` and nothing else, so their output cannot vary
//! between two cases of the same run. Rendering per call would cost a
//! `TemplateEngine` per request to produce the same bytes. `type: http` is the
//! provider whose templates *do* see case vars.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::config_request::{AuthMode, RequestCfg};
use crate::template::TemplateEngine;
use crate::val::Val;

/// Headers merged into every HTTP provider's request, as a JSON object.
///
/// The escape hatch for an environment that must add a header to traffic it does
/// not own the suites for — an egress proxy, a cost-attribution tag. Values are
/// templates, rendered exactly like a suite's own headers, so a credential
/// written `{{ env.X }}` here is redacted from the cache the same way.
pub const GLOBAL_HEADERS_ENV: &str = "DOMARINN_PROVIDER_HEADERS";

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error("rendering `request.{field}`: {source}")]
    Render {
        field: String,
        #[source]
        source: crate::template::TemplateError,
    },
    #[error(
        "`{GLOBAL_HEADERS_ENV}` is not valid JSON: {source}. Expected an object of \
         header name to value, e.g. '{{\"x-egress\":\"gw-7\"}}'"
    )]
    GlobalHeadersJson {
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "`{GLOBAL_HEADERS_ENV}` must be a JSON object of header name to string value, \
         but {problem}"
    )]
    GlobalHeadersShape { problem: String },
}

/// A [`RequestCfg`] rendered against both environments, ready to send and to key.
#[derive(Debug, Clone)]
pub struct ResolvedRequest {
    /// Rendered against the real environment. What goes on the wire.
    wire: Rendered,
    /// Rendered against placeholder env. What the cache keys and stores.
    keyed: Rendered,
    /// Digest of `keyed.headers`, or `None` when no header was declared.
    headers_digest: Option<String>,
    auth: AuthMode,
}

/// One rendering of a [`RequestCfg`] against a particular environment.
#[derive(Debug, Clone, Default)]
struct Rendered {
    /// The suite's and the environment's headers, merged. Excludes the vendor
    /// defaults and the credential, which are applied per call.
    headers: BTreeMap<String, String>,
    /// Path and query string, e.g. `/v1/messages` or `/x?api-version=2024-02-01`.
    path: String,
    body: Option<Json>,
}

impl ResolvedRequest {
    /// The default for a provider with no `request:` block: the vendor's own path
    /// and auth scheme, no headers, no overlay.
    ///
    /// Deliberately not `Default` — there is no meaningful default path or auth
    /// mode in the abstract, and a provider that forgot to pass its own would
    /// silently send requests to `/`.
    pub fn vendor_default(default_path: &str, auth: AuthMode) -> Self {
        let rendered = Rendered {
            headers: BTreeMap::new(),
            path: default_path.to_string(),
            body: None,
        };
        ResolvedRequest {
            wire: rendered.clone(),
            keyed: rendered,
            headers_digest: None,
            auth,
        }
    }

    /// How this provider presents its credential.
    pub fn auth(&self) -> AuthMode {
        self.auth
    }

    /// The path and query to append to `base_url`, as sent.
    pub fn path(&self) -> &str {
        &self.wire.path
    }

    /// The path and query as the cache keys it — identical to [`Self::path`]
    /// unless a template read the environment at call time.
    pub fn keyed_path(&self) -> &str {
        &self.keyed.path
    }

    /// A digest of the declared headers, or `None` when none were declared.
    ///
    /// `None` rather than the digest of an empty map so a provider that declares
    /// no header keeps every cache entry it had before this field existed.
    pub fn headers_digest(&self) -> Option<&str> {
        self.headers_digest.as_deref()
    }

    /// The body overlay as the cache keys it. Applied to the body the provider
    /// built, after it built it.
    pub fn keyed_body(&self) -> Option<&Json> {
        self.keyed.body.as_ref()
    }

    /// Merge the overlay into `body`, using the values that go on the wire.
    pub fn apply_body(&self, body: &mut Json) {
        if let Some(overlay) = &self.wire.body {
            deep_merge(body, overlay);
        }
    }

    /// Merge the overlay into `body` using the *keyed* values, for
    /// [`crate::provider::Provider::canonical_request`].
    pub fn apply_keyed_body(&self, body: &mut Json) {
        if let Some(overlay) = &self.keyed.body {
            deep_merge(body, overlay);
        }
    }

    /// Every header for one call: the vendor's own, then the credential per
    /// [`Self::auth`], then the declared ones — which override both.
    ///
    /// Later wins, matched case-insensitively as HTTP requires, and the spelling
    /// the author wrote is what survives. `credential` is `None` for
    /// [`AuthMode::None`], where no credential is read at all.
    pub fn call_headers(
        &self,
        vendor_defaults: &[(&str, &str)],
        credential: Option<&str>,
    ) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        for (name, value) in vendor_defaults {
            insert_ci(&mut headers, name, value.to_string());
        }
        if let Some(credential) = credential {
            if let Some((name, value)) = self.auth.header(credential) {
                insert_ci(&mut headers, name, value);
            }
        }
        for (name, value) in &self.wire.headers {
            insert_ci(&mut headers, name, value.clone());
        }
        headers
    }
}

/// Resolve `cfg` plus any [`GLOBAL_HEADERS_ENV`] injection into a sendable and
/// keyable request.
///
/// `default_path` and `default_auth` are the vendor's own, used when `cfg` does
/// not override them. `provider_id` only appears in warnings.
pub fn resolve(
    provider_id: &str,
    cfg: Option<&RequestCfg>,
    default_path: &str,
    default_auth: AuthMode,
) -> Result<ResolvedRequest, RequestError> {
    resolve_with_global(
        provider_id,
        cfg,
        default_path,
        default_auth,
        global_headers()?,
    )
}

/// [`resolve`], with the [`GLOBAL_HEADERS_ENV`] injection supplied rather than
/// read.
///
/// Separate because that variable has a fixed name: a test that exported it
/// would change the request every *other* test in the binary builds. Here the
/// injection is an argument, so its behaviour is testable without a process-wide
/// side effect.
pub fn resolve_with_global(
    provider_id: &str,
    cfg: Option<&RequestCfg>,
    default_path: &str,
    default_auth: AuthMode,
    global: BTreeMap<String, String>,
) -> Result<ResolvedRequest, RequestError> {
    if cfg.is_none_or(RequestCfg::is_empty) && global.is_empty() {
        return Ok(ResolvedRequest::vendor_default(default_path, default_auth));
    }

    let empty = RequestCfg::default();
    let cfg = cfg.unwrap_or(&empty);

    // Both maps' values are templates, so both are warned about and both are
    // rendered. The suite's win by name, as the more specific layer.
    let mut declared: BTreeMap<String, String> = global;
    for (name, template) in &cfg.headers {
        insert_ci(&mut declared, name, template.clone());
    }

    warn_on_runtime_env(provider_id, runtime_env_sources(cfg, &declared));
    warn_on_redundant_auth(provider_id, cfg, &declared, default_auth);

    let wire = render(cfg, &declared, default_path, &crate::render::env_object())?;
    let keyed = render(
        cfg,
        &declared,
        default_path,
        &crate::render::env_placeholder_object(),
    )?;

    Ok(ResolvedRequest {
        headers_digest: headers_digest(&keyed.headers),
        wire,
        keyed,
        auth: cfg.auth.unwrap_or(default_auth),
    })
}

/// Render one [`RequestCfg`] against one environment.
///
/// `env` is a parameter rather than read here because it is the only axis on
/// which the sent request and the keyed one differ. Everything else — the
/// templates, the order they render in, the error each failure produces — is
/// shared, so the two cannot drift.
fn render(
    cfg: &RequestCfg,
    declared_headers: &BTreeMap<String, String>,
    default_path: &str,
    env: &Json,
) -> Result<Rendered, RequestError> {
    let engine = TemplateEngine::new();
    let ctx = serde_json::json!({ "env": env });

    let render_one = |field: &str, template: &str| -> Result<String, RequestError> {
        engine
            .render_str(template, &ctx)
            .map_err(|source| RequestError::Render {
                field: field.to_string(),
                source,
            })
    };

    let mut headers = BTreeMap::new();
    for (name, template) in declared_headers {
        headers.insert(
            name.clone(),
            render_one(&format!("headers.{name}"), template)?,
        );
    }

    let path = match &cfg.path {
        Some(path) => render_one("path", path)?,
        None => default_path.to_string(),
    };

    // Sorted by name, because `query` is a BTreeMap: two suites writing the same
    // pairs in a different order produce one cache entry rather than two.
    let mut query = Vec::new();
    for (name, template) in &cfg.query {
        let value = render_one(&format!("query.{name}"), template)?;
        query.push(format!("{}={}", urlencode(name), urlencode(&value)));
    }
    let path = if query.is_empty() {
        path
    } else {
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{path}{sep}{}", query.join("&"))
    };

    let body =
        match &cfg.body {
            Some(body) => Some(engine.render_val(&Val::Tpl(body.clone()), &ctx).map_err(
                |source| RequestError::Render {
                    field: "body".to_string(),
                    source,
                },
            )?),
            None => None,
        };

    Ok(Rendered {
        headers,
        path,
        body,
    })
}

/// Parse [`GLOBAL_HEADERS_ENV`] into a header map.
///
/// A hard error rather than a warning-and-ignore: someone exported this because
/// their gateway requires it, and a silently dropped egress header fails at the
/// far end with no local evidence.
pub fn global_headers() -> Result<BTreeMap<String, String>, RequestError> {
    let Some(raw) = std::env::var(GLOBAL_HEADERS_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return Ok(BTreeMap::new());
    };
    parse_global_headers(&raw)
}

/// Parse the [`GLOBAL_HEADERS_ENV`] payload. See [`global_headers`].
pub fn parse_global_headers(raw: &str) -> Result<BTreeMap<String, String>, RequestError> {
    let parsed: Json =
        serde_json::from_str(raw).map_err(|source| RequestError::GlobalHeadersJson { source })?;
    let Some(object) = parsed.as_object() else {
        return Err(RequestError::GlobalHeadersShape {
            problem: format!("it is a {}", json_kind(&parsed)),
        });
    };
    let mut headers = BTreeMap::new();
    for (name, value) in object {
        let Some(value) = value.as_str() else {
            return Err(RequestError::GlobalHeadersShape {
                problem: format!("`{name}` is a {}", json_kind(value)),
            });
        };
        headers.insert(name.clone(), value.to_string());
    }
    Ok(headers)
}

fn json_kind(value: &Json) -> &'static str {
    match value {
        Json::Null => "null",
        Json::Bool(_) => "boolean",
        Json::Number(_) => "number",
        Json::String(_) => "string",
        Json::Array(_) => "array",
        Json::Object(_) => "object",
    }
}

/// A digest of the declared headers, or `None` when none are declared.
///
/// `None` rather than the digest of an empty map so a provider that declares no
/// header keeps the key — and therefore every cache entry — it had before this
/// member existed.
///
/// A digest rather than the values themselves because both callers persist their
/// output into every cache entry, and a header is where a literal secret sits.
/// The map passed here is always the one rendered against placeholder `env`, so
/// `Authorization: Bearer {{ env.TOKEN }}` renders to the same `${env:TOKEN}`
/// for two teammates holding different tokens and does not partition their
/// shared cache, while `X-Model: gpt-5` and `X-Model: claude-opus-5` separate.
pub fn headers_digest(headers: &BTreeMap<String, String>) -> Option<String> {
    if headers.is_empty() {
        return None;
    }
    let canonical = crate::cache::canonical_json(&serde_json::json!(headers));
    Some(format!(
        "blake3:{}",
        blake3::hash(canonical.as_bytes()).to_hex()
    ))
}

/// Warn when a provider's templates read the environment at call time.
///
/// `{{ env.VAR }}` is rendered after the cache key is decided, so its *value*
/// never reaches the key — only the template text does. That is correct for a
/// credential and wrong for anything else: `path: "/{{ env.MODEL }}/chat"` gives
/// two models one cache key, and the second silently replays the first's
/// answers.
///
/// domarinn cannot tell which is which. A credential must not separate two
/// teammates' entries; a model selector must separate them. So it says what it
/// sees and names the alternative: `${env:VAR}` is resolved at load time, before
/// the provider is built, so the substituted value is in the key.
pub fn warn_on_runtime_env(id: &str, sources: Vec<String>) {
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

/// Every template string a [`RequestCfg`] contains, for the runtime-env warning.
fn runtime_env_sources(
    cfg: &RequestCfg,
    declared_headers: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut sources: Vec<String> = declared_headers.values().cloned().collect();
    sources.extend(cfg.path.iter().cloned());
    sources.extend(cfg.query.values().cloned());
    if let Some(body) = &cfg.body {
        sources.push(body.to_string());
    }
    sources
}

/// Warn when a declared header duplicates the credential the auth mode sends.
///
/// Both would be built, the declared one wins, and the `api_key_env` credential
/// is read and then discarded — which reads as "my key is being ignored" from
/// the outside. Naming it is cheaper than debugging it.
fn warn_on_redundant_auth(
    id: &str,
    cfg: &RequestCfg,
    declared: &BTreeMap<String, String>,
    default_auth: AuthMode,
) {
    let auth = cfg.auth.unwrap_or(default_auth);
    let Some((sent, _)) = auth.header("") else {
        return;
    };
    let Some(declared_name) = declared.keys().find(|name| name.eq_ignore_ascii_case(sent)) else {
        return;
    };
    tracing::warn!(
        provider = %id,
        header = %declared_name,
        "this provider declares a `{declared_name}` header and also sends one for \
         `auth: {auth:?}`. The declared header wins and the credential from \
         `api_key_env` is read and discarded. Set `auth: none` to make that explicit."
    );
}

/// Variable names reached through `env.` in a minijinja template.
///
/// Recognises `env.NAME` and `env['NAME']` / `env["NAME"]`, the two spellings
/// the docs use. Anything else that touches `env` — a computed lookup — yields
/// the placeholder `*`, so the warning still fires without inventing a name.
pub fn runtime_env_refs(template: &str) -> Vec<String> {
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

/// Insert `name`, replacing any existing entry that differs only in case.
///
/// HTTP header names are case-insensitive, so `Authorization` and
/// `authorization` are one header; a plain `BTreeMap` insert would send both and
/// let the server pick. The spelling of the *last* writer survives, matching the
/// precedence the values follow.
fn insert_ci(headers: &mut BTreeMap<String, String>, name: &str, value: String) {
    if let Some(existing) = headers
        .keys()
        .find(|k| k.eq_ignore_ascii_case(name))
        .cloned()
    {
        headers.remove(&existing);
    }
    headers.insert(name.to_string(), value);
}

/// Merge `overlay` into `base`: objects merge key-by-key, anything else replaces.
fn deep_merge(base: &mut Json, overlay: &Json) {
    match (base, overlay) {
        (Json::Object(base), Json::Object(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, overlay) => *base = overlay.clone(),
    }
}

/// Percent-encode a query component.
///
/// Hand-rolled rather than pulling a dependency for it: the unreserved set from
/// RFC 3986 §2.3 passes through and everything else is escaped, which is correct
/// for both a name and a value.
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg_from(yaml: &str) -> RequestCfg {
        serde_yaml_ng::from_str(yaml).expect("test fixture parses")
    }

    fn resolve_bare(cfg: &RequestCfg, default_path: &str, auth: AuthMode) -> ResolvedRequest {
        resolve_with_global("p", Some(cfg), default_path, auth, BTreeMap::new())
            .expect("test fixture resolves")
    }

    /// The whole backward-compatibility property in one assertion: a provider
    /// that declares nothing must key exactly as it did before this type
    /// existed, and a `headers_digest` of an empty map instead of `None` would
    /// silently re-key every cache entry in every store.
    #[test]
    fn an_absent_block_adds_no_digest_and_keeps_the_vendor_path() {
        let resolved =
            resolve_with_global("p", None, "/v1/messages", AuthMode::ApiKey, BTreeMap::new())
                .unwrap();
        assert_eq!(resolved.path(), "/v1/messages");
        assert_eq!(resolved.keyed_path(), "/v1/messages");
        assert_eq!(resolved.headers_digest(), None);
        assert_eq!(resolved.auth(), AuthMode::ApiKey);
        assert!(resolved.keyed_body().is_none());
    }

    /// An empty-but-present block is the same as an absent one. Written out
    /// because `request: {}` is what a suite left behind after deleting the last
    /// override, and it must not cost that suite its cache.
    #[test]
    fn an_empty_block_is_the_same_as_an_absent_one() {
        let resolved = resolve_bare(&RequestCfg::default(), "/v1/messages", AuthMode::ApiKey);
        assert_eq!(resolved.headers_digest(), None);
        assert_eq!(resolved.path(), "/v1/messages");
    }

    /// The redaction property. Two teammates holding different tokens ask the
    /// same question, so their entries must share a key — while the real token
    /// still goes on the wire.
    #[test]
    fn a_credential_header_sends_its_value_and_keys_its_placeholder() {
        std::env::set_var("REQCFG_TOKEN_A", "SENTINEL-ONE");
        let cfg = cfg_from("headers:\n  authorization: \"Bearer {{ env.REQCFG_TOKEN_A }}\"\n");
        let first = resolve_bare(&cfg, "/v1/messages", AuthMode::None);
        let sent = first.call_headers(&[], None);
        assert_eq!(sent["authorization"], "Bearer SENTINEL-ONE");

        std::env::set_var("REQCFG_TOKEN_A", "SENTINEL-TWO");
        let second = resolve_bare(&cfg, "/v1/messages", AuthMode::None);
        assert_eq!(
            second.call_headers(&[], None)["authorization"],
            "Bearer SENTINEL-TWO",
            "the wire value follows the environment"
        );
        assert_eq!(
            first.headers_digest(),
            second.headers_digest(),
            "but the key does not, or a shared cache is partitioned by whose token was used"
        );
    }

    /// The other half of that trade: a header that selects *what* is being asked
    /// must separate two cache entries, or the second value replays the first's
    /// answers.
    #[test]
    fn a_literal_header_value_separates_two_providers() {
        let fast = resolve_bare(
            &cfg_from("headers:\n  x-tier: fast\n"),
            "/p",
            AuthMode::None,
        );
        let slow = resolve_bare(
            &cfg_from("headers:\n  x-tier: slow\n"),
            "/p",
            AuthMode::None,
        );
        assert_ne!(fast.headers_digest(), slow.headers_digest());
    }

    #[test]
    fn each_auth_mode_sends_its_own_header() {
        let bearer = resolve_bare(&cfg_from("auth: bearer\n"), "/p", AuthMode::ApiKey);
        assert_eq!(
            bearer.call_headers(&[], Some("sk-ant-oat-x"))["authorization"],
            "Bearer sk-ant-oat-x"
        );

        let api_key = resolve_bare(&cfg_from("auth: api_key\n"), "/p", AuthMode::Bearer);
        assert_eq!(api_key.call_headers(&[], Some("sk-1"))["x-api-key"], "sk-1");

        let none = resolve_bare(&cfg_from("auth: none\n"), "/p", AuthMode::ApiKey);
        assert!(
            none.call_headers(&[], None).is_empty(),
            "`auth: none` sends no credential header of its own"
        );
        assert!(!none.auth().needs_credential());
    }

    /// The vendor's own headers survive, and the author's override them. The
    /// case-insensitive match is what makes the override work at all: HTTP
    /// treats these as one header, so a plain map insert would send both and let
    /// the server choose.
    #[test]
    fn declared_headers_override_vendor_defaults_case_insensitively() {
        let cfg = cfg_from("headers:\n  Anthropic-Version: \"2099-01-01\"\n");
        let resolved = resolve_bare(&cfg, "/p", AuthMode::ApiKey);
        let sent = resolved.call_headers(&[("anthropic-version", "2023-06-01")], Some("k"));

        assert_eq!(sent["Anthropic-Version"], "2099-01-01");
        assert!(
            !sent.contains_key("anthropic-version"),
            "one header, not two: {sent:?}"
        );
        assert_eq!(sent["x-api-key"], "k", "the credential still goes out");
    }

    /// Precedence: the environment's injection is a default for suites that do
    /// not care, and a suite that named the header meant it.
    #[test]
    fn a_suite_header_beats_the_global_injection() {
        let global = BTreeMap::from([
            ("x-egress".to_string(), "gw-7".to_string()),
            ("x-tier".to_string(), "from-env".to_string()),
        ]);
        let cfg = cfg_from("headers:\n  x-tier: from-suite\n");
        let resolved = resolve_with_global("p", Some(&cfg), "/p", AuthMode::None, global).unwrap();
        let sent = resolved.call_headers(&[], None);

        assert_eq!(sent["x-tier"], "from-suite");
        assert_eq!(
            sent["x-egress"], "gw-7",
            "the uncontested one still applies"
        );
    }

    /// The user's decision: an injected header is request content, so it keys.
    /// Exporting the variable in CI and not locally therefore splits the cache,
    /// which is the documented cost of choosing this over treating it as
    /// transport.
    #[test]
    fn the_global_injection_participates_in_the_digest() {
        let without =
            resolve_with_global("p", None, "/p", AuthMode::None, BTreeMap::new()).unwrap();
        let with = resolve_with_global(
            "p",
            None,
            "/p",
            AuthMode::None,
            BTreeMap::from([("x-egress".to_string(), "gw-7".to_string())]),
        )
        .unwrap();

        assert_eq!(without.headers_digest(), None);
        assert!(with.headers_digest().is_some());
    }

    #[test]
    fn a_path_override_replaces_the_vendor_suffix() {
        let cfg = cfg_from("path: /openai/deployments/gpt4o/chat/completions\n");
        let resolved = resolve_bare(&cfg, "/chat/completions", AuthMode::Bearer);
        assert_eq!(
            resolved.path(),
            "/openai/deployments/gpt4o/chat/completions"
        );
    }

    /// Sorted and percent-encoded, so two suites writing the same pairs in a
    /// different order produce one cache entry rather than two.
    #[test]
    fn query_params_are_sorted_and_encoded() {
        let cfg = cfg_from("query:\n  zeta: \"1\"\n  alpha: \"a b&c\"\n");
        let resolved = resolve_bare(&cfg, "/chat", AuthMode::Bearer);
        assert_eq!(resolved.path(), "/chat?alpha=a%20b%26c&zeta=1");
    }

    #[test]
    fn a_query_joins_a_path_that_already_has_one() {
        let cfg = cfg_from("path: \"/chat?existing=1\"\nquery:\n  added: \"2\"\n");
        assert_eq!(
            resolve_bare(&cfg, "/chat", AuthMode::Bearer).path(),
            "/chat?existing=1&added=2"
        );
    }

    /// The reason this exists rather than `params`: `params` merges *first* and
    /// is then overwritten by `model`/`messages`/`system`, so those three are
    /// exactly what it cannot reach.
    #[test]
    fn the_body_overlay_merges_last_and_deeply() {
        let cfg = cfg_from("body:\n  system: injected\n  metadata:\n    tenant: acme\n");
        let resolved = resolve_bare(&cfg, "/p", AuthMode::Bearer);

        let mut body = json!({
            "model": "m",
            "system": "built by the provider",
            "metadata": {"run": "r1"},
        });
        resolved.apply_body(&mut body);

        assert_eq!(body["system"], json!("injected"), "the overlay wins");
        assert_eq!(body["model"], json!("m"), "untouched keys survive");
        assert_eq!(body["metadata"], json!({"run": "r1", "tenant": "acme"}));
    }

    #[test]
    fn a_malformed_global_injection_is_an_error_not_a_shrug() {
        assert!(matches!(
            parse_global_headers("not json"),
            Err(RequestError::GlobalHeadersJson { .. })
        ));
        assert!(matches!(
            parse_global_headers(r#"["x-a","x-b"]"#),
            Err(RequestError::GlobalHeadersShape { .. })
        ));
        assert!(matches!(
            parse_global_headers(r#"{"x-a": 7}"#),
            Err(RequestError::GlobalHeadersShape { .. })
        ));
        assert_eq!(
            parse_global_headers(r#"{"x-a":"1"}"#).unwrap(),
            BTreeMap::from([("x-a".to_string(), "1".to_string())])
        );
    }

    /// Strict undefined behaviour reaches here too: a typo'd variable is a loud
    /// build-time error, not a header quietly sent as the empty string.
    #[test]
    fn an_undefined_variable_fails_the_build() {
        let cfg = cfg_from("headers:\n  x-a: \"{{ env.REQCFG_DEFINITELY_UNSET }}\"\n");
        assert!(matches!(
            resolve_with_global("p", Some(&cfg), "/p", AuthMode::None, BTreeMap::new()),
            Err(RequestError::Render { .. })
        ));
    }
}
