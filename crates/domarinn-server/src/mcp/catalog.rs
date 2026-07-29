//! Assembly of the three static catalogs: `server/discover`, `tools/list`,
//! and `prompts/list`.
//!
//! All three are identical for every caller — nothing here varies by identity,
//! which is what makes `cacheScope: "public"` accurate. If a future tool is
//! ever gated by scope, this is where the filtering belongs and the scope must
//! become `"private"` at the same time.

use serde_json::{json, Value};

use super::proto::{server_info, CacheHint};
use super::{prompts, tools};

/// Freshness hint for the tool catalog. Static per binary, so an hour would be
/// defensible; five minutes keeps an upgraded server from being shadowed by a
/// stale client cache for long.
pub const TOOLS_TTL_MS: u64 = 300_000;
/// Prompts change even less often than tools.
pub const PROMPTS_TTL_MS: u64 = 600_000;
/// Discovery answers "what is this server", which only changes on upgrade.
pub const DISCOVER_TTL_MS: u64 = 3_600_000;

/// Natural-language guidance sent with discovery. Kept in a Markdown sidecar
/// rather than a Rust string so it can be edited as prose — and so the
/// 1000-line file ratchet never has an opinion about how long it is.
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

/// What this server can do. Declared in one place so `server/discover` and the
/// legacy `initialize` reply can never disagree.
pub fn capabilities() -> Value {
    json!({
        // No `listChanged`: the catalogs are compiled in, so they cannot
        // change while the process is running and there is nothing to notify.
        "tools": { "listChanged": false },
        "prompts": { "listChanged": false },
    })
}

pub fn discover() -> Value {
    json!({
        "supportedVersions": super::jsonrpc::SUPPORTED_VERSIONS,
        "capabilities": capabilities(),
        "instructions": INSTRUCTIONS,
    })
}

/// The legacy `initialize` reply. Same content as [`discover`], shaped the way
/// the handshake era expects it: a single negotiated version, and `serverInfo`
/// at the top level rather than in `_meta`.
pub fn initialize_result(negotiated: &str) -> Value {
    json!({
        "protocolVersion": negotiated,
        "capabilities": capabilities(),
        "serverInfo": server_info(),
        "instructions": INSTRUCTIONS,
    })
}

pub fn tools_list() -> Value {
    json!({ "tools": tools::definitions() })
}

pub fn prompts_list() -> Value {
    json!({ "prompts": prompts::definitions() })
}

pub fn tools_cache() -> CacheHint {
    CacheHint {
        ttl_ms: TOOLS_TTL_MS,
    }
}

pub fn prompts_cache() -> CacheHint {
    CacheHint {
        ttl_ms: PROMPTS_TTL_MS,
    }
}

pub fn discover_cache() -> CacheHint {
    CacheHint {
        ttl_ms: DISCOVER_TTL_MS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_advertises_every_supported_version() {
        let discover = discover();
        assert_eq!(discover["supportedVersions"].as_array().unwrap().len(), 3);
        assert_eq!(
            discover["supportedVersions"][0],
            super::super::jsonrpc::MODERN_VERSION
        );
    }

    #[test]
    fn discovery_and_initialize_declare_the_same_capabilities() {
        assert_eq!(
            discover()["capabilities"],
            initialize_result("2025-11-25")["capabilities"]
        );
    }

    #[test]
    fn instructions_state_the_read_only_boundary_and_the_trust_boundary() {
        // Without the first, models hunt for a `run_eval` tool and hallucinate
        // one. Without the second, stored adversarial output reads as
        // instructions.
        assert!(INSTRUCTIONS.contains("read-only"));
        assert!(INSTRUCTIONS.contains("untrusted"));
        assert!(INSTRUCTIONS.contains("find_runs"));
    }

    #[test]
    fn instructions_stay_within_a_sane_budget() {
        // Sent on every `server/discover`. Not a hard spec limit, just a guard
        // against this file quietly becoming a manual.
        assert!(
            INSTRUCTIONS.len() < 4_000,
            "instructions are {} bytes; trim them",
            INSTRUCTIONS.len()
        );
    }

    #[test]
    fn both_catalogs_are_non_empty() {
        assert_eq!(tools_list()["tools"].as_array().unwrap().len(), 8);
        assert_eq!(prompts_list()["prompts"].as_array().unwrap().len(), 3);
    }
}
