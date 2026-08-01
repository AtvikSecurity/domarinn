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
    /// Every token in the exchange: the whole prompt plus the response. What the
    /// `tokens` assertion grades by default.
    ///
    /// [`Self::cache_read_tokens`] is *included*, and this is load-bearing. The
    /// field means "the part of the prompt that was served from a provider-side
    /// cache" — it is prompt, and it was sent. Both vendors exclude it from
    /// `input_tokens` (Anthropic natively; OpenAI once `usage_from_payload`
    /// normalizes it out of `prompt_tokens` so it is not billed twice), so a
    /// total over `input + output` alone measures the *uncached* prompt and
    /// nothing else.
    ///
    /// That made a `tokens: {max: N}` budget unenforceable exactly when it
    /// mattered. A 6,000-token system prompt fails the budget cold, then passes
    /// it warm at 200 — same suite, same prompt, no config change, and a
    /// prompt-growth regression that can never be caught again once the cache is
    /// warm. A guard that silently switches off is worse than no guard, so the
    /// budget counts the prompt that was sent rather than the fraction of it
    /// that happened to be billed at full rate.
    ///
    /// Saturating for the same reason [`Self::billable_total`] is: these are
    /// counts reported by someone else.
    pub fn total(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens.unwrap_or(0))
    }

    /// Every token the provider bills for, cache *writes* included.
    ///
    /// The delta over [`Self::total`] is the write step: tokens paid for to
    /// populate the cache, which are not part of the prompt the model answered.
    /// Budgeting them is opting in to paying attention to cache economics, which
    /// is what `count: billable` says.
    pub fn billable_total(&self) -> u64 {
        self.total()
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
    /// A tool's result, answering a call an `assistant` turn made.
    ///
    /// Only ever *replayed into* a request — no provider reports one coming
    /// back, because domarinn never runs a tool. It exists so a transcript can
    /// say what came back from the call it is replaying.
    Tool,
}

impl ChatRole {
    /// The wire string for this role (identical to its serde encoding).
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        }
    }
}

/// One block of a chat message's content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Ordinary prose.
    Text { text: String },
    /// The model's reasoning, replayed verbatim.
    ///
    /// `signature` is the vendor's integrity token over the thinking bytes, and
    /// it is the reason a thinking block is **never** templated anywhere in
    /// domarinn: rendering `{{ var }}` inside `thinking` changes the bytes the
    /// signature covers, and the provider rejects the replay. Nothing else in
    /// a suite has that property, so it is stated at the type rather than left
    /// to be discovered.
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        signature: Option<String>,
    },
}

/// A chat message's content: plain text, or an ordered list of typed blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(untagged)]
pub enum MessageContent {
    /// The plain-text turn, which is still almost every turn.
    ///
    /// First in the enum and untagged, so it serializes as a bare string. That
    /// is load-bearing rather than tidy: this type is inside what
    /// `digests::prompt_digest` hashes, so a transcript using nothing else must
    /// write the bytes it wrote before blocks existed or every warm cache entry
    /// re-keys on upgrade.
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    /// The turn's prose, with `thinking` blocks omitted.
    ///
    /// Thinking is the model's scratch work, not its answer. Folding it in here
    /// would leak reasoning into the `http` provider's `{{ prompt }}` string,
    /// the server's search index, and the CLI's transcript view — three places
    /// that want what was *said*.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        use std::borrow::Cow;
        match self {
            MessageContent::Text(s) => Cow::Borrowed(s),
            MessageContent::Blocks(blocks) => {
                let parts: Vec<&str> = blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        ContentBlock::Thinking { .. } => None,
                    })
                    .collect();
                match parts.as_slice() {
                    [] => Cow::Borrowed(""),
                    [only] => Cow::Borrowed(only),
                    many => Cow::Owned(many.join("\n")),
                }
            }
        }
    }

    /// Nothing to send: no prose, and no block carrying anything.
    ///
    /// A `thinking` block counts as content even though [`Self::text`] omits
    /// it — the provider still receives it, so a turn holding only thinking is
    /// not blank.
    pub fn is_blank(&self) -> bool {
        match self {
            MessageContent::Text(s) => s.trim().is_empty(),
            MessageContent::Blocks(blocks) => blocks.iter().all(|b| match b {
                ContentBlock::Text { text } => text.trim().is_empty(),
                ContentBlock::Thinking { thinking, .. } => thinking.trim().is_empty(),
            }),
        }
    }
}

impl From<String> for MessageContent {
    fn from(s: String) -> Self {
        MessageContent::Text(s)
    }
}

impl From<&str> for MessageContent {
    fn from(s: &str) -> Self {
        MessageContent::Text(s.to_string())
    }
}

/// A chat message in a rendered prompt.
///
/// The two tool fields are the request-side mirror of `CaseResult.tool_calls`:
/// domarinn has always been able to report a call coming *out* of a turn, and
/// these are how one goes *in*. They use the very same [`crate::result::ToolCall`],
/// so the `tool_calls` block of a stored case pastes into a suite's `history:`
/// unchanged.
///
/// Both are `skip_serializing_if`-guarded for the same cache reason
/// [`MessageContent::Text`] is first in its enum: a transcript that mentions
/// neither must serialize byte-identically to before they existed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, TS)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: MessageContent,
    /// The calls this `assistant` turn made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<crate::result::ToolCall>,
    /// Which call this `tool` turn answers.
    ///
    /// Optional: for a round of parallel calls answered in order, position is
    /// enough, and the vendor mappings derive the id rather than demanding it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// The plain-text turn, which is still almost every turn.
    pub fn text(role: ChatRole, content: impl Into<String>) -> Self {
        ChatMessage {
            role,
            content: MessageContent::Text(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
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
            (ChatRole::Tool, "tool"),
        ] {
            assert_eq!(role.as_str(), wire);
            assert_eq!(serde_json::to_value(role).unwrap(), serde_json::json!(wire));
            let parsed: ChatRole = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(parsed, role);
        }
        assert!(serde_json::from_value::<ChatRole>(serde_json::json!("developer")).is_err());
    }

    /// The cache tripwire. `ChatMessage` is inside what `prompt_digest` hashes,
    /// so a plain turn must serialize to exactly the two keys it always did —
    /// asserted by whole-value equality, not `contains_key`, because an
    /// unguarded new field is precisely what this needs to catch.
    #[test]
    fn a_plain_turn_serializes_without_the_tool_keys() {
        let turn = ChatMessage::text(ChatRole::User, "hi");
        assert_eq!(
            serde_json::to_value(&turn).unwrap(),
            serde_json::json!({"role": "user", "content": "hi"})
        );
    }

    /// A transcript recorded before tool turns existed still loads.
    #[test]
    fn a_turn_written_before_tool_fields_existed_still_parses() {
        let turn: ChatMessage =
            serde_json::from_value(serde_json::json!({"role": "user", "content": "hi"})).unwrap();
        assert_eq!(turn.content, MessageContent::Text("hi".into()));
        assert!(turn.tool_calls.is_empty());
        assert_eq!(turn.tool_call_id, None);
    }

    #[test]
    fn a_tool_bearing_turn_round_trips() {
        let turn = ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Text(String::new()),
            tool_calls: vec![crate::result::ToolCall {
                id: Some("call_1".into()),
                name: "lookup_order".into(),
                arguments: serde_json::json!({"order_id": 1042}),
            }],
            tool_call_id: None,
        };
        let wire = serde_json::to_value(&turn).unwrap();
        assert_eq!(wire["tool_calls"][0]["name"], "lookup_order");
        // The arguments stay a decoded object, never a JSON string.
        assert_eq!(wire["tool_calls"][0]["arguments"]["order_id"], 1042);
        assert_eq!(serde_json::from_value::<ChatMessage>(wire).unwrap(), turn);
    }

    /// Untagged, `Text` first: a string stays a string on the wire.
    #[test]
    fn plain_content_serializes_as_a_bare_string() {
        assert_eq!(
            serde_json::to_value(MessageContent::Text("hi".into())).unwrap(),
            serde_json::json!("hi")
        );
        let parsed: MessageContent = serde_json::from_value(serde_json::json!("hi")).unwrap();
        assert_eq!(parsed, MessageContent::Text("hi".into()));
    }

    #[test]
    fn block_content_round_trips_and_keeps_its_signature() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Thinking {
                thinking: "The order id is 1042".into(),
                signature: Some("ErUBCk".into()),
            },
            ContentBlock::Text {
                text: "Let me look that up.".into(),
            },
        ]);
        let wire = serde_json::to_value(&content).unwrap();
        assert_eq!(wire[0]["type"], "thinking");
        assert_eq!(wire[0]["signature"], "ErUBCk");
        assert_eq!(wire[1]["type"], "text");
        assert_eq!(
            serde_json::from_value::<MessageContent>(wire).unwrap(),
            content
        );
    }

    /// `text()` is the prose view: thinking is scratch work, not the answer.
    #[test]
    fn text_joins_text_blocks_and_omits_thinking() {
        let content = MessageContent::Blocks(vec![
            ContentBlock::Thinking {
                thinking: "secret reasoning".into(),
                signature: None,
            },
            ContentBlock::Text { text: "one".into() },
            ContentBlock::Text { text: "two".into() },
        ]);
        assert_eq!(content.text(), "one\ntwo");
        assert!(!content.text().contains("secret reasoning"));
    }

    /// A thinking-only turn has no prose but is not blank — the provider still
    /// receives the block.
    #[test]
    fn a_thinking_only_turn_has_no_text_but_is_not_blank() {
        let content = MessageContent::Blocks(vec![ContentBlock::Thinking {
            thinking: "reasoning".into(),
            signature: None,
        }]);
        assert_eq!(content.text(), "");
        assert!(!content.is_blank());
    }

    #[test]
    fn blank_content_is_recognized_in_both_shapes() {
        assert!(MessageContent::Text("   ".into()).is_blank());
        assert!(MessageContent::Blocks(vec![]).is_blank());
        assert!(MessageContent::Blocks(vec![ContentBlock::Text { text: " ".into() }]).is_blank());
        assert!(!MessageContent::Text("hi".into()).is_blank());
    }
}
