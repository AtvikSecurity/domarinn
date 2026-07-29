//! The MCP tool registry.
//!
//! Eight tools covering every read surface the server has, but *not* a 1:1
//! mirror of its thirteen read endpoints — that would spend ~4-5k tokens of
//! `tools/list` per session and force three calls to answer one question.
//! Endpoints returning a bare `Vec<String>` fold into a `group_by` argument,
//! and closely-related detail endpoints fold into an `include` argument.
//!
//! Where the coverage lands, so a gap is obvious if one appears:
//!
//! | Question | Tool |
//! |---|---|
//! | What projects/suites/runs exist? | `find_runs` (with `group_by`) |
//! | How did one run go? Which cell is bad? | `get_run` (`include: matrix, config`) |
//! | Which cases failed? | `list_cases` |
//! | Why did *this* case fail? | `get_case` |
//! | Is it flaky or newly broken? | `case_history` |
//! | Did it get worse between two runs? | `compare_runs` |
//! | Where did I see that phrase? | `search` |
//! | What is this instance, and is the cache healthy? | `get_server_info` |
//!
//! Suite baselines and pass-rate trends need no tool of their own: they ride
//! along on `find_runs(group_by: "suite")` as `baseline_run_id` and `series`.
//!
//! Two omissions are deliberate and should stay that way:
//!
//! * **`export_run`** — the lossless run document, megabytes for a large run,
//!   straight into a context window. `get_run` + `list_cases` + `get_case`
//!   reach every field it carries, under limits it has none of; exposing it
//!   would mostly return "exceeded the response budget".
//! * **`cache_get` / `cache_head`** — opaque `sha256:`-keyed blobs, meaningless
//!   to a model. Cache *health* is covered by `get_server_info`.
//!
//! Handlers call [`crate::storage::Storage`] directly rather than reaching
//! into `routes.rs`. "Wrap the read API" means sharing the storage layer, not
//! re-parsing a serialized HTTP response.

mod analysis;
mod cases;
mod runs;
mod server;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::AppState;

/// A tool's outcome. `is_error` marks a *tool execution* error — something the
/// model can correct and retry, like a bad id or an out-of-range argument.
/// Protocol failures (an unknown tool, a malformed envelope) never come back
/// this way; they are JSON-RPC errors.
pub struct ToolResult {
    pub structured: Value,
    pub text: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn ok(structured: Value, text: impl Into<String>) -> ToolResult {
        ToolResult {
            structured,
            text: text.into(),
            is_error: false,
        }
    }

    /// An actionable failure. The message goes to the model verbatim, so it
    /// should say what to do differently.
    pub fn error(message: impl Into<String>) -> ToolResult {
        let message = message.into();
        ToolResult {
            structured: serde_json::json!({ "error": message }),
            text: message,
            is_error: true,
        }
    }
}

/// The requested tool is not in the registry.
pub struct UnknownTool;

/// Every tool definition, in a stable order so clients can cache `tools/list`
/// and prompt caches stay warm.
pub fn definitions() -> Vec<Value> {
    let mut defs = Vec::new();
    defs.extend(runs::definitions());
    defs.extend(cases::definitions());
    defs.extend(analysis::definitions());
    defs.extend(server::definitions());
    defs
}

/// Dispatch a `tools/call`.
pub async fn call(state: &AppState, name: &str, args: Value) -> Result<ToolResult, UnknownTool> {
    let result = match name {
        "find_runs" => runs::find_runs(state, args).await,
        "get_run" => runs::get_run(state, args).await,
        "list_cases" => cases::list_cases(state, args).await,
        "get_case" => cases::get_case(state, args).await,
        "case_history" => cases::case_history(state, args).await,
        "compare_runs" => analysis::compare_runs(state, args).await,
        "search" => analysis::search(state, args).await,
        "get_server_info" => server::get_server_info(state, args).await,
        _ => return Err(UnknownTool),
    };
    Ok(result)
}

/// Deserialize a tool's arguments, turning a bad shape into a self-correctable
/// tool error.
///
/// Argument structs are `deny_unknown_fields`, so a hallucinated argument
/// comes back naming the offending field instead of being silently dropped.
pub(super) fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, ToolResult> {
    serde_json::from_value(args)
        .map_err(|e| ToolResult::error(format!("invalid arguments: {e}. Check the tool's schema.")))
}

/// Shared annotation block: every tool here is a pure read over a closed,
/// finite corpus. `openWorldHint: false` is load-bearing — it tells the model
/// the data set is enumerable rather than an open-ended external world.
pub(super) fn read_only_annotations() -> Value {
    serde_json::json!({
        "readOnlyHint": true,
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
    })
}

/// Clamp a caller-supplied limit into a tool's window.
pub(super) fn clamp_limit(requested: Option<i64>, default: i64, max: i64) -> i64 {
    requested.unwrap_or(default).clamp(1, max)
}

/// Mirrors [`call`]'s dispatch table without executing anything: deserializes
/// `args` into the named tool's argument struct.
///
/// `None` means the name is not in the registry — which makes this the
/// dispatchability check too. Kept adjacent to `call` so the two match arms
/// are edited together; the drift tests fail loudly if they diverge.
#[cfg(test)]
fn probe_arguments(name: &str, args: Value) -> Option<bool> {
    let accepted = match name {
        "find_runs" => serde_json::from_value::<runs::FindRunsArgs>(args).is_ok(),
        "get_run" => serde_json::from_value::<runs::GetRunArgs>(args).is_ok(),
        "list_cases" => serde_json::from_value::<cases::ListCasesArgs>(args).is_ok(),
        "get_case" => serde_json::from_value::<cases::GetCaseArgs>(args).is_ok(),
        "case_history" => serde_json::from_value::<cases::CaseHistoryArgs>(args).is_ok(),
        "compare_runs" => serde_json::from_value::<analysis::CompareRunsArgs>(args).is_ok(),
        "search" => serde_json::from_value::<analysis::SearchArgs>(args).is_ok(),
        "get_server_info" => serde_json::from_value::<server::GetServerInfoArgs>(args).is_ok(),
        _ => return None,
    };
    Some(accepted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A JSON value matching a schema property's declared `type`, used to
    /// probe whether the argument struct actually accepts it.
    fn dummy_for(schema: &Value) -> Value {
        match schema.get("type").and_then(Value::as_str) {
            Some("string") => schema
                .get("enum")
                .and_then(Value::as_array)
                .and_then(|v| v.first().cloned())
                .unwrap_or_else(|| json!("x")),
            Some("integer") | Some("number") => json!(1),
            Some("boolean") => json!(true),
            Some("array") => {
                let item = schema.get("items").cloned().unwrap_or(json!({}));
                json!([dummy_for(&item)])
            }
            Some("object") => json!({}),
            _ => json!("x"),
        }
    }

    fn properties(def: &Value) -> Vec<(String, Value)> {
        def["inputSchema"]["properties"]
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    }

    fn required(def: &Value) -> Vec<String> {
        def["inputSchema"]["required"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Round-trip every declared property through the tool's own argument
    /// struct. Because those structs are `deny_unknown_fields`, a property
    /// declared in the schema but missing from the struct fails here — which
    /// is the drift a hand-written schema is otherwise exposed to.
    #[test]
    fn every_schema_property_is_accepted_by_its_argument_struct() {
        for def in definitions() {
            let name = def["name"].as_str().unwrap().to_string();
            let mut probe = serde_json::Map::new();
            for (key, schema) in properties(&def) {
                probe.insert(key, dummy_for(&schema));
            }
            let value = Value::Object(probe);
            assert_eq!(
                probe_arguments(&name, value.clone()),
                Some(true),
                "tool {name}: schema properties rejected by its argument struct: {value}"
            );
        }
    }

    #[test]
    fn required_names_exist_and_optional_args_really_are_optional() {
        for def in definitions() {
            let name = def["name"].as_str().unwrap().to_string();
            let props: Vec<String> = properties(&def).into_iter().map(|(k, _)| k).collect();
            for req in required(&def) {
                assert!(
                    props.contains(&req),
                    "tool {name}: required '{req}' is not a declared property"
                );
            }
            // The reverse drift: a field made mandatory in Rust without being
            // added to `required`, or vice versa.
            assert_eq!(
                probe_arguments(&name, json!({})),
                Some(required(&def).is_empty()),
                "tool {name}: empty-args acceptance disagrees with `required`"
            );
        }
    }

    #[test]
    fn every_definition_is_dispatchable_and_vice_versa() {
        let declared: Vec<String> = definitions()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(declared.len(), 8, "the catalog is deliberately small");

        for name in &declared {
            assert!(
                probe_arguments(name, json!({})).is_some(),
                "tool {name} is declared but not dispatchable"
            );
        }
        assert!(
            probe_arguments("not_a_tool", json!({})).is_none(),
            "an unregistered name must not resolve"
        );
        // Names are unique, so a client aggregating servers sees no collision
        // within ours.
        let mut sorted = declared.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), declared.len(), "duplicate tool name");
    }

    #[test]
    fn descriptions_are_present_and_bounded() {
        for def in definitions() {
            let name = def["name"].as_str().unwrap();
            let description = def["description"].as_str().unwrap_or_default();
            assert!(!description.is_empty(), "tool {name} has no description");
            // `tools/list` is sent every session; descriptions are a budget.
            assert!(
                description.len() <= 800,
                "tool {name}: description is {} bytes, over the 800-byte budget",
                description.len()
            );
            assert_eq!(
                def["annotations"]["readOnlyHint"], true,
                "tool {name} must be marked read-only"
            );
            assert!(
                def["inputSchema"]["type"] == "object",
                "tool {name}: inputSchema must be an object schema"
            );
        }
    }

    #[test]
    fn names_use_only_the_specs_safe_character_set() {
        for def in definitions() {
            let name = def["name"].as_str().unwrap();
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
                "tool name '{name}' uses characters outside the spec's safe set"
            );
            assert!((1..=128).contains(&name.len()), "tool name '{name}' length");
        }
    }
}
