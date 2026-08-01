//! Per-case conversation history: the config types for a test case's prior
//! turns and the `- history` placeholder marker inside a `messages:` prompt.
//!
//! A prompt opts into an explicit splice position with a bare `history` entry;
//! prompts without a marker get the default position (after the leading run of
//! `system` turns). A case supplies turns inline or as a whole transcript via
//! `file://`. See [`crate::render`] for the splice itself.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::types::ChatRole;

/// A tool call an `assistant` history turn made.
///
/// Field-for-field [`crate::result::ToolCall`], the type providers report on the
/// way *out*, so the `tool_calls` block of a stored case pastes into a suite's
/// `history:` unchanged. It is a separate type only because this one carries
/// `deny_unknown_fields` and the wire type must not: that one parses stored
/// blobs and vendor payloads, where an unknown key is forward compatibility;
/// this one parses a file a human typed, where `nmae:` should be an error
/// rather than a call with no name.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCallSpec {
    /// The call id this turn's result will answer. Optional — the vendor
    /// mappings derive one from position when a transcript omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// The arguments object. Every string leaf is a template; numbers and
    /// booleans pass through untouched, so `{order_id: 1042}` stays an integer.
    #[serde(default, skip_serializing_if = "Json::is_null")]
    pub arguments: Json,
}

/// One block of a turn's content in a suite file.
///
/// `deny_unknown_fields` for the same reason as [`ToolCallSpec`]: a typo'd
/// `tex:` should name itself, not silently produce an empty block.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentBlockSpec {
    /// Prose. A template, and may be `file://path`.
    Text { text: String },
    /// Reasoning replayed verbatim — **never** templated, and never loaded from
    /// `file://`, because `signature` is an integrity token over these exact
    /// bytes and any rewrite invalidates it. See [`crate::types::ContentBlock`].
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

/// A turn's content: prose, or an ordered list of blocks.
///
/// Untagged with `Text` first so the overwhelmingly common `content: "hi"`
/// keeps serializing as a bare string — an existing suite's `config_digest`
/// must not move because a block form became expressible.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MessageContentSpec {
    Text(String),
    Blocks(Vec<ContentBlockSpec>),
}

/// A chat message; `content` may be `file://path` to load from disk.
///
/// `content` is optional only because an `assistant` turn whose whole point is
/// a tool call has no prose, and a turn with neither content nor `tool_calls`
/// is caught by [`turn_problem`] rather than sent as an empty message.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: ChatRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContentSpec>,
    /// The calls this `assistant` turn made, replayed into the next request in
    /// each provider's native shape — `tool_use` blocks for `anthropic`, a
    /// `tool_calls` array for `openai`, the same JSON for `exec`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallSpec>,
    /// Which call a `tool` turn answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl MessageContentSpec {
    /// The authored prose, `thinking` blocks omitted — the config-side mirror
    /// of [`crate::types::MessageContent::text`], before any templating.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        use std::borrow::Cow;
        match self {
            MessageContentSpec::Text(s) => Cow::Borrowed(s),
            MessageContentSpec::Blocks(blocks) => {
                let parts: Vec<&str> = blocks
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlockSpec::Text { text } => Some(text.as_str()),
                        ContentBlockSpec::Thinking { .. } => None,
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

    /// Statically blank: nothing authored here can reach a provider.
    ///
    /// Only what is visible in the file — a `{{ note }}` that happens to render
    /// empty is *not* blank, because predicting that would mean rendering, and
    /// `validate` deliberately does not.
    pub fn is_blank(&self) -> bool {
        match self {
            MessageContentSpec::Text(s) => s.trim().is_empty(),
            MessageContentSpec::Blocks(blocks) => blocks.iter().all(|b| match b {
                ContentBlockSpec::Text { text } => text.trim().is_empty(),
                ContentBlockSpec::Thinking { thinking, .. } => thinking.trim().is_empty(),
            }),
        }
    }
}

impl Message {
    /// The plain-text turn, which is still almost every turn.
    pub fn text(role: ChatRole, content: impl Into<String>) -> Self {
        Message {
            role,
            content: Some(MessageContentSpec::Text(content.into())),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

/// The one rule about a single turn's internal coherence, in one place.
///
/// `loader_validate` reports it for the turns it can see at load; [`crate::render`]
/// errors on the ones it cannot, because a `file://` transcript is only read at
/// run time. Two call sites, one rule.
///
/// Deliberately **not** about role *ordering*, which stays unvalidated — that
/// is the provider's contract, as `docs/reference/domarinn-yaml.md` says. This
/// is about a turn that cannot mean anything at all.
pub fn turn_problem(m: &Message) -> Option<String> {
    if m.content.is_none() && m.tool_calls.is_empty() {
        return Some("a turn needs `content`, `tool_calls`, or both".to_string());
    }
    if !m.tool_calls.is_empty() && m.role != ChatRole::Assistant {
        return Some(format!(
            "`tool_calls` belongs on an `assistant` turn, not `{}`",
            m.role.as_str()
        ));
    }
    if m.tool_call_id.is_some() && m.role != ChatRole::Tool {
        return Some(format!(
            "`tool_call_id` belongs on a `tool` turn, not `{}`",
            m.role.as_str()
        ));
    }
    None
}

/// Deserialize out of a buffered [`Json`] value with the inner path
/// re-attached to the error text.
///
/// Buffering into `Json` (the peek-the-shape pattern shared with `TestSource`)
/// detaches the loader's outer `serde_path_to_error` tracking, so without this
/// an error deep in a 40-turn transcript loses its turn index. The path is
/// prefixed here instead — `[17].content: …` — which the outer tracker then
/// anchors at the right key.
fn detached<T: serde::de::DeserializeOwned>(value: Json) -> Result<T, String> {
    serde_path_to_error::deserialize(value).map_err(|e| {
        let path = e.path().to_string();
        if path == "." {
            e.inner().to_string()
        } else {
            format!("{path}: {}", e.inner())
        }
    })
}

/// The bare-string `history` placeholder inside a `messages:` prompt.
///
/// A single-variant fieldless enum so serde and schemars both see exactly the
/// string `"history"` — it round-trips as a bare string (config-digest
/// stability) and the generated schema is a one-value string enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HistoryMarker {
    History,
}

/// One entry in a `messages:` prompt: a chat turn, or the `history` marker
/// naming where each case's [`HistorySpec`] turns splice in.
///
/// `Deserialize` is hand-written (same contract as `TestSource`) so a typo'd
/// turn surfaces [`Message`]'s precise deny-guarded error and a misspelled
/// marker names the valid spelling, instead of the opaque "did not match any
/// variant" an untagged derive emits.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum PromptEntry {
    Marker(HistoryMarker),
    Turn(Message),
}

impl<'de> Deserialize<'de> for PromptEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let value = Json::deserialize(deserializer)?;
        match value {
            Json::String(s) if s == "history" => Ok(PromptEntry::Marker(HistoryMarker::History)),
            Json::String(s) => Err(D::Error::custom(format!(
                "unknown prompt entry '{s}': a string entry must be the \
                 `history` marker; a turn is a {{role, content}} mapping"
            ))),
            Json::Object(_) => detached::<Message>(value)
                .map(PromptEntry::Turn)
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "a prompt entry must be the `history` marker or a \
                 {{role, content}} mapping, found {other}"
            ))),
        }
    }
}

/// A test case's prior conversation: inline turns, or a whole transcript
/// loaded from a `file://` path (YAML or JSON list of `{role, content}`).
///
/// Each turn's `content` is a minijinja template rendered against the case's
/// vars, and may itself be `file://path` — the same contract as a `messages:`
/// prompt turn.
///
/// `Deserialize` is hand-written for precise errors, and so the string form
/// rejects anything that is not a `file://` path at parse time — a forgotten
/// prefix would otherwise surface much later, at render time, with the file's
/// path-shaped content as the mystery.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum HistorySpec {
    /// `file://path` to a YAML/JSON transcript file.
    File(String),
    /// Inline turns.
    Inline(Vec<Message>),
}

impl<'de> Deserialize<'de> for HistorySpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let value = Json::deserialize(deserializer)?;
        match value {
            Json::String(s) if s.starts_with("file://") => Ok(HistorySpec::File(s)),
            Json::String(s) => Err(D::Error::custom(format!(
                "history '{s}' is not a file:// path: a string history must be \
                 `file://<transcript>`; inline turns are a list of \
                 {{role, content}} mappings"
            ))),
            Json::Array(_) => detached::<Vec<Message>>(value)
                .map(HistorySpec::Inline)
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "history must be a file:// string or a list of \
                 {{role, content}} turns, found {other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Defaults, Prompt, TestCase};

    #[test]
    fn a_prompt_messages_list_accepts_a_marker_between_turns() {
        let prompt: Prompt = serde_json::from_value(serde_json::json!({
            "id": "support",
            "messages": [
                {"role": "system", "content": "You are a support agent."},
                "history",
                {"role": "user", "content": "{{ followup }}"},
            ],
        }))
        .unwrap();
        let entries = prompt.messages.unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[1], PromptEntry::Marker(_)));
    }

    #[test]
    fn a_test_case_takes_inline_and_file_history() {
        let inline: TestCase = serde_json::from_value(serde_json::json!({
            "id": "short",
            "history": [{"role": "user", "content": "hi"}],
        }))
        .unwrap();
        assert!(matches!(inline.history, Some(HistorySpec::Inline(_))));

        let file: TestCase = serde_json::from_value(serde_json::json!({
            "id": "long",
            "history": "file://convos/long.yaml",
        }))
        .unwrap();
        assert!(matches!(file.history, Some(HistorySpec::File(_))));
    }

    /// The load-bearing back-compat property (same contract as `EnvNames`): a
    /// case or defaults block that never mentions `history` must serialize
    /// byte-identically to before the field existed, or every existing suite's
    /// `config_digest` moves and `--against` reports drift that did not happen.
    #[test]
    fn unset_history_stays_out_of_the_serialized_config() {
        let case = serde_json::to_value(TestCase::default()).unwrap();
        assert!(!case.as_object().unwrap().contains_key("history"));
        let defaults = serde_json::to_value(Defaults::default()).unwrap();
        assert!(!defaults.as_object().unwrap().contains_key("history"));
    }

    #[test]
    fn defaults_take_a_history_fallback() {
        let defaults: Defaults = serde_json::from_value(serde_json::json!({
            "history": [{"role": "user", "content": "hi"}],
        }))
        .unwrap();
        assert!(matches!(defaults.history, Some(HistorySpec::Inline(_))));
    }

    /// The marker is the bare string `history`, and it must round-trip back to
    /// that bare string: the serialized suite feeds `config_digest`, so any
    /// other serialization would move the digest of every suite using a marker
    /// between domarinn versions.
    #[test]
    fn the_marker_round_trips_as_a_bare_string() {
        let entry: PromptEntry = serde_json::from_value(serde_json::json!("history")).unwrap();
        assert!(matches!(entry, PromptEntry::Marker(_)));
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            serde_json::json!("history")
        );
    }

    #[test]
    fn a_mapping_entry_parses_as_a_turn() {
        let entry: PromptEntry =
            serde_json::from_value(serde_json::json!({"role": "user", "content": "hi"})).unwrap();
        match entry {
            PromptEntry::Turn(m) => assert_eq!(m.content.unwrap().text(), "hi"),
            PromptEntry::Marker(_) => panic!("a mapping must not parse as the marker"),
        }
    }

    /// A misspelled marker must say what the valid spellings are, not emit the
    /// opaque "did not match any variant" an untagged derive would.
    #[test]
    fn a_misspelled_marker_names_the_valid_forms() {
        let err = serde_json::from_value::<PromptEntry>(serde_json::json!("histroy"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("history"), "unhelpful error: {err}");
    }

    /// A bad turn surfaces [`Message`]'s own deny-guarded error verbatim, the
    /// same contract `TestSource` keeps for inline test cases.
    ///
    /// The vehicle is a misspelled field rather than a missing `content`:
    /// `content` became optional when an `assistant` turn was allowed to be
    /// nothing but a tool call, so `{role: user}` now parses here and is caught
    /// later by [`turn_problem`] instead.
    #[test]
    fn a_bad_turn_surfaces_the_message_error() {
        let err = serde_json::from_value::<PromptEntry>(
            serde_json::json!({"role": "user", "contnet": "hi"}),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("contnet"), "unhelpful error: {err}");
    }

    /// The turn shapes that parse but cannot mean anything, caught by
    /// [`turn_problem`] rather than by serde.
    #[test]
    fn an_incoherent_turn_is_reported_by_turn_problem() {
        let empty = Message {
            role: ChatRole::User,
            content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        assert!(turn_problem(&empty).unwrap().contains("tool_calls"));

        let calls_on_a_user_turn = Message {
            tool_calls: vec![ToolCallSpec {
                id: None,
                name: "f".into(),
                arguments: Json::Null,
            }],
            ..Message::text(ChatRole::User, "hi")
        };
        assert!(turn_problem(&calls_on_a_user_turn)
            .unwrap()
            .contains("assistant"));

        let id_on_an_assistant_turn = Message {
            tool_call_id: Some("call_1".into()),
            ..Message::text(ChatRole::Assistant, "hi")
        };
        assert!(turn_problem(&id_on_an_assistant_turn)
            .unwrap()
            .contains("tool"));

        // The shapes that are fine.
        assert!(turn_problem(&Message::text(ChatRole::User, "hi")).is_none());
        assert!(turn_problem(&Message {
            content: None,
            tool_calls: vec![ToolCallSpec {
                id: None,
                name: "f".into(),
                arguments: Json::Null,
            }],
            ..Message::text(ChatRole::Assistant, "")
        })
        .is_none());
    }

    #[test]
    fn history_accepts_a_file_string_and_inline_turns() {
        let file: HistorySpec =
            serde_json::from_value(serde_json::json!("file://convo.yaml")).unwrap();
        assert!(matches!(&file, HistorySpec::File(p) if p == "file://convo.yaml"));

        let inline: HistorySpec =
            serde_json::from_value(serde_json::json!([{"role": "user", "content": "hi"}])).unwrap();
        match inline {
            HistorySpec::Inline(turns) => assert_eq!(turns.len(), 1),
            HistorySpec::File(_) => panic!("a list must parse as inline turns"),
        }
    }

    /// Both forms serialize back to their input shape (digest stability for
    /// suites that use either).
    #[test]
    fn history_serializes_back_to_its_input_shape() {
        for raw in [
            serde_json::json!("file://convo.yaml"),
            serde_json::json!([{"role": "assistant", "content": "prior answer"}]),
        ] {
            let spec: HistorySpec = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(serde_json::to_value(&spec).unwrap(), raw);
        }
    }

    /// The string form exists only to point at a transcript file; accepting an
    /// arbitrary string would silently treat a forgotten `file://` prefix (or a
    /// pasted transcript) as a path-shaped no-op much later, at render time.
    #[test]
    fn a_non_file_string_history_is_rejected_at_parse() {
        let err = serde_json::from_value::<HistorySpec>(serde_json::json!("convo.yaml"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("file://"), "unhelpful error: {err}");
    }

    /// The detach-into-Value pattern turns off `serde_path_to_error`'s outer
    /// tracking, so the deserializer must re-attach the path itself: in a
    /// 40-turn transcript, "unknown field `contnet`" without the turn index is
    /// a needle-in-haystack hunt.
    #[test]
    fn a_bad_turn_error_names_its_index_in_the_transcript() {
        let err = serde_json::from_value::<HistorySpec>(serde_json::json!([
            {"role": "user", "content": "ok"},
            {"role": "user", "contnet": "typo"},
        ]))
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("[1]"),
            "error must name the failing turn's index: {err}"
        );
    }

    #[test]
    fn a_bad_inline_turn_surfaces_the_message_error() {
        let err = serde_json::from_value::<HistorySpec>(
            serde_json::json!([{"role": "developer", "content": "x"}]),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("developer"), "unhelpful error: {err}");
    }
}
