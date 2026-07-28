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
///
/// The cache fields are their own line items rather than folded into
/// `input_tokens` because they are *billed* differently — a cache read is a
/// fraction of the input rate and a cache write is a premium on it — so a cost
/// model that cannot see them is wrong on exactly the calls that populate the
/// cache. On a cache-heavy workload they can be the majority of real spend.
///
/// All three are optional and `skip_serializing_if`, so a stored `CaseResult`
/// written before they existed re-serializes byte-identically. That property is
/// load-bearing: the server content-hashes the run document for ingest
/// idempotency, so a field that appears with a zero default would turn a
/// re-upload into a conflict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[ts(optional_fields)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Input tokens served from a provider-side prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Input tokens written *into* a provider-side prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// The subset of [`Self::cache_write_tokens`] written at a longer-lived
    /// cache TTL, when the provider reports the split. Absent means "all at the
    /// default TTL" — which is what the vendors' own default is, so absence and
    /// zero mean the same thing here and neither is a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_tokens: Option<u64>,
}

impl TokenUsage {
    /// Input plus output. What the `tokens` assertion grades by default.
    ///
    /// Deliberately excludes the cache counts. Re-scoping this to include them
    /// would silently change the meaning of every `tokens: {max: N}` already
    /// written — a cache-heavy case that passed at 1,200 could start failing at
    /// 40,000 with no config change, which is a breaking behavior change
    /// wearing a bugfix's clothes. Use [`Self::billable_total`] to opt in.
    pub fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Every token the provider bills for, cache traffic included.
    ///
    /// Saturating, because these are counts reported by someone else: a
    /// provider that reports nonsense should skew a budget assertion, not panic
    /// a release build into wrapping around zero.
    pub fn billable_total(&self) -> u64 {
        self.total()
            .saturating_add(self.cache_read_tokens.unwrap_or(0))
            .saturating_add(self.cache_write_tokens.unwrap_or(0))
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
