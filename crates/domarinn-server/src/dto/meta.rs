//! DTO for `GET /meta`.

use serde::Serialize;
use ts_rs::TS;

use crate::domain::{CacheTier, SsoKind};
use crate::AuthMode;

/// The effective cache retention/size limits, as reported to clients (mirrors
/// [`crate::CacheLimits`], which does not itself derive `Serialize`/`TS`
/// since it is also used as plain server-internal config).
#[derive(Debug, Clone, Serialize, TS)]
pub struct MetaCacheLimits {
    pub max_entry_bytes: usize,
    pub max_bytes: u64,
    pub max_age_days: u64,
}

/// A browsable cache tier on this instance.
///
/// Deliberately carries no filesystem path. `/meta` is unauthenticated by
/// design — the login page reads `setup_required` and `auth_mode` from it — and
/// where an operator keeps a cache directory is their infrastructure, not a
/// fact for anonymous callers. The tier's identity is all a client needs to
/// render a switcher.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheTierMeta {
    pub id: CacheTier,
    pub label: String,
    /// What `?q=` means on this tier. The server tier answers with a full-text
    /// index; a mounted directory can only substring-match previews. Advertised
    /// rather than left implicit, because "search" quietly meaning two
    /// different things is worse than a missing feature.
    pub search: String,
}

/// A configured SSO provider, as advertised to the login page: enough to
/// render a "Continue with {label}" button that navigates to `login_url`.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SsoProviderMeta {
    pub name: String,
    pub kind: SsoKind,
    pub label: String,
    /// Same-origin path starting the flow, e.g.
    /// `/api/v1/auth/oidc/google/start`. Accepts `?return_to=/path`.
    pub login_url: String,
}

/// `GET /meta` response: server identity, the active auth mode, whether
/// first-run account setup is still required, the configured SSO providers,
/// and the schema-version / cache-limit contract clients should honor.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MetaResponse {
    pub name: String,
    pub version: String,
    pub auth_mode: AuthMode,
    pub setup_required: bool,
    pub sso_providers: Vec<SsoProviderMeta>,
    pub supported_schema_versions: Vec<u32>,
    pub result_schema_version: u32,
    pub cache: MetaCacheLimits,
    /// Cache tiers this instance can browse. Always contains the server tier;
    /// a second entry appears only when a local directory is mounted.
    pub cache_tiers: Vec<CacheTierMeta>,
    /// Whether the MCP endpoint is mounted (`DOMARINN_MCP_ENABLED`).
    ///
    /// Advertised because the endpoint is opt-in and unauthenticated probing
    /// cannot distinguish "disabled" from "wrong URL": both are a JSON 404.
    /// The settings page uses this to show operators how to connect a client,
    /// or how to turn it on.
    pub mcp_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meta_response_matches_todays_wire_shape() {
        let dto = MetaResponse {
            name: "domarinn".to_string(),
            version: "0.1.0".to_string(),
            auth_mode: AuthMode::ProtectWrites,
            setup_required: true,
            sso_providers: vec![SsoProviderMeta {
                name: "google".to_string(),
                kind: SsoKind::Oidc,
                label: "Google".to_string(),
                login_url: "/api/v1/auth/oidc/google/start".to_string(),
            }],
            supported_schema_versions: vec![0, 1],
            result_schema_version: 1,
            cache: MetaCacheLimits {
                max_entry_bytes: 4 * 1024 * 1024,
                max_bytes: 1024 * 1024 * 1024,
                max_age_days: 30,
            },
            cache_tiers: vec![CacheTierMeta {
                id: CacheTier::Server,
                label: "Server".to_string(),
                search: "fts".to_string(),
            }],
            mcp_enabled: true,
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "name": "domarinn",
                "version": "0.1.0",
                "auth_mode": "protect-writes",
                "setup_required": true,
                "sso_providers": [{
                    "name": "google",
                    "kind": "oidc",
                    "label": "Google",
                    "login_url": "/api/v1/auth/oidc/google/start",
                }],
                "supported_schema_versions": [0, 1],
                "result_schema_version": 1,
                "cache": {
                    "max_entry_bytes": 4 * 1024 * 1024,
                    "max_bytes": 1024 * 1024 * 1024,
                    "max_age_days": 30,
                },
                "cache_tiers": [{
                    "id": "server",
                    "label": "Server",
                    "search": "fts",
                }],
                "mcp_enabled": true,
            })
        );
    }

    #[test]
    fn auth_mode_variants_serialize_to_kebab_case() {
        assert_eq!(serde_json::to_value(AuthMode::Open).unwrap(), json!("open"));
        assert_eq!(
            serde_json::to_value(AuthMode::ProtectWrites).unwrap(),
            json!("protect-writes")
        );
        assert_eq!(
            serde_json::to_value(AuthMode::Closed).unwrap(),
            json!("closed")
        );
    }
}
