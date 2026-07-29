//! JSON-RPC 2.0 envelope types and the MCP error-code allocation.
//!
//! Deliberately free of `axum` and [`crate::AppState`] so the whole envelope
//! layer is unit-testable in isolation.

use serde_json::{json, Value};

/// The protocol revision this server implements natively: stateless, no
/// `initialize` handshake, no session ids.
pub const MODERN_VERSION: &str = "2026-07-28";

/// Handshake-based revisions we still answer, newest first. Kept because no
/// shipping client speaks [`MODERN_VERSION`] yet; the spec blesses dual-era
/// servers explicitly.
pub const LEGACY_VERSIONS: [&str; 2] = ["2025-11-25", "2025-06-18"];

/// Every revision this server accepts, advertised by `server/discover` and in
/// `UnsupportedProtocolVersionError.data.supported`.
pub const SUPPORTED_VERSIONS: [&str; 3] = [MODERN_VERSION, "2025-11-25", "2025-06-18"];

/// The `_meta` key carrying a request's protocol version.
pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// The `_meta` key carrying the client's declared capabilities. Required on
/// modern requests even though this server requires no client capability.
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
/// The `_meta` key servers use to identify themselves on every modern result.
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

// -- Error codes -------------------------------------------------------------
//
// MCP partitions JSON-RPC's implementation-defined range: `-32000..=-32019` is
// a *legacy* sub-range new implementations must not use, and `-32020..=-32099`
// belongs to the specification. Codes we invent therefore live outside the
// whole reserved range (`-32768..=-32000`), per the spec's guidance.

/// Standard JSON-RPC: malformed JSON.
pub const PARSE_ERROR: i64 = -32700;
/// Standard JSON-RPC: not a valid Request object.
pub const INVALID_REQUEST: i64 = -32600;
/// Standard JSON-RPC: unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Standard JSON-RPC: bad params. Also MCP's code for a missing `_meta` field.
///
/// There is deliberately no `-32603` (internal error) here: a storage failure
/// inside a tool surfaces as `isError` in the result, not as a protocol
/// error, because the model can retry a narrower query but can do nothing
/// with a transport-level fault.
pub const INVALID_PARAMS: i64 = -32602;
/// MCP: HTTP headers disagree with the request body, or are missing.
pub const HEADER_MISMATCH: i64 = -32020;
/// MCP: the requested protocol version is not implemented here.
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;
/// domarinn: authentication required. Outside the JSON-RPC reserved range.
pub const AUTH_REQUIRED: i64 = -31001;
/// domarinn: too many calls. Outside the JSON-RPC reserved range.
pub const RATE_LIMITED: i64 = -31002;

/// A parsed client message. A [`Self::is_notification`] message carries no
/// `id` and gets `202 Accepted` with no body.
#[derive(Debug, Clone)]
pub struct Incoming {
    /// Absent for notifications. Never `null` — JSON-RPC allows it, MCP does not.
    pub id: Option<Value>,
    pub method: String,
    /// Always an object (possibly empty); a missing or non-object `params`
    /// normalizes to `{}` so callers never branch on shape.
    pub params: Value,
}

impl Incoming {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    /// `params.arguments`, or `{}` when absent — the object a tool's argument
    /// struct is deserialized from. Note `_meta` lives on `params`, never here.
    pub fn arguments(&self) -> Value {
        match self.params.get("arguments") {
            Some(v) if v.is_object() => v.clone(),
            _ => json!({}),
        }
    }

    /// `params._meta`, or `{}` when absent.
    pub fn meta(&self) -> &Value {
        self.params.get("_meta").unwrap_or(&Value::Null)
    }

    /// The value the `Mcp-Name` header must mirror: `params.name` for
    /// `tools/call` and `prompts/get`, `params.uri` for `resources/read`.
    pub fn name_field(&self) -> Option<&str> {
        self.params
            .get("name")
            .or_else(|| self.params.get("uri"))
            .and_then(Value::as_str)
    }
}

/// Parse a decoded JSON body into an [`Incoming`].
///
/// Returns the JSON-RPC error code to report on failure. The envelope is
/// parsed from a [`Value`] rather than a `deny_unknown_fields` struct so that
/// unknown top-level members and future `_meta` keys never hard-fail.
pub fn parse(body: &Value) -> Result<Incoming, (i64, String)> {
    let obj = body
        .as_object()
        .ok_or((INVALID_REQUEST, "request must be a JSON object".to_string()))?;

    if let Some(version) = obj.get("jsonrpc") {
        if version.as_str() != Some("2.0") {
            return Err((INVALID_REQUEST, "jsonrpc must be \"2.0\"".to_string()));
        }
    }

    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or((INVALID_REQUEST, "missing method".to_string()))?
        .to_string();

    // An explicit `null` id is a malformed request under MCP, not a
    // notification: notifications omit the member entirely.
    let id = match obj.get("id") {
        None => None,
        Some(Value::Null) => {
            return Err((INVALID_REQUEST, "id must not be null".to_string()));
        }
        Some(v) if v.is_string() || v.is_number() => Some(v.clone()),
        Some(_) => {
            return Err((INVALID_REQUEST, "id must be a string or number".to_string()));
        }
    };

    let params = match obj.get("params") {
        Some(v) if v.is_object() => v.clone(),
        _ => json!({}),
    };

    Ok(Incoming { id, method, params })
}

/// A successful JSON-RPC response envelope.
pub fn success(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A JSON-RPC error response, optionally carrying `error.data`.
///
/// `id` is omitted when the request could not be read far enough to recover
/// one — which the spec explicitly allows, and which the Origin rejection path
/// relies on, since it fires before the body is parsed.
pub fn failure_data(
    id: Option<&Value>,
    code: i64,
    message: impl Into<String>,
    data: Option<Value>,
) -> Value {
    let mut error = json!({ "code": code, "message": message.into() });
    if let Some(data) = data {
        error["data"] = data;
    }
    let mut envelope = json!({ "jsonrpc": "2.0", "error": error });
    if let Some(id) = id {
        envelope["id"] = id.clone();
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_request() {
        let parsed = parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "get_run", "arguments": { "run_id": "r1" } }
        }))
        .unwrap();
        assert!(!parsed.is_notification());
        assert_eq!(parsed.method, "tools/call");
        assert_eq!(parsed.name_field(), Some("get_run"));
        assert_eq!(parsed.arguments()["run_id"], "r1");
    }

    #[test]
    fn a_missing_id_is_a_notification() {
        let parsed =
            parse(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).unwrap();
        assert!(parsed.is_notification());
    }

    #[test]
    fn an_explicit_null_id_is_malformed_not_a_notification() {
        let err = parse(&json!({ "jsonrpc": "2.0", "id": null, "method": "ping" })).unwrap_err();
        assert_eq!(err.0, INVALID_REQUEST);
    }

    #[test]
    fn rejects_a_wrong_jsonrpc_version() {
        let err = parse(&json!({ "jsonrpc": "1.0", "id": 1, "method": "ping" })).unwrap_err();
        assert_eq!(err.0, INVALID_REQUEST);
    }

    #[test]
    fn missing_params_normalizes_to_an_empty_object() {
        let parsed = parse(&json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).unwrap();
        assert_eq!(parsed.params, json!({}));
        assert_eq!(parsed.arguments(), json!({}));
        assert_eq!(parsed.name_field(), None);
    }

    #[test]
    fn resources_read_mirrors_uri_into_the_name_field() {
        let parsed = parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "resources/read",
            "params": { "uri": "file:///a.json" }
        }))
        .unwrap();
        assert_eq!(parsed.name_field(), Some("file:///a.json"));
    }

    #[test]
    fn failure_omits_the_id_when_there_is_none() {
        let body = failure_data(None, HEADER_MISMATCH, "nope", None);
        assert!(body.get("id").is_none());
        assert!(body["error"].get("data").is_none());
        assert_eq!(body["error"]["code"], HEADER_MISMATCH);
    }

    #[test]
    fn failure_carries_id_and_data_when_given() {
        let body = failure_data(
            Some(&json!(7)),
            UNSUPPORTED_PROTOCOL_VERSION,
            "nope",
            Some(json!({ "supported": SUPPORTED_VERSIONS })),
        );
        assert_eq!(body["id"], 7);
        assert_eq!(body["error"]["data"]["supported"][0], MODERN_VERSION);
    }

    #[test]
    fn every_invented_code_sits_outside_the_reserved_range() {
        // -32768..=-32000 is reserved by JSON-RPC; MCP further forbids
        // -32000..=-32019 to new implementations.
        for code in [AUTH_REQUIRED, RATE_LIMITED] {
            assert!(code > -32000, "{code} must be outside the reserved range");
        }
    }
}
