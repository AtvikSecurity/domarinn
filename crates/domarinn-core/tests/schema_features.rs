//! Guard that the published JSON Schema exposes every provider and assertion
//! type. Catches an accidental drop of a feature from the config surface.

fn schema_text() -> String {
    serde_json::to_string(&domarinn_core::config_schema()).unwrap()
}

#[test]
fn all_provider_types_are_in_the_schema() {
    let schema = schema_text();
    for provider in ["exec", "anthropic", "openai", "http", "embeddings"] {
        assert!(
            schema.contains(provider),
            "provider type '{provider}' missing from config schema"
        );
    }
}

#[test]
fn all_assertion_types_are_in_the_schema() {
    let schema = schema_text();
    for kind in [
        "contains",
        "icontains",
        "icontains-any",
        "regex",
        "equals",
        "starts-with",
        "is-json",
        "contains-json",
        "length",
        "jinja",
        "exec",
        "llm-rubric",
        "cost",
        "latency",
        "tokens",
        "similar",
    ] {
        assert!(
            schema.contains(kind),
            "assertion type '{kind}' missing from config schema"
        );
    }
}

#[test]
fn result_schema_is_versioned() {
    let schema = serde_json::to_string(&domarinn_core::result_schema()).unwrap();
    assert!(schema.contains("schema_version"));
    assert_eq!(domarinn_core::RESULT_SCHEMA_VERSION, 1);
}

#[test]
fn exports_typescript_definitions() {
    let dir = tempfile::tempdir().unwrap();
    domarinn_core::export_types(dir.path()).unwrap();
    assert!(dir.path().join("RunResult.ts").exists());
    assert!(dir.path().join("RunDiff.ts").exists());
}
