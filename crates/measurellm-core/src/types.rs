//! Small value types shared across providers, asserts, and results.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use ts_rs::TS;

/// A provider's output — either free text or a structured JSON value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum Output {
    Text(String),
    Json(#[ts(type = "unknown")] Json),
}

impl Output {
    /// A text view of the output, serializing JSON compactly if needed.
    pub fn as_text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Output::Text(s) => std::borrow::Cow::Borrowed(s),
            Output::Json(v) => std::borrow::Cow::Owned(v.to_string()),
        }
    }

    /// The output as JSON if it is (or parses as) JSON.
    pub fn as_json(&self) -> Option<Json> {
        match self {
            Output::Json(v) => Some(v.clone()),
            Output::Text(s) => serde_json::from_str(s).ok(),
        }
    }
}

/// Token accounting for a single provider call.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
}

impl TokenUsage {
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// A chat message role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatRole {
    /// The wire string for this role (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

/// A chat message in a rendered prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

/// A prompt after rendering, ready to hand to a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum RenderedPrompt {
    Text(String),
    Messages(Vec<ChatMessage>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_role_round_trips_and_matches_as_str() {
        for (role, wire) in [
            (ChatRole::System, "system"),
            (ChatRole::User, "user"),
            (ChatRole::Assistant, "assistant"),
        ] {
            assert_eq!(role.as_str(), wire);
            assert_eq!(serde_json::to_value(role).unwrap(), serde_json::json!(wire));
            let parsed: ChatRole = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(parsed, role);
        }
        assert!(serde_json::from_value::<ChatRole>(serde_json::json!("developer")).is_err());
    }
}
