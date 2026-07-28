//! The stable wire name of an assertion kind.
//!
//! Separate from the engine's `AssertKind` (which carries each kind's
//! configuration) because this is the value that gets *stored*: it appears on
//! every `AssertResult` in every run document, so it belongs with the wire
//! types rather than with the code that evaluates them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A field-less mirror of every [`AssertKind`] variant, recorded in results as
/// the stable "kind" name (matches the config `type` tag). Kept in sync with
/// `AssertKind` by [`AssertKind::name`] and the pin test below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AssertName {
    Contains,
    Icontains,
    IcontainsAny,
    Regex,
    Equals,
    StartsWith,
    IsJson,
    ContainsJson,
    Length,
    Jinja,
    Exec,
    LlmRubric,
    Cost,
    Latency,
    Tokens,
    Similar,
    ToolCall,
}

impl AssertName {
    /// The wire string for this kind (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            AssertName::Contains => "contains",
            AssertName::Icontains => "icontains",
            AssertName::IcontainsAny => "icontains-any",
            AssertName::Regex => "regex",
            AssertName::Equals => "equals",
            AssertName::StartsWith => "starts-with",
            AssertName::IsJson => "is-json",
            AssertName::ContainsJson => "contains-json",
            AssertName::Length => "length",
            AssertName::Jinja => "jinja",
            AssertName::Exec => "exec",
            AssertName::LlmRubric => "llm-rubric",
            AssertName::Cost => "cost",
            AssertName::Latency => "latency",
            AssertName::Tokens => "tokens",
            AssertName::Similar => "similar",
            AssertName::ToolCall => "tool-call",
        }
    }
}
