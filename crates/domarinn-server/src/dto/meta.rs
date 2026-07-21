//! DTO for `GET /meta`.

use serde::Serialize;
use ts_rs::TS;

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

/// `GET /meta` response: server identity, the active auth mode, whether
/// first-run account setup is still required, and the schema-version /
/// cache-limit contract clients should honor.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MetaResponse {
    pub name: String,
    pub version: String,
    pub auth_mode: AuthMode,
    pub setup_required: bool,
    pub supported_schema_versions: Vec<u32>,
    pub result_schema_version: u32,
    pub cache: MetaCacheLimits,
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
            supported_schema_versions: vec![0, 1],
            result_schema_version: 1,
            cache: MetaCacheLimits {
                max_entry_bytes: 4 * 1024 * 1024,
                max_bytes: 1024 * 1024 * 1024,
                max_age_days: 30,
            },
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "name": "domarinn",
                "version": "0.1.0",
                "auth_mode": "protect-writes",
                "setup_required": true,
                "supported_schema_versions": [0, 1],
                "result_schema_version": 1,
                "cache": {
                    "max_entry_bytes": 4 * 1024 * 1024,
                    "max_bytes": 1024 * 1024 * 1024,
                    "max_age_days": 30,
                },
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
