//! `domarinn import promptfoo <PATH>`: translate a promptfoo config into a
//! domarinn suite. Mappable constructs are converted; anything without a
//! faithful equivalent is emitted as a commented note so nothing is silently
//! dropped.

use std::path::PathBuf;

use serde_json::{json, Map, Value as Json};

use crate::exit;

type Obj = Map<String, Json>;

pub fn execute(path: PathBuf) -> u8 {
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: reading {}: {e}", path.display());
            return exit::USAGE;
        }
    };
    let src: Json = match domarinn_core::yaml::parse_value(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: parsing promptfoo config: {e}");
            return exit::USAGE;
        }
    };

    let mut notes: Vec<String> = Vec::new();
    let suite = convert(&src, &mut notes);

    match to_yaml(&suite) {
        Ok(yaml) => {
            println!("# Converted from a promptfoo config by `domarinn import promptfoo`.");
            println!(
                "# yaml-language-server: $schema={}",
                domarinn_core::CONFIG_SCHEMA_ID
            );
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

/// Emit `v` as YAML. serde-saphyr folds single-line strings longer than 80
/// characters into `>-` blocks by default; turn that off so a long prompt or
/// rubric stays on one line, as the user wrote it.
fn to_yaml<T: serde::Serialize>(v: &T) -> Result<String, serde_saphyr::SerializeError> {
    // `SerializerOptions` is `#[non_exhaustive]`: mutate a default, don't build a literal.
    let mut o = serde_saphyr::SerializerOptions::default();
    o.folded_wrap_chars = usize::MAX;
    serde_saphyr::to_string_with_options(v, o)
}

/// Top-level promptfoo keys the converter reads. Anything else is reported.
const TOP_LEVEL_KEYS: &[&str] = &[
    "description",
    "providers",
    "prompts",
    "defaultTest",
    "tests",
    // Editor hint; the converter writes its own.
    "$schema",
];

fn convert(src: &Json, notes: &mut Vec<String>) -> Json {
    let mut suite = Obj::new();
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

    if let Some(obj) = src.as_object() {
        let extra = unknown_keys(obj, TOP_LEVEL_KEYS);
        if !extra.is_empty() {
            notes.push(format!(
                "top-level key(s) {extra} have no domarinn equivalent and were not converted"
            ));
        }
    }

    Json::Object(suite)
}

/// The keys of `obj` not in `known`, formatted for a note (empty when none).
fn unknown_keys(obj: &Obj, known: &[&str]) -> String {
    let extra: Vec<String> = obj
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .map(|k| format!("'{k}'"))
        .collect();
    extra.join(", ")
}

/// promptfoo accepts a single value wherever it accepts a list.
fn as_list(v: Option<&Json>) -> Option<Vec<&Json>> {
    match v? {
        Json::Array(items) => Some(items.iter().collect()),
        Json::Null => None,
        other => Some(vec![other]),
    }
}

// ---------------------------------------------------------------- providers

/// `config:` keys passed through to a vendor provider's `params` verbatim.
/// These are request-body fields with the same name and meaning on both sides.
const PARAM_KEYS: &[&str] = &[
    "temperature",
    "max_tokens",
    "max_completion_tokens",
    "top_p",
    "top_k",
    "frequency_penalty",
    "presence_penalty",
    "stop",
    "seed",
    "response_format",
    "reasoning_effort",
];

/// Where Ollama serves its OpenAI-compatible API. See docs/guides/local-llms.md.
const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

fn convert_providers(providers: Option<&Json>, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let Some(list) = as_list(providers) else {
        notes.push("no providers found; add at least one".into());
        return out;
    };
    let empty = Obj::new();
    for (i, p) in list.into_iter().enumerate() {
        let id = format!("p{i}");
        if let Some(spec) = p.as_str() {
            out.push(Json::Object(provider_from_spec(&id, spec, notes)));
            continue;
        }
        let Some(obj) = p.as_object() else {
            notes.push(format!(
                "provider #{i} is not a string or a mapping; skipped"
            ));
            continue;
        };
        // `{id: openai:gpt-4o, config: ...}`, or the map form
        // `{"openai:gpt-4o": {config: ...}}`.
        let (spec, opts) = match obj.get("id").and_then(|v| v.as_str()) {
            Some(spec) => (spec.to_string(), obj),
            None if obj.len() == 1 => {
                let (k, v) = obj.iter().next().expect("len 1");
                (k.clone(), v.as_object().unwrap_or(&empty))
            }
            None => {
                notes.push(format!(
                    "provider #{i} has no `id`; skipped — add it by hand"
                ));
                continue;
            }
        };
        let mut m = provider_from_spec(&id, &spec, notes);
        if let Some(label) = opts.get("label").and_then(|v| v.as_str()) {
            m.insert("label".into(), json!(label));
        }
        if let Some(cfg) = opts.get("config").and_then(|v| v.as_object()) {
            apply_provider_config(&mut m, &spec, cfg, notes);
        }
        let extra = unknown_keys(opts, &["id", "label", "config"]);
        if !extra.is_empty() {
            notes.push(format!(
                "provider '{spec}': key(s) {extra} were not converted"
            ));
        }
        out.push(Json::Object(m));
    }
    out
}

/// Split an optional leading mode (`chat:`, `messages:`, …) off a provider
/// spec's remainder. Model names may themselves contain `:` (`llama3:8b`), so
/// only a recognized first segment is treated as a mode.
fn split_mode<'a>(rest: &'a str, modes: &[&str]) -> (Option<&'a str>, &'a str) {
    match rest.split_once(':') {
        Some((head, tail)) if modes.contains(&head) => (Some(head), tail),
        _ if modes.contains(&rest) => (Some(rest), ""),
        _ => (None, rest),
    }
}

fn non_empty<'a>(s: &'a str, default: &'a str) -> &'a str {
    if s.is_empty() {
        default
    } else {
        s
    }
}

/// Map a promptfoo provider string like `openai:gpt-4o`,
/// `anthropic:messages:claude-3-5-sonnet` or `ollama:chat:llama3:8b` to a
/// domarinn provider.
fn provider_from_spec(id: &str, spec: &str, notes: &mut Vec<String>) -> Obj {
    let (vendor, rest) = spec.split_once(':').unwrap_or((spec, ""));
    let mut m = Obj::new();
    m.insert("id".into(), json!(id));
    match vendor {
        "openai" => {
            let modes = ["chat", "completion", "responses", "embedding", "embeddings"];
            let (mode, model) = split_mode(rest, &modes);
            let model = non_empty(model, "gpt-4o");
            match mode {
                Some("embedding" | "embeddings") => {
                    m.insert("type".into(), json!("embeddings"));
                }
                Some(mode @ ("completion" | "responses")) => {
                    notes.push(format!(
                        "provider '{spec}': the `{mode}` API was mapped to chat-completions \
                         (`type: openai`), the only OpenAI API domarinn speaks"
                    ));
                    m.insert("type".into(), json!("openai"));
                }
                _ => {
                    m.insert("type".into(), json!("openai"));
                }
            }
            m.insert("model".into(), json!(model));
        }
        "anthropic" => {
            let (mode, model) = split_mode(rest, &["messages", "completion"]);
            if mode == Some("completion") {
                notes.push(format!(
                    "provider '{spec}': the legacy completion API was mapped to the Messages API"
                ));
            }
            m.insert("type".into(), json!("anthropic"));
            m.insert(
                "model".into(),
                json!(non_empty(model, "claude-3-5-sonnet-latest")),
            );
        }
        "ollama" => {
            let modes = ["chat", "completion", "embedding", "embeddings"];
            let (mode, model) = split_mode(rest, &modes);
            let kind = match mode {
                Some("embedding" | "embeddings") => "embeddings",
                Some("completion") => {
                    notes.push(format!(
                        "provider '{spec}': Ollama's completion API was mapped to its \
                         OpenAI-compatible chat endpoint"
                    ));
                    "openai"
                }
                _ => "openai",
            };
            m.insert("type".into(), json!(kind));
            m.insert("model".into(), json!(non_empty(model, "llama3")));
            m.insert("base_url".into(), json!(OLLAMA_BASE_URL));
            // Ollama ignores credentials; `auth: none` also exempts the
            // provider from the credential preflight.
            m.insert("request".into(), json!({"auth": "none"}));
        }
        "exec" => {
            let command: Vec<&str> = rest.split_whitespace().collect();
            if rest.contains(['"', '\'']) {
                notes.push(format!(
                    "provider '{spec}': the command was split on whitespace; check its quoting"
                ));
            }
            m.insert("type".into(), json!("exec"));
            m.insert("command".into(), json!(command));
        }
        "file" => {
            let path = rest.trim_start_matches("//");
            notes.push(format!(
                "provider '{spec}' is a promptfoo module provider; domarinn runs no \
                 in-process code — wrap it in a program speaking the exec protocol"
            ));
            m.insert("type".into(), json!("exec"));
            m.insert("command".into(), json!([path]));
        }
        "http" | "https" => {
            m.insert("type".into(), json!("http"));
            m.insert("url".into(), json!(spec));
        }
        _ => {
            notes.push(format!(
                "provider '{spec}' could not be mapped; defaulting to an exec provider — edit it"
            ));
            m.insert("type".into(), json!("exec"));
            m.insert("command".into(), json!([spec]));
        }
    }
    m
}

/// Map a promptfoo provider `config:` block onto a converted provider.
fn apply_provider_config(m: &mut Obj, spec: &str, cfg: &Obj, notes: &mut Vec<String>) {
    let ty = m.get("type").and_then(|t| t.as_str()).unwrap_or_default();
    if !matches!(ty, "openai" | "anthropic" | "embeddings") {
        let keys = unknown_keys(cfg, &[]);
        if !keys.is_empty() {
            notes.push(format!(
                "provider '{spec}': config key(s) {keys} were not converted"
            ));
        }
        return;
    }
    let mut params = Obj::new();
    let mut leftover: Vec<String> = Vec::new();
    for (k, v) in cfg {
        match k.as_str() {
            k if PARAM_KEYS.contains(&k) => {
                params.insert(k.into(), v.clone());
            }
            // Ollama's own name for the output-token cap.
            "num_predict" if spec.starts_with("ollama:") => {
                params.insert("max_tokens".into(), v.clone());
            }
            "apiBaseUrl" if v.is_string() => {
                m.insert("base_url".into(), v.clone());
            }
            "apiKeyEnvar" if v.is_string() => {
                m.insert("api_key_env".into(), v.clone());
            }
            "apiKey" => notes.push(format!(
                "provider '{spec}': a literal `apiKey` was not copied; name the variable \
                 holding it with `api_key_env`"
            )),
            _ => leftover.push(format!("'{k}'")),
        }
    }
    if !params.is_empty() {
        m.insert("params".into(), Json::Object(params));
    }
    if !leftover.is_empty() {
        notes.push(format!(
            "provider '{spec}': config key(s) {} were not converted",
            leftover.join(", ")
        ));
    }
}

// ------------------------------------------------------------------ prompts

fn convert_prompts(prompts: Option<&Json>, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let Some(list) = as_list(prompts) else {
        return out;
    };
    for (i, p) in list.into_iter().enumerate() {
        let id = format!("prompt-{i}");
        // `{raw: "..."}` is an inline prompt; `{id: file://...}` a file one.
        let template = p.as_str().or_else(|| {
            p.get("raw")
                .or_else(|| p.get("id"))
                .and_then(|v| v.as_str())
        });
        match template {
            Some(t) => out.push(json!({"id": id, "template": t})),
            None => notes.push(format!(
                "prompt #{i} is not a string or a `{{raw}}`/`{{id}}` mapping; skipped — \
                 rewrite it as a `messages:` prompt"
            )),
        }
    }
    out
}

// -------------------------------------------------------------------- tests

fn convert_default_test(default_test: &Json, notes: &mut Vec<String>) -> Option<Json> {
    let mut defaults = Obj::new();
    if let Some(vars) = default_test.get("vars").and_then(|v| v.as_object()) {
        defaults.insert("vars".into(), json!(vars));
    }
    if let Some(asserts) = default_test.get("assert") {
        let converted = convert_asserts(asserts, notes);
        if !converted.is_empty() {
            defaults.insert("assert".into(), json!(converted));
        }
    }
    if let Some(t) = default_test.get("threshold") {
        defaults.insert("threshold".into(), t.clone());
    }
    if let Some(obj) = default_test.as_object() {
        let extra = unknown_keys(obj, &["vars", "assert", "threshold"]);
        if !extra.is_empty() {
            notes.push(format!("defaultTest key(s) {extra} were not converted"));
        }
    }
    (!defaults.is_empty()).then_some(Json::Object(defaults))
}

fn convert_tests(tests: Option<&Json>, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let Some(list) = as_list(tests) else {
        return out;
    };
    for (i, t) in list.into_iter().enumerate() {
        if let Some(s) = t.as_str() {
            convert_test_file(s, notes, &mut out);
            continue;
        }
        let Some(obj) = t.as_object() else {
            notes.push(format!("test #{i} is not a string or a mapping; skipped"));
            continue;
        };
        out.push(Json::Object(convert_test_case(i, obj, notes)));
    }
    out
}

/// A `tests:` string: a `file://` reference to a file of cases, which domarinn
/// loads itself (YAML, JSON, JSONL, CSV, TSV) — but without converting it.
fn convert_test_file(s: &str, notes: &mut Vec<String>, out: &mut Vec<Json>) {
    if !s.starts_with("file://") {
        notes.push(format!(
            "test source '{s}' is not a file:// path domarinn can load; skipped"
        ));
        return;
    }
    let path = s.trim_start_matches("file://");
    let is_code = [".py", ".js", ".ts", ".mjs", ".cjs"]
        .iter()
        .any(|ext| path.split(':').next().unwrap_or(path).ends_with(ext));
    if is_code {
        notes.push(format!(
            "test source '{s}' is a promptfoo test generator; skipped — rewrite it as a \
             `generator:` test source"
        ));
        return;
    }
    notes.push(format!(
        "test source '{s}' is loaded by domarinn as-is, not converted: its paths resolve \
         against the suite directory, assertion types must be domarinn's, and a CSV's \
         `__expected` columns become plain vars (use a JSON `__assert` column instead)"
    ));
    out.push(json!(s));
}

fn convert_test_case(i: usize, t: &Obj, notes: &mut Vec<String>) -> Obj {
    let mut tc = Obj::new();
    if let Some(desc) = t.get("description").and_then(|d| d.as_str()) {
        tc.insert("description".into(), json!(desc));
    } else {
        tc.insert("id".into(), json!(format!("case-{i}")));
    }
    match t.get("vars") {
        Some(Json::Object(vars)) => {
            // promptfoo expands an array-valued var into one case per value
            // (the cartesian product over every such var): domarinn's `matrix`.
            let (axes, scalars): (Obj, Obj) = {
                let mut axes = Obj::new();
                let mut scalars = Obj::new();
                for (k, v) in vars {
                    if v.is_array() {
                        axes.insert(k.clone(), v.clone());
                    } else {
                        scalars.insert(k.clone(), v.clone());
                    }
                }
                (axes, scalars)
            };
            if !scalars.is_empty() {
                tc.insert("vars".into(), Json::Object(scalars));
            }
            if !axes.is_empty() {
                tc.insert("matrix".into(), Json::Object(axes));
            }
        }
        Some(Json::Null) | None => {}
        Some(other) => notes.push(format!(
            "test #{i}: `vars: {other}` (a vars file) was not converted; inline the vars"
        )),
    }
    if let Some(asserts) = t.get("assert") {
        tc.insert("assert".into(), json!(convert_asserts(asserts, notes)));
    }
    if let Some(threshold) = t.get("threshold") {
        tc.insert("threshold".into(), threshold.clone());
    }
    let extra = unknown_keys(t, &["description", "vars", "assert", "threshold"]);
    if !extra.is_empty() {
        notes.push(format!("test #{i}: key(s) {extra} were not converted"));
    }
    tc
}

// --------------------------------------------------------------- assertions

fn convert_asserts(asserts: &Json, notes: &mut Vec<String>) -> Vec<Json> {
    let mut out = Vec::new();
    let list = match asserts.as_array() {
        Some(l) => l,
        None => return out,
    };
    for a in list {
        out.extend(convert_assert(a, notes).into_iter().map(Json::Object));
    }
    out
}

/// Escape `s` for use as a literal inside a Rust regex.
fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if r"\.+*?()|[]{}^$".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A `contains-any`/`-all` value: a list, or promptfoo's comma-separated string.
fn string_list(v: Option<&Json>) -> Option<Vec<String>> {
    match v? {
        Json::Array(items) => items
            .iter()
            .map(|i| match i {
                Json::String(s) => Some(s.clone()),
                Json::Number(n) => Some(n.to_string()),
                _ => None,
            })
            .collect(),
        Json::String(s) => Some(s.split(',').map(|p| p.trim().to_string()).collect()),
        _ => None,
    }
}

/// Convert one promptfoo assertion. Usually one domarinn assertion; a
/// `contains-all` expands to one per value; an unmappable one to none (with a
/// note).
fn convert_assert(a: &Json, notes: &mut Vec<String>) -> Vec<Obj> {
    let Some(ty) = a.get("type").and_then(|t| t.as_str()) else {
        notes.push(format!("assertion {a} has no `type`; skipped"));
        return Vec::new();
    };
    if let Some(obj) = a.as_object() {
        let extra = unknown_keys(obj, &["type", "value", "weight", "threshold"]);
        if !extra.is_empty() {
            notes.push(format!(
                "'{ty}' assertion: key(s) {extra} were not converted"
            ));
        }
    }
    let (negated, base) = match ty.strip_prefix("not-") {
        Some(b) => (true, b),
        None => (false, ty),
    };
    let converted = match convert_base(base, negated, a) {
        Ok(v) => v,
        Err(reason) => {
            notes.push(format!("assertion type '{ty}' was skipped ({reason})"));
            return Vec::new();
        }
    };
    let weight = a.get("weight");
    let n = converted.len();
    converted
        .into_iter()
        .map(|mut m| {
            if negated {
                let t = m["type"].as_str().unwrap_or_default().to_string();
                m.insert("type".into(), json!(format!("not-{t}")));
            }
            if let Some(w) = weight {
                // A split `contains-all` shares its weight across the parts.
                let w = match w.as_f64() {
                    Some(f) if n > 1 => json!(f / n as f64),
                    _ => w.clone(),
                };
                m.insert("weight".into(), w);
            }
            m
        })
        .collect()
}

/// The un-negated mapping of `ty`. `negated` only decides which types are
/// refused: a `not-` prefix is applied by the caller.
fn convert_base(ty: &str, negated: bool, a: &Json) -> Result<Vec<Obj>, String> {
    let value = a.get("value");
    let threshold = a.get("threshold");
    let one = |pairs: Vec<(&str, Json)>| {
        let mut m = Obj::new();
        for (k, v) in pairs {
            m.insert(k.into(), v);
        }
        Ok(vec![m])
    };
    let need_value = || {
        value
            .cloned()
            .ok_or_else(|| "it has no `value`".to_string())
    };
    match ty {
        "contains" | "icontains" | "starts-with" | "regex" | "equals" => {
            one(vec![("type", json!(ty)), ("value", need_value()?)])
        }
        "is-json" => {
            if value.is_some() {
                return Err(
                    "domarinn's `is-json` takes no schema; use `contains-json` with `schema:`"
                        .into(),
                );
            }
            one(vec![("type", json!("is-json"))])
        }
        "contains-json" => {
            let mut pairs = vec![("type", json!("contains-json"))];
            if let Some(schema) = value {
                pairs.push(("schema", schema.clone()));
            }
            one(pairs)
        }
        // Case-sensitive, so not `icontains-any`: a regex alternation of the
        // escaped values is the exact equivalent.
        "contains-any" => {
            let values = string_list(value).ok_or("its `value` is not a list of strings")?;
            let alternation: Vec<String> = values.iter().map(|v| regex_escape(v)).collect();
            one(vec![
                ("type", json!("regex")),
                ("value", json!(alternation.join("|"))),
            ])
        }
        "icontains-any" => {
            let values = string_list(value).ok_or("its `value` is not a list of strings")?;
            one(vec![
                ("type", json!("icontains-any")),
                ("values", json!(values)),
            ])
        }
        "contains-all" | "icontains-all" => {
            if negated {
                return Err("\"missing at least one value\" has no domarinn equivalent".into());
            }
            let values = string_list(value).ok_or("its `value` is not a list of strings")?;
            let kind = ty.trim_end_matches("-all");
            Ok(values
                .into_iter()
                .map(|v| {
                    let mut m = Obj::new();
                    m.insert("type".into(), json!(kind));
                    m.insert("value".into(), json!(v));
                    m
                })
                .collect())
        }
        "llm-rubric" | "similar" => {
            let mut pairs = vec![("type", json!(ty)), ("value", need_value()?)];
            if let Some(t) = threshold {
                pairs.push(("threshold", t.clone()));
            }
            one(pairs)
        }
        "cost" | "latency" => {
            if negated {
                return Err("a negated budget has no meaning".into());
            }
            // promptfoo spells the budget `threshold`; accept `value` too.
            let max = threshold
                .or(value)
                .and_then(|v| v.as_f64())
                .ok_or("it has no numeric `threshold`")?;
            let max = if ty == "latency" {
                json!(max.ceil() as u64)
            } else {
                json!(max)
            };
            one(vec![("type", json!(ty)), ("max", max)])
        }
        "javascript" | "python" | "webhook" => {
            Err("no direct equivalent; rewrite it as an `exec` assertion".into())
        }
        _ => Err("no equivalent".into()),
    }
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
