//! Computing the cache key for a provider call.
//!
//! The key hashes the provider fingerprint, the rendered request, and the repeat
//! index, canonically (see [`crate::cache::canonical_json`]). `repeat` is always
//! present (default 0) so N=1 and repeat-0 of N=3 agree.

use serde_json::Value as Json;

use crate::cache::CacheKey;
use crate::provider::ProviderRequest;

/// Compute the cache key for one provider call.
pub fn provider_cache_key(fingerprint: &Json, req: &ProviderRequest, repeat: u32) -> CacheKey {
    let prompt = req
        .prompt
        .as_ref()
        .map(|p| serde_json::to_value(p).unwrap_or(Json::Null))
        .unwrap_or(Json::Null);
    let parts = serde_json::json!({
        "fingerprint": fingerprint,
        "prompt": prompt,
        "vars": Json::Object(req.vars.clone().into_iter().collect()),
        "params": Json::Object(req.params.clone()),
        "repeat": repeat,
    });
    CacheKey::compute(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use std::collections::BTreeMap;

    fn req(var: &str) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert("x".to_string(), Json::String(var.into()));
        ProviderRequest {
            prompt: None,
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
        }
    }

    #[test]
    fn key_is_stable_for_same_inputs() {
        let fp = serde_json::json!({"type": "exec"});
        assert_eq!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("a"), 0)
        );
    }

    #[test]
    fn key_changes_with_vars_and_repeat() {
        let fp = serde_json::json!({"type": "exec"});
        assert_ne!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("b"), 0)
        );
        assert_ne!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("a"), 1)
        );
    }
}
