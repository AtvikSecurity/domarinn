//! Small value types shared across providers, asserts, and results.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// A provider's output — either free text or a structured JSON value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Output {
    Text(String),
    Json(Json),
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// A chat message in a rendered prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// A prompt after rendering, ready to hand to a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RenderedPrompt {
    Text(String),
    Messages(Vec<ChatMessage>),
}
