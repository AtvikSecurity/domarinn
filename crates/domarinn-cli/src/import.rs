//! `domarinn import promptfoo <PATH>`: translate a promptfoo config into a
//! domarinn suite. Mappable constructs are converted; anything without a
//! faithful equivalent is emitted as a commented note so nothing is silently
//! dropped.

use std::path::PathBuf;

use serde_json::{json, Value as Json};

use crate::exit;

pub fn execute(path: PathBuf) -> u8 {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: reading {}: {e}", path.display());
            return exit::USAGE;
        }
    };
    let src: Json = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text)
        .and_then(|v| serde_json::to_value(v).map_err(serde::de::Error::custom))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: parsing promptfoo config: {e}");
            return exit::USAGE;
        }
    };

    let mut notes: Vec<String> = Vec::new();
    let suite = convert(&src, &mut notes);

    match serde_yaml_ng::to_string(&suite) {
        Ok(yaml) => {
            println!("# Converted from a promptfoo config by `domarinn import promptfoo`.");
            println!("# yaml-language-server: $schema=./domarinn.schema.json");
            for note in &notes {
                println!("# NOTE: {note}");
            }
            print!("{yaml}");
            exit::OK
        }
        Err(e) => {
            eprintln!("error: emitting YAML: {e}");
            exit::INFRA
        }
    }
}

fn convert(src: &Json, notes: &mut Vec<String>) -> Json {
    let mut suite = serde_json::Map::new();
    suite.insert("version".into(), json!(1));
    if let Some(desc) = src.get("description").and_then(|d| d.as_str()) {
        suite.insert("description".into(), json!(desc));
    }

    let providers = convert_providers(src.get("providers"), notes);
    suite.insert("providers".into(), json!(providers));

    let prompts = convert_prompts(src.get("prompts"), notes);
    if !prompts.is_empty() {
        suite.insert("prompts".into(), json!(prompts));
    }

    if let Some(default_test) = src.get("defaultTest") {
        if let Some(defaults) = convert_default_test(default_test, notes) {
            suite.insert("defaults".into(), defaults);
        }
    }

    let tests = convert_tests(src.get("tests"), notes);
    suite.insert("tests".into(), json!(tests));

    Json::Object(suite)
}

fn convert_providers(providers: Option<&Json>, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let list = match providers.and_then(|p| p.as_array()) {
        Some(l) => l,
        None => {
            notes.push("no providers found; add at least one".into());
            return out;
        }
    };
    for (i, p) in list.iter().enumerate() {
        if let Some(s) = p.as_str() {
            out.push(provider_from_string(s, i, notes));
        } else if let Some(obj) = p.as_object() {
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| format!("provider-{i}"));
            out.push(provider_from_string(&id, i, notes));
            notes.push(format!(
                "provider '{id}' had extra config; review model/params after import"
            ));
        }
    }
    out
}

/// Map a promptfoo provider string like `openai:gpt-4o` or
/// `anthropic:messages:claude-3-5-sonnet` to a domarinn provider.
fn provider_from_string(spec: &str, index: usize, notes: &mut Vec<String>) -> Json {
    let parts: Vec<&str> = spec.split(':').collect();
    let id = format!("p{index}");
    match parts.as_slice() {
        ["openai", rest @ ..] => {
            let model = rest.last().copied().unwrap_or("gpt-4o");
            json!({"id": id, "type": "openai", "model": model})
        }
        ["anthropic", rest @ ..] => {
            let model = rest.last().copied().unwrap_or("claude-3-5-sonnet-latest");
            json!({"id": id, "type": "anthropic", "model": model})
        }
        ["exec", cmd] | ["file", cmd] => {
            json!({"id": id, "type": "exec", "command": [cmd]})
        }
        ["http", ..] | ["https", ..] => {
            json!({"id": id, "type": "http", "url": spec})
        }
        _ => {
            notes.push(format!(
                "provider '{spec}' could not be mapped; defaulting to an exec provider — edit it"
            ));
            json!({"id": id, "type": "exec", "command": [spec]})
        }
    }
}

fn convert_prompts(prompts: Option<&Json>, _notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    if let Some(list) = prompts.and_then(|p| p.as_array()) {
        for (i, p) in list.iter().enumerate() {
            if let Some(s) = p.as_str() {
                out.push(json!({"id": format!("prompt-{i}"), "template": s}));
            }
        }
    }
    out
}

fn convert_default_test(default_test: &Json, notes: &mut Vec<String>) -> Option<Json> {
    let mut defaults = serde_json::Map::new();
    if let Some(vars) = default_test.get("vars").and_then(|v| v.as_object()) {
        defaults.insert("vars".into(), json!(vars));
    }
    if let Some(asserts) = default_test.get("assert") {
        let converted = convert_asserts(asserts, notes);
        if !converted.is_empty() {
            defaults.insert("assert".into(), json!(converted));
        }
    }
    (!defaults.is_empty()).then_some(Json::Object(defaults))
}

fn convert_tests(tests: Option<&Json>, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let list = match tests.and_then(|t| t.as_array()) {
        Some(l) => l,
        None => return out,
    };
    for (i, t) in list.iter().enumerate() {
        if let Some(s) = t.as_str() {
            // A file:// glob of test cases.
            out.push(json!(s));
            continue;
        }
        let mut tc = serde_json::Map::new();
        if let Some(desc) = t.get("description").and_then(|d| d.as_str()) {
            tc.insert("description".into(), json!(desc));
        } else {
            tc.insert("id".into(), json!(format!("case-{i}")));
        }
        if let Some(vars) = t.get("vars").and_then(|v| v.as_object()) {
            tc.insert("vars".into(), json!(vars));
        }
        if let Some(asserts) = t.get("assert") {
            tc.insert("assert".into(), json!(convert_asserts(asserts, notes)));
        }
        out.push(Json::Object(tc));
    }
    out
}

fn convert_asserts(asserts: &Json, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let list = match asserts.as_array() {
        Some(l) => l,
        None => return out,
    };
    for a in list {
        if let Some(converted) = convert_assert(a, notes) {
            out.push(converted);
        }
    }
    out
}

fn convert_assert(a: &Json, notes: &mut Vec<String>) -> Option<Json> {
    let ty = a.get("type").and_then(|t| t.as_str())?;
    let value = a.get("value");
    let weight = a.get("weight");
    let threshold = a.get("threshold");

    let mut m = serde_json::Map::new();
    let mapped_type = match ty {
        "contains" | "icontains" | "starts-with" | "regex" | "equals" | "is-json"
        | "contains-json" | "llm-rubric" | "similar" => ty,
        "contains-any" | "icontains-any" => {
            m.insert("type".into(), json!("icontains-any"));
            if let Some(v) = value {
                m.insert("values".into(), v.clone());
            }
            return finalize(m, weight, threshold);
        }
        "cost" => "cost",
        "latency" => "latency",
        "javascript" | "python" | "webhook" => {
            notes.push(format!(
                "'{ty}' assertion has no direct equivalent; rewrite as an `exec` assertion"
            ));
            return None;
        }
        other => {
            notes.push(format!(
                "assertion type '{other}' was skipped (no equivalent)"
            ));
            return None;
        }
    };
    m.insert("type".into(), json!(mapped_type));
    if let Some(v) = value {
        match mapped_type {
            "cost" | "latency" => {
                m.insert("max".into(), v.clone());
            }
            _ => {
                m.insert("value".into(), v.clone());
            }
        }
    }
    finalize(m, weight, threshold)
}

fn finalize(
    mut m: serde_json::Map<String, Json>,
    weight: Option<&Json>,
    threshold: Option<&Json>,
) -> Option<Json> {
    if let Some(w) = weight {
        m.insert("weight".into(), w.clone());
    }
    if let Some(t) = threshold {
        m.insert("threshold".into(), t.clone());
    }
    Some(Json::Object(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_providers_and_asserts() {
        let src = json!({
            "providers": ["openai:gpt-4o", "anthropic:messages:claude-3-5-sonnet"],
            "prompts": ["Answer: {{ q }}"],
            "tests": [
                {"vars": {"q": "hi"}, "assert": [
                    {"type": "contains", "value": "hello"},
                    {"type": "llm-rubric", "value": "is polite", "weight": 3}
                ]}
            ]
        });
        let mut notes = Vec::new();
        let suite = convert(&src, &mut notes);
        let providers = suite["providers"].as_array().unwrap();
        assert_eq!(providers[0]["type"], "openai");
        assert_eq!(providers[0]["model"], "gpt-4o");
        assert_eq!(providers[1]["type"], "anthropic");
        let asserts = suite["tests"][0]["assert"].as_array().unwrap();
        assert_eq!(asserts[0]["type"], "contains");
        assert_eq!(asserts[1]["type"], "llm-rubric");
        assert_eq!(asserts[1]["weight"], 3);
    }

    #[test]
    fn unmappable_assert_produces_note_not_output() {
        let src = json!({
            "providers": ["openai:gpt-4o"],
            "tests": [{"assert": [{"type": "javascript", "value": "output.length > 3"}]}]
        });
        let mut notes = Vec::new();
        let suite = convert(&src, &mut notes);
        assert!(suite["tests"][0]["assert"].as_array().unwrap().is_empty());
        assert!(notes.iter().any(|n| n.contains("javascript")));
    }
}
