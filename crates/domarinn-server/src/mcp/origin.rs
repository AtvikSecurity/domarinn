//! Origin policy for the MCP endpoint, and the CORS layer derived from it.
//!
//! The spec requires servers to validate `Origin` to defeat DNS rebinding: a
//! malicious page resolves its own hostname to `127.0.0.1` and then talks to a
//! local MCP server from the victim's browser.
//!
//! **This deliberately does not reuse [`crate::origin_allowed`].** That helper
//! accepts any origin equal to the request's own `Host`, which is exactly what
//! a rebinding attacker produces — the browser sends `Host: evil.example` *and*
//! `Origin: http://evil.example`, they match, and the check passes. A CSRF
//! defense can afford that (the cookie is `SameSite=Lax` anyway); the MCP
//! endpoint cannot. Here the allowlist is closed and never derived from the
//! request.

use std::sync::Arc;
use std::time::Duration;

use axum::http::{header, HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::{presented_origin, url_host, PresentedOrigin};

/// Hosts always allowed when no explicit policy is configured: an operator
/// running `domarinn server` locally should not have to configure anything to
/// point a local MCP client at it. Matched on the host part only, so any port
/// qualifies.
const LOOPBACK_HOSTS: [&str; 4] = ["localhost", "127.0.0.1", "[::1]", "::1"];

/// The set of origins permitted to reach the MCP endpoint.
#[derive(Debug, Clone, Default)]
pub struct OriginPolicy {
    /// Normalized `host[:port]` entries. An entry without a port matches any
    /// port on that host.
    allowed: Arc<Vec<String>>,
}

impl OriginPolicy {
    /// Build from `DOMARINN_PUBLIC_URL` and `DOMARINN_MCP_ALLOWED_ORIGINS`.
    ///
    /// When neither yields an entry the policy falls back to loopback only.
    pub fn new(public_url: Option<&str>, allowed_origins: Option<&str>) -> OriginPolicy {
        let mut allowed = Vec::new();
        if let Some(host) = public_url.and_then(url_host) {
            allowed.push(host);
        }
        for entry in allowed_origins.unwrap_or_default().split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            // Accept either a full origin (`https://app.example`) or a bare
            // `host[:port]`, so operators need not think about which.
            let normalized = url_host(entry).unwrap_or_else(|| entry.to_ascii_lowercase());
            allowed.push(normalized);
        }
        allowed.sort();
        allowed.dedup();
        OriginPolicy {
            allowed: Arc::new(allowed),
        }
    }

    /// Whether a presented `host[:port]` may talk to the MCP endpoint.
    pub fn allows_host(&self, presented: &str) -> bool {
        let presented = presented.to_ascii_lowercase();
        let host_only = host_part(&presented);
        if self.allowed.is_empty() {
            return LOOPBACK_HOSTS.contains(&host_only);
        }
        self.allowed.iter().any(|entry| {
            if entry.contains(':') {
                entry == &presented
            } else {
                entry == host_only
            }
        })
    }

    /// Whether a raw `Origin` header value is allowed.
    pub fn allows_origin_value(&self, origin: &HeaderValue) -> bool {
        origin
            .to_str()
            .ok()
            .and_then(url_host)
            .is_some_and(|host| self.allows_host(&host))
    }

    /// Apply the spec's rule to a request: reject only a *present and invalid*
    /// origin. An absent one is allowed — every CLI MCP client sends none, and
    /// rejecting absence would break all of them for no security gain.
    ///
    /// `Origin: null` (a sandboxed iframe, a `data:` document) parses to
    /// nothing and so fails closed.
    pub fn permits(&self, headers: &axum::http::HeaderMap) -> bool {
        match presented_origin(headers) {
            PresentedOrigin::Absent => true,
            PresentedOrigin::Unparseable => false,
            PresentedOrigin::Host(host) => self.allows_host(&host),
        }
    }

    /// The CORS layer for the MCP route, so browser-hosted MCP clients can
    /// reach it. Layered on the MCP method router **only** — the rest of the
    /// API is same-origin by design and must stay that way.
    pub fn cors_layer(&self) -> CorsLayer {
        let policy = self.clone();
        CorsLayer::new()
            .allow_origin(AllowOrigin::predicate(move |origin, _parts| {
                policy.allows_origin_value(origin)
            }))
            .allow_methods([Method::POST, Method::OPTIONS])
            .allow_headers([
                header::CONTENT_TYPE,
                header::AUTHORIZATION,
                header::HeaderName::from_static("mcp-protocol-version"),
                header::HeaderName::from_static("mcp-method"),
                header::HeaderName::from_static("mcp-name"),
            ])
            // THE load-bearing line. With credentials off the browser will not
            // attach `domarinn_session`, so a cross-origin MCP call can never
            // be cookie-authed and therefore can never be a CSRF vector,
            // whatever the allowlist says. Bearer tokens are unaffected: the
            // client sets that header explicitly.
            .allow_credentials(false)
            .max_age(Duration::from_secs(600))
    }
}

/// The host portion of a `host[:port]`, leaving bracketed IPv6 literals intact.
fn host_part(hostport: &str) -> &str {
    if hostport.starts_with('[') {
        return match hostport.find(']') {
            Some(end) => &hostport[..=end],
            None => hostport,
        };
    }
    hostport.split(':').next().unwrap_or(hostport)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn with_origin(origin: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        h
    }

    #[test]
    fn absent_origin_is_allowed() {
        assert!(OriginPolicy::default().permits(&HeaderMap::new()));
    }

    #[test]
    fn loopback_is_the_default_allowlist_on_any_port() {
        let policy = OriginPolicy::new(None, None);
        for origin in [
            "http://localhost:8321",
            "http://127.0.0.1:3000",
            "http://localhost",
            "http://[::1]:9000",
        ] {
            assert!(policy.permits(&with_origin(origin)), "{origin}");
        }
        assert!(!policy.permits(&with_origin("http://evil.example")));
    }

    #[test]
    fn origin_null_fails_closed() {
        assert!(!OriginPolicy::new(None, None).permits(&with_origin("null")));
    }

    /// The rebinding case `crate::origin_allowed` would let through: the
    /// attacker controls both `Host` and `Origin`, so they always agree.
    #[test]
    fn a_matching_host_header_does_not_rescue_a_foreign_origin() {
        let policy = OriginPolicy::new(Some("https://domarinn.internal"), None);
        let mut headers = with_origin("http://evil.example");
        headers.insert(header::HOST, "evil.example".parse().unwrap());
        assert!(!policy.permits(&headers));
    }

    #[test]
    fn public_url_host_is_allowed() {
        let policy = OriginPolicy::new(Some("https://domarinn.internal:8443"), None);
        assert!(policy.permits(&with_origin("https://domarinn.internal:8443")));
        // A configured entry carrying a port must match that port exactly.
        assert!(!policy.permits(&with_origin("https://domarinn.internal:9999")));
        // Configuring anything drops the loopback default.
        assert!(!policy.permits(&with_origin("http://localhost:8321")));
    }

    #[test]
    fn explicit_origins_accept_both_spellings_and_bare_hosts_match_any_port() {
        let policy = OriginPolicy::new(None, Some("https://app.example, studio.example"));
        assert!(policy.permits(&with_origin("https://app.example")));
        assert!(policy.permits(&with_origin("http://studio.example:4444")));
        assert!(!policy.permits(&with_origin("https://other.example")));
    }

    #[test]
    fn host_part_handles_ipv6_literals() {
        assert_eq!(host_part("[::1]:9000"), "[::1]");
        assert_eq!(host_part("localhost:5173"), "localhost");
        assert_eq!(host_part("localhost"), "localhost");
    }
}
