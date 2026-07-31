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
fn test_case_matrix_fields_are_in_the_schema() {
    // Matrix / parameter sweeps add `matrix` and `matrix_id` to a test case; the
    // published config schema must advertise both so editors complete them.
    let schema = schema_text();
    for field in ["matrix", "matrix_id"] {
        assert!(
            schema.contains(field),
            "test-case field '{field}' missing from config schema"
        );
    }
}

#[test]
fn test_case_and_defaults_expose_cache_salt() {
    // A substring check would pass vacuously: "cache_salt" already appears in the
    // schema for the `exec` provider. Assert against the specific definitions so
    // this actually guards the per-case field.
    let schema = serde_json::to_value(domarinn_core::config_schema()).unwrap();
    let defs = schema.get("$defs").expect("schema carries a $defs block");
    for def in ["TestCase", "Defaults"] {
        assert!(
            defs.pointer(&format!("/{def}/properties/cache_salt"))
                .is_some(),
            "{def} must advertise `cache_salt` in the config schema"
        );
    }
}

#[test]
fn test_case_and_defaults_expose_history() {
    // Same pointer style as `cache_salt` above: "history" appears elsewhere in
    // the schema (the marker enum), so a substring check would pass vacuously.
    let schema = serde_json::to_value(domarinn_core::config_schema()).unwrap();
    let defs = schema.get("$defs").expect("schema carries a $defs block");
    for def in ["TestCase", "Defaults"] {
        assert!(
            defs.pointer(&format!("/{def}/properties/history"))
                .is_some(),
            "{def} must advertise `history` in the config schema"
        );
    }
}

#[test]
fn prompt_messages_entries_advertise_the_history_marker() {
    // A `messages:` entry is a turn or the bare `history` marker; editors only
    // learn that if the schema names both alternatives.
    let schema = serde_json::to_value(domarinn_core::config_schema()).unwrap();
    let defs = schema.get("$defs").expect("schema carries a $defs block");
    assert!(
        defs.pointer("/PromptEntry/anyOf").is_some(),
        "PromptEntry must be a marker-or-turn alternative"
    );
    let marker = defs
        .pointer("/HistoryMarker")
        .expect("HistoryMarker definition present");
    assert!(
        serde_json::to_string(marker).unwrap().contains("history"),
        "the marker's schema must pin the literal string: {marker}"
    );
}

#[test]
fn result_schema_is_versioned() {
    let schema = serde_json::to_string(&domarinn_core::result_schema()).unwrap();
    assert!(schema.contains("schema_version"));
    assert_eq!(domarinn_core::RESULT_SCHEMA_VERSION, 2);
}

#[test]
fn result_schema_exposes_prompt_stop_reason_and_raw() {
    // v2 persists the rendered prompt, the provider stop_reason, and raw
    // provider metadata per case; the published schema must advertise all three.
    let schema = serde_json::to_string(&domarinn_core::result_schema()).unwrap();
    for field in ["\"prompt\"", "\"stop_reason\"", "\"raw\""] {
        assert!(
            schema.contains(field),
            "result JSON Schema must expose the {field} field"
        );
    }
}

#[test]
fn exports_typescript_definitions() {
    let dir = tempfile::tempdir().unwrap();
    domarinn_core::export_types(dir.path()).unwrap();
    assert!(dir.path().join("RunResult.ts").exists());
    assert!(dir.path().join("RunDiff.ts").exists());
}
