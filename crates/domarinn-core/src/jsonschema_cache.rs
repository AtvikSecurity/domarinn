//! Compiled JSON Schemas, memoized for the life of one run.
//!
//! A schema is compiled once per distinct schema *document*, not once per case
//! × assert. The same `&Assert` is evaluated for every provider × prompt ×
//! repeat cell, so a 500-case suite would otherwise recompile the same document
//! hundreds of times. Keyed on [`crate::cache::canonical_json`], so two asserts
//! that wrote the same schema with different key order share one compilation.
//!
//! Compile *failures* are memoized too — otherwise a malformed schema pays the
//! compile cost on every case just to produce the same error.
//!
//! # Why this is threaded, not global
//!
//! Three alternatives were considered and rejected:
//!
//! - A `OnceLock` process-global memo is the cheapest to write, but there is no
//!   such state anywhere in this crate, it is untestable in isolation, and it
//!   would outlive a run — leaking across the server's in-process runs.
//! - Hanging it off [`crate::template::TemplateEngine`] costs no signature
//!   churn but is a layering violation: templating is not schema validation.
//! - A `#[serde(skip)]` field on `AssertKind` puts a runtime artifact inside
//!   the serialized config type that feeds `config_digest`.
//!
//! So it is a field on [`crate::asserts::EvalCtx`], built per run and dropped
//! with it.
//!
//! # No remote references
//!
//! `jsonschema` is depended on with `default-features = false`, which drops
//! `resolve-http` and `resolve-file`. A `$ref` pointing at a URL is a compile
//! error rather than an outbound request — see
//! `a_remote_ref_fails_to_compile_rather_than_fetching`, which is the test
//! standing between a dependency bump and an SSRF regression.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value as Json;

use crate::assertion::AssertOutcome;

/// How many individual validation errors to report before summarizing.
///
/// A schema mismatch on a large document can produce hundreds; the first few
/// name the problem and the rest are noise in a table cell.
const MAX_REPORTED_ERRORS: usize = 10;

/// A compiled schema, or the reason it would not compile.
type Compiled = Arc<Result<jsonschema::Validator, String>>;

#[derive(Default)]
pub struct SchemaCache {
    compiled: Mutex<HashMap<String, Compiled>>,
}

impl SchemaCache {
    pub fn new() -> Self {
        SchemaCache::default()
    }

    /// The compiled form of `schema`, compiling on first use.
    pub fn get(&self, schema: &Json) -> Compiled {
        let key = crate::cache::canonical_json(schema);
        let mut map = self.compiled.lock().expect("schema cache mutex");
        map.entry(key)
            .or_insert_with(|| {
                Arc::new(jsonschema::validator_for(schema).map_err(|e| e.to_string()))
            })
            .clone()
    }
}

/// Validate `instance` against `schema`, using `cache` to compile once.
pub fn validate_against(instance: &Json, schema: &Json, cache: &SchemaCache) -> AssertOutcome {
    let compiled = cache.get(schema);
    let validator = match compiled.as_ref() {
        Ok(v) => v,
        // Should be unreachable: `validate` rejects an uncompilable schema at
        // load time. Fail closed anyway, matching the `regex` arm's precedent.
        Err(e) => return AssertOutcome::fail(format!("invalid JSON Schema: {e}")),
    };

    let mut errors: Vec<(String, String)> = validator
        .iter_errors(instance)
        .map(|e| (e.instance_path().to_string(), e.to_string()))
        .collect();
    if errors.is_empty() {
        return AssertOutcome::pass("output contains JSON matching the schema");
    }

    // Sorted before formatting. The validator's error order is not a stable
    // contract, and this string is persisted, diffed by `diff.rs`, and
    // full-text indexed by the server — so an unsorted message would show a
    // spurious change between two runs that failed identically.
    errors.sort();

    let (first_path, first_message) = &errors[0];
    let location = if first_path.is_empty() {
        String::new()
    } else {
        format!(" at `{first_path}`")
    };
    let more = match errors.len() - 1 {
        0 => String::new(),
        n => format!(" (and {n} more)"),
    };

    let details = serde_json::json!({
        "errors": errors
            .iter()
            .take(MAX_REPORTED_ERRORS)
            .map(|(path, message)| serde_json::json!({"path": path, "message": message}))
            .collect::<Vec<_>>(),
        "total": errors.len(),
    });

    AssertOutcome::fail(format!(
        "JSON does not match schema{location}: {first_message}{more}"
    ))
    .with_details(details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_matching_document_passes_and_a_mismatching_one_fails() {
        let cache = SchemaCache::new();
        let schema = json!({"type": "object", "required": ["age"],
                            "properties": {"age": {"type": "integer"}}});
        assert!(validate_against(&json!({"age": 7}), &schema, &cache).passed);
        assert!(!validate_against(&json!({"age": "seven"}), &schema, &cache).passed);
        assert!(!validate_against(&json!({}), &schema, &cache).passed);
    }

    #[test]
    fn a_failure_names_the_instance_path() {
        let cache = SchemaCache::new();
        let schema = json!({
            "type": "object",
            "properties": {"items": {"type": "array", "items": {"type": "integer"}}}
        });
        let outcome = validate_against(&json!({"items": [1, "x"]}), &schema, &cache);
        assert!(!outcome.passed);
        assert!(
            outcome.reason.contains("/items/1"),
            "reason should locate the failure, got: {}",
            outcome.reason
        );
        assert!(outcome.details.is_some(), "structured errors are attached");
    }

    /// The reason string is persisted, diffed, and FTS-indexed, so two runs
    /// that failed identically must produce byte-identical text.
    #[test]
    fn a_failure_reason_is_deterministic_across_evaluations() {
        let cache = SchemaCache::new();
        let schema = json!({
            "type": "object",
            "properties": {
                "a": {"type": "integer"}, "b": {"type": "integer"},
                "c": {"type": "integer"}, "d": {"type": "integer"}
            }
        });
        let bad = json!({"a": "x", "b": "x", "c": "x", "d": "x"});
        let first = validate_against(&bad, &schema, &cache).reason;
        for _ in 0..20 {
            assert_eq!(
                validate_against(&bad, &schema, &SchemaCache::new()).reason,
                first
            );
        }
    }

    #[test]
    fn the_same_schema_compiles_once() {
        let cache = SchemaCache::new();
        let schema = json!({"type": "object"});
        let a = cache.get(&schema);
        let b = cache.get(&schema);
        assert!(Arc::ptr_eq(&a, &b), "the compiled schema must be reused");
    }

    /// Canonical-JSON keying, so two asserts that wrote the same schema with
    /// their keys in a different order share one compilation.
    #[test]
    fn key_order_does_not_defeat_the_memo() {
        let cache = SchemaCache::new();
        let a = cache.get(&json!({"type": "object", "title": "t"}));
        let b = cache.get(&json!({"title": "t", "type": "object"}));
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn a_compile_failure_is_memoized_and_fails_closed() {
        let cache = SchemaCache::new();
        let broken = json!({"type": "not-a-real-type"});
        let a = cache.get(&broken);
        let b = cache.get(&broken);
        assert!(Arc::ptr_eq(&a, &b));
        assert!(a.is_err());
        let outcome = validate_against(&json!({}), &broken, &cache);
        assert!(!outcome.passed);
        assert!(outcome.reason.contains("invalid JSON Schema"));
    }

    /// The guard that keeps a suite's `$ref` from becoming an outbound request.
    /// If this ever starts passing by *fetching*, the dependency's features
    /// changed and `default-features = false` is no longer buying what its
    /// comment in Cargo.toml claims.
    #[test]
    fn a_remote_ref_fails_to_compile_rather_than_fetching() {
        let cache = SchemaCache::new();
        let remote = json!({"$ref": "https://example.invalid/schema.json"});
        let compiled = cache.get(&remote);
        assert!(
            compiled.is_err(),
            "a remote $ref must not resolve; resolve-http is supposed to be off"
        );
    }
}
