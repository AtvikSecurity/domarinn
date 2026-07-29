//! Integration tests for the MCP endpoint: gating, transport conformance,
//! header validation, origin policy, CORS, and the dual-era split.
//!
//! Authorization and the tool/prompt surface live in `mcp_tools.rs`.

mod common;

use axum::http::StatusCode;
use axum::Router;
use common::*;
use serde_json::{json, Value};
use tempfile::TempDir;

use domarinn_server::{build_app, AuthMode, ServerConfig, Settings};

pub const MCP: &str = "/api/v1/mcp";
pub const MODERN: &str = "2026-07-28";

/// An MCP-enabled app in `open` mode, so transport tests are not entangled
/// with authorization.
pub async fn mcp_app() -> (Router, TempDir) {
    mcp_app_with(Settings {
        mcp_enabled: Some(true),
        ..Default::default()
    })
    .await
}

pub async fn mcp_app_with(settings: Settings) -> (Router, TempDir) {
    let mode = settings.auth_mode.unwrap_or(AuthMode::Open);
    let dir = TempDir::new().expect("tempdir");
    let config = ServerConfig {
        port: 0,
        data_dir: dir.path().to_path_buf(),
        auth_mode: mode,
    };
    let (app, _state) = build_app(&config, settings).await.expect("build_app");
    (app, dir)
}

/// A modern-era request body carrying the `_meta` fields the era requires.
pub fn modern_body(id: i64, method: &str, params: Value) -> Value {
    let mut params = params;
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": { "name": "test", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// The headers a conforming modern client mirrors from the body.
pub fn modern_headers(method: &str, name: Option<&str>) -> Vec<(String, String)> {
    let mut headers = vec![
        ("mcp-protocol-version".to_string(), MODERN.to_string()),
        ("mcp-method".to_string(), method.to_string()),
    ];
    if let Some(name) = name {
        headers.push(("mcp-name".to_string(), name.to_string()));
    }
    headers
}

pub async fn post_mcp(app: &Router, headers: &[(String, String)], body: &Value) -> Reply {
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    send_with_headers(
        app,
        "POST",
        MCP,
        &borrowed,
        serde_json::to_vec(body).unwrap(),
    )
    .await
}

/// A modern `tools/list` with every header correct — the happy path other
/// tests perturb.
pub async fn modern_tools_list(app: &Router) -> Reply {
    post_mcp(
        app,
        &modern_headers("tools/list", None),
        &modern_body(1, "tools/list", json!({})),
    )
    .await
}

// ---------------------------------------------------------------------------
// Gating
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_endpoint_is_absent_unless_enabled() {
    let (app, _dir) = mcp_app_with(Settings::default()).await;
    let r = post_mcp(
        &app,
        &modern_headers("tools/list", None),
        &modern_body(1, "tools/list", json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // A JSON 404 from `spa_fallback`'s `/api/` branch, never the SPA shell.
    assert_eq!(r.json()["error"], "route not found");
}

#[tokio::test]
async fn the_endpoint_answers_once_enabled() {
    let (app, _dir) = mcp_app().await;
    assert_eq!(modern_tools_list(&app).await.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retired_verbs_return_405_with_allow_post() {
    let (app, _dir) = mcp_app().await;
    for method in ["GET", "DELETE"] {
        let r = send_with_headers(&app, method, MCP, &[], Vec::new()).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method}");
        assert_eq!(
            r.headers.get("allow").unwrap().to_str().unwrap(),
            "POST",
            "{method} must advertise the one verb we accept"
        );
    }
}

#[tokio::test]
async fn a_notification_is_accepted_with_no_body_and_no_header_validation() {
    let (app, _dir) = mcp_app().await;
    // Deliberately no Mcp-Method header: notifications are exempt.
    let r = post_mcp(
        &app,
        &[],
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(r.status, StatusCode::ACCEPTED);
    assert!(r.body.is_empty(), "202 must carry no body");
}

#[tokio::test]
async fn responses_are_always_json_never_an_event_stream() {
    let (app, _dir) = mcp_app().await;
    let r = modern_tools_list(&app).await;
    let content_type = r.headers.get("content-type").unwrap().to_str().unwrap();
    assert!(
        content_type.starts_with("application/json"),
        "{content_type}"
    );
}

#[tokio::test]
async fn an_oversized_body_is_rejected_even_when_gzipped() {
    let (app, _dir) = mcp_app().await;
    // 512 KiB of padding, twice the route's 256 KiB ceiling.
    let fat = modern_body(1, "tools/list", json!({ "pad": "x".repeat(512 * 1024) }));
    let raw = serde_json::to_vec(&fat).unwrap();

    let plain = send_with_headers(
        &app,
        "POST",
        MCP,
        &[
            ("mcp-protocol-version", MODERN),
            ("mcp-method", "tools/list"),
        ],
        raw.clone(),
    )
    .await;
    assert_eq!(plain.status, StatusCode::PAYLOAD_TOO_LARGE);

    // The limit sits outside decompression, so it bounds the *decompressed*
    // stream — which is what defuses a gzip bomb.
    let squashed = gzip(&raw);
    assert!(
        squashed.len() < 256 * 1024,
        "the bomb must look small on the wire"
    );
    let compressed = send_with_headers(
        &app,
        "POST",
        MCP,
        &[
            ("mcp-protocol-version", MODERN),
            ("mcp-method", "tools/list"),
            ("content-encoding", "gzip"),
        ],
        squashed,
    )
    .await;
    assert_eq!(compressed.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn malformed_json_is_a_parse_error() {
    let (app, _dir) = mcp_app().await;
    let r = send_with_headers(&app, "POST", MCP, &[], b"{not json".to_vec()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32700);
}

#[tokio::test]
async fn an_explicit_null_id_is_rejected_rather_than_treated_as_a_notification() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &[],
        &json!({ "jsonrpc": "2.0", "id": null, "method": "ping" }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32600);
}

#[tokio::test]
async fn an_unknown_method_is_a_404_with_method_not_found() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &modern_headers("foo/bar", None),
        &modern_body(1, "foo/bar", json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"]["code"], -32601);
    // The JSON-RPC body is what tells a client this is a modern server rather
    // than a host that simply does not have the path.
    assert_eq!(r.json()["jsonrpc"], "2.0");
}

// ---------------------------------------------------------------------------
// Modern-era header validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_method_header_disagreeing_with_the_body_is_rejected() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &modern_headers("tools/list", None),
        &modern_body(1, "prompts/list", json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32020);
}

#[tokio::test]
async fn a_name_header_disagreeing_with_the_body_is_rejected() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &modern_headers("prompts/get", Some("summarize_run")),
        &modern_body(1, "prompts/get", json!({ "name": "investigate_case" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32020);
}

#[tokio::test]
async fn a_missing_name_header_is_rejected_for_the_methods_that_require_it() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &modern_headers("prompts/get", None),
        &modern_body(1, "prompts/get", json!({ "name": "summarize_run" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32020);
}

#[tokio::test]
async fn a_base64_sentinel_name_header_is_decoded_before_comparison() {
    let (app, _dir) = mcp_app().await;
    // "summarize_run"
    let r = post_mcp(
        &app,
        &modern_headers("prompts/get", Some("=?base64?c3VtbWFyaXplX3J1bg==?=")),
        &modern_body(
            1,
            "prompts/get",
            json!({ "name": "summarize_run", "arguments": { "run_id": "r1" } }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "body: {:?}", r.json());
}

#[tokio::test]
async fn header_names_are_matched_case_insensitively() {
    let (app, _dir) = mcp_app().await;
    let headers = vec![
        ("MCP-Protocol-Version".to_string(), MODERN.to_string()),
        ("MCP-METHOD".to_string(), "tools/list".to_string()),
    ];
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn the_version_header_must_agree_with_the_meta_field() {
    let (app, _dir) = mcp_app().await;
    let headers = vec![
        ("mcp-protocol-version".to_string(), "2025-11-25".to_string()),
        ("mcp-method".to_string(), "tools/list".to_string()),
    ];
    // Body claims modern, header claims legacy: the era resolves from the
    // header (legacy), so this is served without header validation — the
    // mismatch that matters is caught only in the modern branch. Assert the
    // reverse pairing, which does reach validation.
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);

    let mut body = modern_body(1, "tools/list", json!({}));
    body["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2025-11-25");
    let r = post_mcp(&app, &modern_headers("tools/list", None), &body).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32020);
}

#[tokio::test]
async fn an_unsupported_protocol_version_lists_what_we_do_support() {
    let (app, _dir) = mcp_app().await;
    let headers = vec![
        ("mcp-protocol-version".to_string(), "2099-01-01".to_string()),
        ("mcp-method".to_string(), "tools/list".to_string()),
    ];
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32022);
    let supported = r.json()["error"]["data"]["supported"].clone();
    assert_eq!(supported.as_array().unwrap().len(), 3);
    assert_eq!(supported[0], MODERN);
    assert_eq!(r.json()["error"]["data"]["requested"], "2099-01-01");
}

#[tokio::test]
async fn modern_requests_must_carry_the_mandatory_meta_fields() {
    let (app, _dir) = mcp_app().await;
    let mut body = modern_body(1, "tools/list", json!({}));
    body["params"]["_meta"]
        .as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    let r = post_mcp(&app, &modern_headers("tools/list", None), &body).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], -32602);
}

#[tokio::test]
async fn mcp_param_headers_are_ignored_not_validated() {
    let (app, _dir) = mcp_app().await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("mcp-param-region".to_string(), "us-west1".to_string()));
    headers.push(("mcp-param-other".to_string(), "=?base64?bogus".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn session_and_resume_headers_are_ignored_and_never_echoed() {
    let (app, _dir) = mcp_app().await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("mcp-session-id".to_string(), "abc123".to_string()));
    headers.push(("last-event-id".to_string(), "42".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.headers.get("mcp-session-id").is_none());
}

// ---------------------------------------------------------------------------
// Origin policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_absent_origin_is_allowed() {
    // The case that breaks every CLI client if it is wrong.
    let (app, _dir) = mcp_app().await;
    assert_eq!(modern_tools_list(&app).await.status, StatusCode::OK);
}

#[tokio::test]
async fn loopback_origins_are_allowed_by_default() {
    let (app, _dir) = mcp_app().await;
    for origin in [
        "http://localhost:8321",
        "http://127.0.0.1:3000",
        "http://[::1]:9",
    ] {
        let mut headers = modern_headers("tools/list", None);
        headers.push(("origin".to_string(), origin.to_string()));
        let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
        assert_eq!(r.status, StatusCode::OK, "{origin}");
    }
}

#[tokio::test]
async fn a_foreign_origin_is_rejected_with_an_idless_error() {
    let (app, _dir) = mcp_app().await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("origin".to_string(), "http://evil.example".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // The spec allows an id-less error here, and we take it: the body has
    // deliberately not been parsed at that point.
    assert!(r.json().get("id").is_none());
}

#[tokio::test]
async fn origin_null_fails_closed() {
    let (app, _dir) = mcp_app().await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("origin".to_string(), "null".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

/// The DNS-rebinding case the shared `origin_allowed` helper would let
/// through: an attacker controls both `Host` and `Origin`, so they agree.
/// This test is the entire reason the MCP endpoint has its own policy.
#[tokio::test]
async fn a_matching_host_header_does_not_rescue_a_foreign_origin() {
    let (app, _dir) = mcp_app_with(Settings {
        mcp_enabled: Some(true),
        public_url: Some("https://domarinn.internal".to_string()),
        ..Default::default()
    })
    .await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("origin".to_string(), "http://evil.example".to_string()));
    headers.push(("host".to_string(), "evil.example".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn explicitly_allowed_origins_are_accepted() {
    let (app, _dir) = mcp_app_with(Settings {
        mcp_enabled: Some(true),
        mcp_allowed_origins: Some("https://studio.example".to_string()),
        ..Default::default()
    })
    .await;
    let mut headers = modern_headers("tools/list", None);
    headers.push(("origin".to_string(), "https://studio.example".to_string()));
    let r = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// CORS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_preflight_from_an_allowed_origin_is_answered() {
    let (app, _dir) = mcp_app().await;
    let r = send_with_headers(
        &app,
        "OPTIONS",
        MCP,
        &[
            ("origin", "http://localhost:5173"),
            ("access-control-request-method", "POST"),
            ("access-control-request-headers", "authorization,mcp-method"),
        ],
        Vec::new(),
    )
    .await;
    assert!(r.status.is_success(), "status: {}", r.status);
    assert_eq!(
        r.headers
            .get("access-control-allow-origin")
            .unwrap()
            .to_str()
            .unwrap(),
        "http://localhost:5173"
    );
    let allowed_headers = r
        .headers
        .get("access-control-allow-headers")
        .unwrap()
        .to_str()
        .unwrap()
        .to_ascii_lowercase();
    assert!(allowed_headers.contains("mcp-method"));
    assert!(allowed_headers.contains("authorization"));
}

#[tokio::test]
async fn a_preflight_from_a_foreign_origin_gets_no_allow_origin() {
    let (app, _dir) = mcp_app().await;
    let r = send_with_headers(
        &app,
        "OPTIONS",
        MCP,
        &[
            ("origin", "http://evil.example"),
            ("access-control-request-method", "POST"),
        ],
        Vec::new(),
    )
    .await;
    assert!(r.headers.get("access-control-allow-origin").is_none());
}

/// The load-bearing safety property: with credentials off, a browser will not
/// attach `domarinn_session` cross-origin, so the endpoint cannot become a
/// CSRF vector no matter what the allowlist says.
#[tokio::test]
async fn cors_never_allows_credentials() {
    let (app, _dir) = mcp_app().await;
    let preflight = send_with_headers(
        &app,
        "OPTIONS",
        MCP,
        &[
            ("origin", "http://localhost:5173"),
            ("access-control-request-method", "POST"),
        ],
        Vec::new(),
    )
    .await;
    assert!(preflight
        .headers
        .get("access-control-allow-credentials")
        .is_none());

    let mut headers = modern_headers("tools/list", None);
    headers.push(("origin".to_string(), "http://localhost:5173".to_string()));
    let actual = post_mcp(&app, &headers, &modern_body(1, "tools/list", json!({}))).await;
    assert!(actual
        .headers
        .get("access-control-allow-credentials")
        .is_none());
}

#[tokio::test]
async fn cors_is_scoped_to_the_mcp_route_only() {
    let (app, _dir) = mcp_app().await;
    let r = send_with_headers(
        &app,
        "GET",
        "/api/v1/runs",
        &[("origin", "http://localhost:5173")],
        Vec::new(),
    )
    .await;
    assert!(
        r.headers.get("access-control-allow-origin").is_none(),
        "the rest of the API is same-origin by design"
    );
}

// ---------------------------------------------------------------------------
// Dual era
// ---------------------------------------------------------------------------

#[tokio::test]
async fn legacy_initialize_negotiates_without_minting_a_session() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &[],
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-11-25", "capabilities": {} }
        }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let result = &r.json()["result"];
    assert_eq!(result["protocolVersion"], "2025-11-25");
    assert_eq!(result["serverInfo"]["name"], "domarinn");
    assert!(result["capabilities"]["tools"].is_object());
    assert!(result["capabilities"]["prompts"].is_object());
    // Legacy results predate `resultType`; emitting it would confuse a client
    // validating against the older schema.
    assert!(result.get("resultType").is_none());
    assert!(r.headers.get("mcp-session-id").is_none());
}

#[tokio::test]
async fn an_unknown_requested_version_still_gets_a_usable_initialize_reply() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &[],
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "1999-01-01" }
        }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    // A legacy client has no fall-forward mechanism, so naming a version we
    // speak is the only useful thing we can tell it.
    assert_eq!(r.json()["result"]["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn legacy_results_carry_no_result_type_or_cache_hints() {
    let (app, _dir) = mcp_app().await;
    let headers = vec![
        ("mcp-protocol-version".to_string(), "2025-06-18".to_string()),
        ("mcp-method".to_string(), "tools/list".to_string()),
    ];
    let r = post_mcp(
        &app,
        &headers,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let result = &r.json()["result"];
    assert!(result["tools"].as_array().unwrap().len() == 8);
    assert!(result.get("resultType").is_none());
    assert!(result.get("ttlMs").is_none());
    assert!(result.get("cacheScope").is_none());
}

#[tokio::test]
async fn modern_results_carry_result_type_and_cache_hints() {
    let (app, _dir) = mcp_app().await;
    let result = modern_tools_list(&app).await.json()["result"].clone();
    assert_eq!(result["resultType"], "complete");
    assert_eq!(result["ttlMs"], 300_000);
    // Both catalogs are identical for every caller, which is what makes
    // `public` accurate rather than merely convenient.
    assert_eq!(result["cacheScope"], "public");
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "domarinn"
    );
}

#[tokio::test]
async fn a_header_less_request_is_served_as_legacy() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &[],
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {} }),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json()["result"].get("resultType").is_none());
}

#[tokio::test]
async fn server_discover_answers_in_both_eras() {
    let (app, _dir) = mcp_app().await;

    let modern = post_mcp(
        &app,
        &modern_headers("server/discover", None),
        &modern_body(1, "server/discover", json!({})),
    )
    .await;
    assert_eq!(modern.status, StatusCode::OK);
    let result = &modern.json()["result"];
    assert_eq!(result["supportedVersions"].as_array().unwrap().len(), 3);
    assert_eq!(result["capabilities"]["tools"]["listChanged"], false);
    assert_eq!(result["ttlMs"], 3_600_000);
    assert!(result["instructions"]
        .as_str()
        .unwrap()
        .contains("read-only"));

    // Costs nothing to answer for a probing legacy client, and removes a branch.
    let legacy = post_mcp(
        &app,
        &[],
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "server/discover", "params": {} }),
    )
    .await;
    assert_eq!(legacy.status, StatusCode::OK);
}

#[tokio::test]
async fn ping_is_answered() {
    let (app, _dir) = mcp_app().await;
    let r = post_mcp(
        &app,
        &modern_headers("ping", None),
        &modern_body(1, "ping", json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["result"]["resultType"], "complete");
}

// ---------------------------------------------------------------------------
// OAuth discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth_discovery_returns_a_definitive_json_404() {
    let (app, _dir) = mcp_app().await;
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/api/v1/mcp",
        "/.well-known/oauth-authorization-server",
    ] {
        let r = get(&app, path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        // Without these routes `spa_fallback` answers with the SPA shell at
        // HTTP 200, which reads as "maybe OAuth" rather than "no".
        let content_type = r.headers.get("content-type").unwrap().to_str().unwrap();
        assert!(content_type.starts_with("application/json"), "{path}");
        assert!(r.json()["error"]
            .as_str()
            .unwrap()
            .contains("does not use OAuth"));
    }
}
