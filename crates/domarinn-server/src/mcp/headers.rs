//! Modern-era header ↔ body cross-validation.
//!
//! `2026-07-28` mirrors selected body fields into HTTP headers so gateways can
//! route without parsing JSON. Any server that reads the body **MUST** verify
//! the two agree — otherwise an intermediary routing on the header and a
//! server executing on the body can be made to disagree.
//!
//! Two deliberate non-behaviors, stated so they are not "fixed" later:
//!
//! * **`Mcp-Param-*` headers are ignored entirely.** They exist only for tool
//!   parameters a server marks with `x-mcp-header` in its `inputSchema`. This
//!   server marks none, so it recognizes none, and per RFC 9110 an
//!   unrecognized field is forwarded and otherwise ignored.
//! * **`Accept` is not validated.** Listing both media types is a *client*
//!   MUST; rejecting on it would break real clients for no benefit.

use axum::http::HeaderMap;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;

use super::jsonrpc::Incoming;
use super::proto::{header_str, meta_version, Rejection, PROTOCOL_VERSION_HEADER};

/// Mirrors the request body's `method`.
const METHOD_HEADER: &str = "mcp-method";
/// Mirrors `params.name` or `params.uri`.
const NAME_HEADER: &str = "mcp-name";

/// Methods whose `Mcp-Name` header is required, per the spec's table.
const NAME_REQUIRED_METHODS: [&str; 3] = ["tools/call", "resources/read", "prompts/get"];

/// Marks a header value as base64-encoded UTF-8. Case-sensitive, lowercase.
const SENTINEL_PREFIX: &str = "=?base64?";
const SENTINEL_SUFFIX: &str = "?=";

/// Verify the mirrored headers against the parsed body.
///
/// Only called for modern-era **requests**. Notifications are exempt: the spec
/// does not define header requirements for a notification POST.
pub fn validate(headers: &HeaderMap, incoming: &Incoming) -> Result<(), Rejection> {
    // The version header must agree with `params._meta`. Both are required in
    // the modern era, so a missing header here is a mismatch, not leniency.
    let header_version = header_str(headers, PROTOCOL_VERSION_HEADER).ok_or_else(|| {
        Rejection::header_mismatch("missing required header MCP-Protocol-Version")
    })?;
    if let Some(body_version) = meta_version(incoming) {
        if header_version != body_version {
            return Err(Rejection::header_mismatch(format!(
                "Header mismatch: MCP-Protocol-Version header value '{header_version}' \
                 does not match body value '{body_version}'"
            )));
        }
    }

    let header_method = header_str(headers, METHOD_HEADER)
        .ok_or_else(|| Rejection::header_mismatch("missing required header Mcp-Method"))?;
    if header_method != incoming.method {
        return Err(Rejection::header_mismatch(format!(
            "Header mismatch: Mcp-Method header value '{header_method}' \
             does not match body value '{}'",
            incoming.method
        )));
    }

    if NAME_REQUIRED_METHODS.contains(&incoming.method.as_str()) {
        let raw = header_str(headers, NAME_HEADER)
            .ok_or_else(|| Rejection::header_mismatch("missing required header Mcp-Name"))?;
        let header_name = decode_sentinel(raw).ok_or_else(|| {
            Rejection::header_mismatch("Mcp-Name is not valid base64-sentinel encoded UTF-8")
        })?;
        let body_name = incoming.name_field().unwrap_or_default();
        if header_name != body_name {
            return Err(Rejection::header_mismatch(format!(
                "Header mismatch: Mcp-Name header value '{header_name}' \
                 does not match body value '{body_name}'"
            )));
        }
    }

    Ok(())
}

/// Decode a header value that may use the `=?base64?...?=` sentinel.
///
/// Plain values pass through unchanged. Returns `None` only when a value
/// *claims* the sentinel but is not decodable base64 UTF-8 — a plain value
/// can never fail.
pub fn decode_sentinel(raw: &str) -> Option<String> {
    let Some(rest) = raw.strip_prefix(SENTINEL_PREFIX) else {
        return Some(raw.to_string());
    };
    let Some(encoded) = rest.strip_suffix(SENTINEL_SUFFIX) else {
        // Starts like a sentinel but is not one. Treat it literally rather
        // than guessing; a conforming client would have encoded it.
        return Some(raw.to_string());
    };
    let bytes = BASE64.decode(encoded).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::jsonrpc::{
        self, META_CLIENT_CAPABILITIES, META_PROTOCOL_VERSION, MODERN_VERSION,
    };
    use serde_json::json;

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

    fn call(name: &str) -> Incoming {
        jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {
                "name": name,
                "arguments": {},
                "_meta": {
                    META_PROTOCOL_VERSION: MODERN_VERSION,
                    META_CLIENT_CAPABILITIES: {}
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn accepts_matching_headers() {
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "get_run"),
        ]);
        assert!(validate(&h, &call("get_run")).is_ok());
    }

    #[test]
    fn header_names_are_case_insensitive() {
        let h = headers(&[
            ("MCP-Protocol-Version", MODERN_VERSION),
            ("Mcp-Method", "tools/call"),
            ("MCP-NAME", "get_run"),
        ]);
        assert!(validate(&h, &call("get_run")).is_ok());
    }

    #[test]
    fn rejects_a_method_mismatch() {
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/list"),
            (NAME_HEADER, "get_run"),
        ]);
        let err = validate(&h, &call("get_run")).unwrap_err();
        assert_eq!(err.code, jsonrpc::HEADER_MISMATCH);
    }

    #[test]
    fn rejects_a_name_mismatch_and_a_missing_name() {
        let mismatch = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "other_tool"),
        ]);
        assert_eq!(
            validate(&mismatch, &call("get_run")).unwrap_err().code,
            jsonrpc::HEADER_MISMATCH
        );

        let missing = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/call"),
        ]);
        assert_eq!(
            validate(&missing, &call("get_run")).unwrap_err().code,
            jsonrpc::HEADER_MISMATCH
        );
    }

    #[test]
    fn rejects_a_version_header_body_disagreement() {
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, "2025-11-25"),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "get_run"),
        ]);
        assert_eq!(
            validate(&h, &call("get_run")).unwrap_err().code,
            jsonrpc::HEADER_MISMATCH
        );
    }

    #[test]
    fn name_is_not_required_for_other_methods() {
        let list = jsonrpc::parse(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/list",
            "params": { "_meta": {
                META_PROTOCOL_VERSION: MODERN_VERSION,
                META_CLIENT_CAPABILITIES: {}
            } }
        }))
        .unwrap();
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/list"),
        ]);
        assert!(validate(&h, &list).is_ok());
    }

    #[test]
    fn decodes_the_base64_sentinel() {
        // "get_run"
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "=?base64?Z2V0X3J1bg==?="),
        ]);
        assert!(validate(&h, &call("get_run")).is_ok());
    }

    #[test]
    fn sentinel_round_trips_the_specs_self_referential_example() {
        // A literal value that itself looks like the sentinel must be encoded
        // by the client; decoding must recover it exactly.
        assert_eq!(
            decode_sentinel("=?base64?PT9iYXNlNjQ/bGl0ZXJhbD89?=").unwrap(),
            "=?base64?literal?="
        );
    }

    #[test]
    fn sentinel_handles_non_ascii_and_plain_values() {
        assert_eq!(decode_sentinel("us-west1").unwrap(), "us-west1");
        assert_eq!(
            decode_sentinel("=?base64?SGVsbG8sIOS4lueVjA==?=").unwrap(),
            "Hello, 世界"
        );
        // Claims the sentinel but is not decodable.
        assert!(decode_sentinel("=?base64?not!valid!base64?=").is_none());
        // Opens like a sentinel but never closes: taken literally.
        assert_eq!(decode_sentinel("=?base64?abc").unwrap(), "=?base64?abc");
    }

    #[test]
    fn mcp_param_headers_are_ignored() {
        let h = headers(&[
            (PROTOCOL_VERSION_HEADER, MODERN_VERSION),
            (METHOD_HEADER, "tools/call"),
            (NAME_HEADER, "get_run"),
            ("mcp-param-region", "junk"),
            ("mcp-param-anything", "=?base64?bogus"),
        ]);
        assert!(validate(&h, &call("get_run")).is_ok());
    }
}
