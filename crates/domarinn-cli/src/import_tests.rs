use super::*;

fn run(src: Json) -> (Json, Vec<String>) {
    let mut notes = Vec::new();
    let suite = convert(&src, &mut notes);
    (suite, notes)
}

fn asserts_of(src_asserts: Json) -> (Vec<Json>, Vec<String>) {
    let (suite, notes) = run(json!({
        "providers": ["openai:gpt-4o"],
        "tests": [{"assert": src_asserts}]
    }));
    let asserts = suite["tests"][0]["assert"].as_array().unwrap().clone();
    (asserts, notes)
}

#[test]
fn maps_providers_and_asserts() {
    let (suite, _) = run(json!({
        "providers": ["openai:gpt-4o", "anthropic:messages:claude-3-5-sonnet"],
        "prompts": ["Answer: {{ q }}"],
        "tests": [
            {"vars": {"q": "hi"}, "assert": [
                {"type": "contains", "value": "hello"},
                {"type": "llm-rubric", "value": "is polite", "weight": 3}
            ]}
        ]
    }));
    let providers = suite["providers"].as_array().unwrap();
    assert_eq!(providers[0]["type"], "openai");
    assert_eq!(providers[0]["model"], "gpt-4o");
    assert_eq!(providers[1]["type"], "anthropic");
    assert_eq!(providers[1]["model"], "claude-3-5-sonnet");
    let asserts = suite["tests"][0]["assert"].as_array().unwrap();
    assert_eq!(asserts[0]["type"], "contains");
    assert_eq!(asserts[1]["type"], "llm-rubric");
    assert_eq!(asserts[1]["weight"], 3);
}

#[test]
fn unmappable_assert_produces_note_not_output() {
    let (asserts, notes) =
        asserts_of(json!([{"type": "javascript", "value": "output.length > 3"}]));
    assert!(asserts.is_empty());
    assert!(notes.iter().any(|n| n.contains("javascript")));
}

/// Finding 1: promptfoo spells a budget `threshold`, and domarinn requires `max`.
#[test]
fn latency_and_cost_threshold_becomes_max() {
    let (asserts, notes) = asserts_of(json!([
        {"type": "latency", "threshold": 1500},
        {"type": "cost", "threshold": 0.002},
        {"type": "latency", "value": 900},
    ]));
    assert_eq!(asserts[0], json!({"type": "latency", "max": 1500}));
    assert_eq!(asserts[1], json!({"type": "cost", "max": 0.002}));
    assert_eq!(asserts[2], json!({"type": "latency", "max": 900}));
    assert!(notes.is_empty(), "{notes:?}");
}

/// Finding 2: `contains-any` is case-sensitive in promptfoo, so it must not
/// silently become `icontains-any`.
#[test]
fn contains_any_stays_case_sensitive() {
    let (asserts, notes) = asserts_of(json!([
        {"type": "contains-any", "value": ["a.b", "C(d)"]},
        {"type": "icontains-any", "value": ["x", "y"]},
        {"type": "contains-any", "value": "p, q"},
    ]));
    assert_eq!(
        asserts[0],
        json!({"type": "regex", "value": r"a\.b|C\(d\)"})
    );
    assert_eq!(
        asserts[1],
        json!({"type": "icontains-any", "values": ["x", "y"]})
    );
    assert_eq!(asserts[2], json!({"type": "regex", "value": "p|q"}));
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn contains_all_expands_and_splits_weight() {
    let (asserts, _) = asserts_of(json!([
        {"type": "icontains-all", "value": ["a", "b"], "weight": 2}
    ]));
    assert_eq!(
        asserts,
        vec![
            json!({"type": "icontains", "value": "a", "weight": 1.0}),
            json!({"type": "icontains", "value": "b", "weight": 1.0}),
        ]
    );
}

/// Finding 3: `not-<kind>` is domarinn sugar too, so it maps instead of
/// being skipped.
#[test]
fn not_prefixed_asserts_map_to_not_sugar() {
    let (asserts, notes) = asserts_of(json!([
        {"type": "not-icontains", "value": "as an AI"},
        {"type": "not-contains-any", "value": ["x", "y"]},
        {"type": "not-contains-all", "value": ["x", "y"]},
        {"type": "not-javascript", "value": "true"},
    ]));
    assert_eq!(
        asserts,
        vec![
            json!({"type": "not-icontains", "value": "as an AI"}),
            json!({"type": "not-regex", "value": "x|y"}),
        ]
    );
    assert!(notes.iter().any(|n| n.contains("not-contains-all")));
    assert!(notes.iter().any(|n| n.contains("not-javascript")));
}

/// Finding 4: a `tests: file://...` string is a test source, not nothing.
#[test]
fn file_tests_are_kept_with_a_note() {
    let (suite, notes) = run(json!({
        "providers": ["openai:gpt-4o"],
        "tests": "file://tests.csv"
    }));
    assert_eq!(suite["tests"], json!(["file://tests.csv"]));
    assert!(
        notes
            .iter()
            .any(|n| n.contains("file://tests.csv") && n.contains("__expected")),
        "{notes:?}"
    );

    let (suite, notes) = run(json!({
        "providers": ["openai:gpt-4o"],
        "tests": ["file://gen.py:make_tests"]
    }));
    assert_eq!(suite["tests"], json!([]));
    assert!(notes.iter().any(|n| n.contains("generator")), "{notes:?}");
}

/// Finding 5: provider `config:` maps to `params` and connection fields.
#[test]
fn provider_config_maps_to_params() {
    let (suite, notes) = run(json!({
        "providers": [{
            "id": "openai:chat:gpt-4o-mini",
            "label": "mini",
            "config": {
                "temperature": 0,
                "max_tokens": 256,
                "apiBaseUrl": "https://gw.example/v1",
                "apiKeyEnvar": "GW_KEY",
                "functionToolCallbacks": {}
            }
        }]
    }));
    let p = &suite["providers"][0];
    assert_eq!(p["type"], "openai");
    assert_eq!(p["model"], "gpt-4o-mini");
    assert_eq!(p["label"], "mini");
    assert_eq!(p["params"], json!({"temperature": 0, "max_tokens": 256}));
    assert_eq!(p["base_url"], "https://gw.example/v1");
    assert_eq!(p["api_key_env"], "GW_KEY");
    assert!(
        notes.iter().any(|n| n.contains("functionToolCallbacks")),
        "{notes:?}"
    );
    assert!(!notes.iter().any(|n| n.contains("temperature")));
}

/// Finding 6: `ollama:` goes through Ollama's OpenAI-compatible endpoint, and
/// a model tag containing `:` survives.
#[test]
fn ollama_maps_to_openai_compatible_endpoint() {
    let (suite, notes) = run(json!({
        "providers": [
            "ollama:chat:llama3:8b",
            {"id": "ollama:qwen3:4b", "config": {"num_predict": 64}}
        ]
    }));
    let p0 = &suite["providers"][0];
    assert_eq!(p0["type"], "openai");
    assert_eq!(p0["model"], "llama3:8b");
    assert_eq!(p0["base_url"], OLLAMA_BASE_URL);
    assert_eq!(p0["request"], json!({"auth": "none"}));
    let p1 = &suite["providers"][1];
    assert_eq!(p1["model"], "qwen3:4b");
    assert_eq!(p1["params"], json!({"max_tokens": 64}));
    assert!(notes.is_empty(), "{notes:?}");
}

#[test]
fn array_vars_become_a_matrix() {
    let (suite, _) = run(json!({
        "providers": ["openai:gpt-4o"],
        "tests": [{"vars": {"q": "hi", "lang": ["en", "fr"]}}]
    }));
    let tc = &suite["tests"][0];
    assert_eq!(tc["vars"], json!({"q": "hi"}));
    assert_eq!(tc["matrix"], json!({"lang": ["en", "fr"]}));
}

#[test]
fn unconverted_keys_are_noted() {
    let (_, notes) = run(json!({
        "providers": ["openai:gpt-4o"],
        "outputPath": "out.json",
        "tests": [{
            "options": {"provider": "openai:gpt-4o"},
            "assert": [{"type": "contains", "value": "x", "transform": "output.trim()"}]
        }]
    }));
    for needle in ["outputPath", "options", "transform"] {
        assert!(
            notes.iter().any(|n| n.contains(needle)),
            "{needle}: {notes:?}"
        );
    }
}

#[test]
fn object_prompts_are_converted_or_noted() {
    let (suite, notes) = run(json!({
        "providers": ["openai:gpt-4o"],
        "prompts": [{"raw": "Hi {{ q }}", "label": "x"}, {"id": "file://p.txt"}, {"label": "y"}]
    }));
    let prompts = suite["prompts"].as_array().unwrap();
    assert_eq!(prompts[0]["template"], "Hi {{ q }}");
    assert_eq!(prompts[1]["template"], "file://p.txt");
    assert_eq!(prompts.len(), 2);
    assert!(notes.iter().any(|n| n.contains("prompt #2")), "{notes:?}");
}
