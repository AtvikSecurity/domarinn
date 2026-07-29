//! Protocol-era detection, `_meta` validation, and result decoration.
//!
//! This module owns the **entire** difference between the two protocol eras.
//! Payloads are built era-blind everywhere else and passed through
//! [`decorate`] exactly once on the way out; an `if era == Modern` anywhere in
//! a tool or prompt handler is a bug.

use axum::http::{HeaderMap, StatusCode};
use serde_json::{json, Value};

use super::jsonrpc::{
    self, LEGACY_VERSIONS, META_CLIENT_CAPABILITIES, META_PROTOCOL_VERSION, META_SERVER_INFO,
    MODERN_VERSION, SUPPORTED_VERSIONS,
};

/// The `MCP-Protocol-Version` request header.
pub const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";

/// A refusal to process a message, carrying both the HTTP status the spec
/// mandates and the JSON-RPC error to put in the body.
#[derive(Debug, Clone)]
pub struct Rejection {
    pub status: StatusCode,
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl Rejection {
    pub fn new(status: StatusCode, code: i64, message: impl Into<String>) -> Rejection {
        Rejection {
            status,
            code,
            message: message.into(),
            data: None,
        }
    }

    /// A header/body disagreement: HTTP 400 + `-32020`.
    pub fn header_mismatch(message: impl Into<String>) -> Rejection {
        Rejection::new(StatusCode::BAD_REQUEST, jsonrpc::HEADER_MISMATCH, message)
    }

    pub fn with_data(mut self, data: Value) -> Rejection {
        self.data = Some(data);
        self
    }
}

/// Which protocol era a request is speaking.
///
/// `Modern` is `2026-07-28` and later: stateless, header-validated, results
/// carry `resultType`. `Legacy` is `2025-11-25` and earlier: an `initialize`
/// handshake, no header mirroring, no `resultType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Era {
    Modern,
    Legacy,
}

/// Classify a request.
///
/// Keyed on the protocol version *value*, never on header presence:
/// `MCP-Protocol-Version` was introduced in `2025-06-18`, so legacy clients
/// send it too and branching on presence gives wrong answers.
pub fn detect(headers: &HeaderMap, method: &str) -> Result<Era, Rejection> {
    // The handshake methods only exist in the legacy era, whatever the header
    // claims — a client sending `initialize` is by definition pre-modern.
    if method == "initialize" || method == "notifications/initialized" {
        return Ok(Era::Legacy);
    }

    let Some(version) = header_str(headers, PROTOCOL_VERSION_HEADER) else {
        // The spec permits rejecting a header-less request, but we gain
        // nothing by it: we are not an intermediary routing on unvalidated
        // headers, and the body is validated regardless. Being lenient keeps
        // pre-2025-06-18 clients and hand-rolled `curl` calls working.
        return Ok(Era::Legacy);
    };

    if version == MODERN_VERSION {
        return Ok(Era::Modern);
    }
    if LEGACY_VERSIONS.contains(&version) {
        return Ok(Era::Legacy);
    }
    Err(Rejection::new(
        StatusCode::BAD_REQUEST,
        jsonrpc::UNSUPPORTED_PROTOCOL_VERSION,
        "Unsupported protocol version",
    )
    .with_data(json!({ "supported": SUPPORTED_VERSIONS, "requested": version })))
}

/// Validate the per-request `_meta` fields the modern era requires.
///
/// `protocolVersion` and `clientCapabilities` are both mandatory; a request
/// missing either is malformed and gets `-32602` at HTTP 400. We never demand
/// a *specific* client capability, so `-32021` can never fire here.
pub fn validate_meta(incoming: &jsonrpc::Incoming) -> Result<(), Rejection> {
    let meta = incoming.meta();
    let invalid = |message: &str| {
        Rejection::new(
            StatusCode::BAD_REQUEST,
            jsonrpc::INVALID_PARAMS,
            message.to_string(),
        )
    };

    if meta
        .get(META_PROTOCOL_VERSION)
        .and_then(Value::as_str)
        .is_none()
    {
        return Err(invalid(
            "params._meta must carry io.modelcontextprotocol/protocolVersion",
        ));
    }
    if !meta
        .get(META_CLIENT_CAPABILITIES)
        .is_some_and(Value::is_object)
    {
        return Err(invalid(
            "params._meta must carry io.modelcontextprotocol/clientCapabilities",
        ));
    }
    Ok(())
}

/// The protocol version declared in `params._meta`, when present.
pub fn meta_version(incoming: &jsonrpc::Incoming) -> Option<&str> {
    incoming
        .meta()
        .get(META_PROTOCOL_VERSION)
        .and_then(Value::as_str)
}

/// Caching hints for a cacheable result. The spec makes these a **MUST** on
/// `server/discover`, `tools/list`, and `prompts/list`.
#[derive(Debug, Clone, Copy)]
pub struct CacheHint {
    pub ttl_ms: u64,
}

impl CacheHint {
    /// Both catalogs this server serves are static and identical for every
    /// caller, so `public` is accurate. Anything varying by identity would
    /// have to be `private`.
    const SCOPE: &'static str = "public";
}

/// Apply every era-dependent field to an outgoing result, in one place.
///
/// Legacy results are returned untouched: `resultType` and the caching hints
/// did not exist before `2026-07-28`, and emitting them would confuse a
/// client that validates against the older schema.
pub fn decorate(era: Era, result: &mut Value, cache: Option<CacheHint>) {
    if era != Era::Modern {
        return;
    }
    let Some(obj) = result.as_object_mut() else {
        return;
    };
    obj.insert("resultType".to_string(), json!("complete"));
    if let Some(cache) = cache {
        obj.insert("ttlMs".to_string(), json!(cache.ttl_ms));
        obj.insert("cacheScope".to_string(), json!(CacheHint::SCOPE));
    }
    // Servers SHOULD identify themselves on every modern result.
    let meta = obj.entry("_meta".to_string()).or_insert_with(|| json!({}));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(META_SERVER_INFO.to_string(), server_info());
    }
}

/// This server's self-reported identity. Display/logging only — the spec is
/// explicit that clients must not make security decisions on it.
pub fn server_info() -> Value {
    json!({ "name": "domarinn", "version": env!("CARGO_PKG_VERSION") })
}

/// Read a header as a string, ignoring non-ASCII values.
pub fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn era_truth_table() {
        let modern = headers(&[(PROTOCOL_VERSION_HEADER, MODERN_VERSION)]);
        assert_eq!(detect(&modern, "tools/list").unwrap(), Era::Modern);

        for legacy in LEGACY_VERSIONS {
            let h = headers(&[(PROTOCOL_VERSION_HEADER, legacy)]);
            assert_eq!(detect(&h, "tools/list").unwrap(), Era::Legacy, "{legacy}");
        }

        // `initialize` is legacy even when the header claims otherwise.
        assert_eq!(detect(&modern, "initialize").unwrap(), Era::Legacy);
        assert_eq!(
            detect(&modern, "notifications/initialized").unwrap(),
            Era::Legacy
        );

        // Absent header: lenient, deliberately.
        assert_eq!(
            detect(&HeaderMap::new(), "tools/list").unwrap(),
            Era::Legacy
        );
    }

    #[test]
    fn an_unknown_version_is_rejected_with_the_supported_list() {
        let h = headers(&[(PROTOCOL_VERSION_HEADER, "2099-01-01")]);
        let err = detect(&h, "tools/list").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, jsonrpc::UNSUPPORTED_PROTOCOL_VERSION);
        let data = err.data.unwrap();
        assert_eq!(data["requested"], "2099-01-01");
        assert_eq!(data["supported"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn decorate_is_a_no_op_for_legacy() {
        let mut result = json!({ "tools": [] });
        decorate(Era::Legacy, &mut result, Some(CacheHint { ttl_ms: 1 }));
        assert_eq!(result, json!({ "tools": [] }));
    }

    #[test]
    fn decorate_adds_result_type_cache_hints_and_server_info() {
        let mut result = json!({ "tools": [] });
        decorate(
            Era::Modern,
            &mut result,
            Some(CacheHint { ttl_ms: 300_000 }),
        );
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["ttlMs"], 300_000);
        assert_eq!(result["cacheScope"], "public");
        assert_eq!(result["_meta"][META_SERVER_INFO]["name"], "domarinn");
    }

    #[test]
    fn decorate_omits_cache_hints_when_not_cacheable() {
        let mut result = json!({ "content": [] });
        decorate(Era::Modern, &mut result, None);
        assert_eq!(result["resultType"], "complete");
        assert!(result.get("ttlMs").is_none());
        assert!(result.get("cacheScope").is_none());
    }

    #[test]
    fn meta_validation_requires_both_mandatory_fields() {
        let ok = jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": {
                META_PROTOCOL_VERSION: MODERN_VERSION,
                META_CLIENT_CAPABILITIES: {}
            } }
        }))
        .unwrap();
        assert!(validate_meta(&ok).is_ok());

        let no_caps = jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": { META_PROTOCOL_VERSION: MODERN_VERSION } }
        }))
        .unwrap();
        assert_eq!(
            validate_meta(&no_caps).unwrap_err().code,
            jsonrpc::INVALID_PARAMS
        );

        let no_version = jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": { META_CLIENT_CAPABILITIES: {} } }
        }))
        .unwrap();
        assert_eq!(
            validate_meta(&no_version).unwrap_err().code,
            jsonrpc::INVALID_PARAMS
        );
    }
}
