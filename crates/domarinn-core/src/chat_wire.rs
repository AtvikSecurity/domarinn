//! The two vendor wire shapes for a rendered transcript, side by side.
//!
//! Anthropic and OpenAI disagree about tool turns in exactly mirrored ways:
//! Anthropic puts every result of a round of parallel calls in **one** `user`
//! message, as `tool_result` blocks; OpenAI wants **one** `role: "tool"`
//! message per call. Keeping both here is the point — the pairing (which
//! result answers which call) exists once, and the split is one arm of one
//! match in each direction rather than two files a reviewer has to hold in
//! their head at the same time.
//!
//! Byte-identity is a hard constraint throughout. A transcript with no tool
//! turns and no content blocks must produce exactly the body it produced
//! before either existed, because that body is what
//! [`crate::digests::prompt_digest`] hashes into every warm cache entry. Each
//! mapping's plain arm is therefore a verbatim copy of the pre-feature code,
//! and the tool arms are only ever *added* paths.

use serde_json::{json, Value as Json};

use crate::result::ToolCall;
use crate::types::{ChatMessage, ChatRole, ContentBlock, MessageContent};

/// The transcript, grouped the way both vendors need to see it.
enum Turn<'a> {
    /// A `system`/`user` turn, or an `assistant` turn that made no calls: one
    /// message either way.
    Plain(&'a ChatMessage),
    /// An `assistant` turn that made calls, every call's id resolved.
    Calls(&'a ChatMessage, Vec<(&'a ToolCall, String)>),
    /// One round of results — a run of consecutive `tool` turns — each paired
    /// with the id it answers.
    Results(Vec<(&'a ChatMessage, String)>),
}

/// A deterministic id for a call the transcript did not name.
///
/// Both vendors *require* ids (`tool_use.id` / `tool_calls[].id`, and the
/// matching `tool_use_id` / `tool_call_id`), and a transcript pasted out of a
/// log or typed by hand often has none. Deriving it from position makes the
/// omission just work without inventing anything a rerun could disagree with:
/// the same transcript always produces the same body, so the same cache key.
fn synthetic_id(turn: usize, call: usize) -> String {
    format!("domarinn_call_{turn}_{call}")
}

/// Walk the transcript once, resolving every call/result id.
///
/// A `tool` turn's id is, in order: its own `tool_call_id`; else the id of the
/// call at the same position in the most recent round of calls; else
/// [`synthetic_id`]. That fallback chain is what lets an author write a
/// parallel round without naming a single id and still have both vendors
/// receive the correlation they demand.
fn plan(msgs: &[ChatMessage]) -> Vec<Turn<'_>> {
    let mut out = Vec::new();
    // The ids of the most recent `Calls` turn, so a following run of results
    // can be paired with it positionally.
    let mut open_calls: Vec<String> = Vec::new();
    let mut i = 0;
    while i < msgs.len() {
        let m = &msgs[i];

        if m.role == ChatRole::Tool {
            let start = i;
            let mut run = Vec::new();
            while i < msgs.len() && msgs[i].role == ChatRole::Tool {
                let nth = i - start;
                let id = msgs[i]
                    .tool_call_id
                    .clone()
                    .or_else(|| open_calls.get(nth).cloned())
                    .unwrap_or_else(|| synthetic_id(start, nth));
                run.push((&msgs[i], id));
                i += 1;
            }
            // Consumed. Without this, a later run of `tool` turns that is *not*
            // preceded by its own call turn would pair positionally against
            // this round and re-emit an id already used — silently answering
            // the wrong call. Such a transcript is malformed either way, but a
            // fresh synthetic id is wrong in a way the provider can see.
            open_calls.clear();
            out.push(Turn::Results(run));
            continue;
        }

        if m.tool_calls.is_empty() {
            out.push(Turn::Plain(m));
        } else {
            let calls: Vec<(&ToolCall, String)> = m
                .tool_calls
                .iter()
                .enumerate()
                .map(|(nth, tc)| (tc, tc.id.clone().unwrap_or_else(|| synthetic_id(i, nth))))
                .collect();
            open_calls = calls.iter().map(|(_, id)| id.clone()).collect();
            out.push(Turn::Calls(m, calls));
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Anthropic
// ---------------------------------------------------------------------------

/// A transcript as `(system, messages)` for the Messages API.
pub(crate) fn anthropic_messages(msgs: &[ChatMessage]) -> (Option<String>, Vec<Json>) {
    let mut system: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for turn in plan(msgs) {
        match turn {
            Turn::Plain(m) if m.role == ChatRole::System => {
                system.push(m.content.text().into_owned());
            }
            Turn::Plain(m) => {
                out.push(json!({"role": m.role, "content": anthropic_content(&m.content)}));
            }
            Turn::Calls(m, calls) => {
                // Text first, then the calls: the order the model emitted them
                // in, and the only order this shape can express.
                let mut blocks = anthropic_blocks(&m.content);
                for (tc, id) in calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": id,
                        "name": tc.name,
                        // `{}`, not `null`: a call with no arguments is a call
                        // with an empty arguments object.
                        "input": if tc.arguments.is_null() { json!({}) } else { tc.arguments.clone() },
                    }));
                }
                out.push(json!({"role": "assistant", "content": blocks}));
            }
            // The coalesce. Every result for a round of parallel calls goes in
            // ONE user message, because a `tool_result` must sit in the message
            // that follows its `tool_use` and Anthropic accepts at most one
            // such message per round.
            //
            // Only consecutive `tool` turns are merged — a following plain
            // `user` turn stays its own message. That does leave two `user`
            // messages in a row whenever a prompt appends a turn after the
            // history (the shape example 42 uses), which is fine: Anthropic
            // accepts consecutive same-role messages and combines them. Folding
            // the author's prose into a `tool_result` message to avoid it would
            // put their question inside a tool's answer.
            Turn::Results(results) => {
                let blocks: Vec<Json> = results
                    .iter()
                    .map(|(m, id)| {
                        json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": m.content.text(),
                        })
                    })
                    .collect();
                out.push(json!({"role": "user", "content": blocks}));
            }
        }
    }
    ((!system.is_empty()).then(|| system.join("\n\n")), out)
}

/// A turn's content in Anthropic's shape.
///
/// Plain text stays a **bare string** rather than becoming a one-element block
/// array. Both are valid to the API, but only the string reproduces the bytes
/// this code emitted before blocks existed.
fn anthropic_content(content: &MessageContent) -> Json {
    match content {
        MessageContent::Text(s) => json!(s),
        MessageContent::Blocks(_) => Json::Array(anthropic_blocks(content)),
    }
}

/// A turn's content as an explicit block list, for the arms that must append.
fn anthropic_blocks(content: &MessageContent) -> Vec<Json> {
    match content {
        // `trim`, not `is_empty`: Anthropic rejects a text block that holds
        // only whitespace ("text content blocks must contain non-whitespace
        // text"), so a turn written as `content: " "` alongside a tool call
        // would 400 on a block that carries nothing anyway.
        MessageContent::Text(s) if s.trim().is_empty() => Vec::new(),
        MessageContent::Text(s) => vec![json!({"type": "text", "text": s})],
        MessageContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => json!({"type": "text", "text": text}),
                ContentBlock::Thinking {
                    thinking,
                    signature,
                } => {
                    let mut block = json!({"type": "thinking", "thinking": thinking});
                    // Absent rather than null when the transcript has no
                    // signature: the API rejects a null one.
                    if let Some(sig) = signature {
                        block["signature"] = json!(sig);
                    }
                    block
                }
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// OpenAI
// ---------------------------------------------------------------------------

/// A transcript as a chat-completions `messages` array.
pub(crate) fn openai_messages(msgs: &[ChatMessage]) -> Vec<Json> {
    let mut out = Vec::new();
    for turn in plan(msgs) {
        match turn {
            Turn::Plain(m) => {
                out.push(json!({"role": m.role, "content": openai_content(&m.content)}));
            }
            Turn::Calls(m, calls) => {
                let text = m.content.text();
                out.push(json!({
                    "role": "assistant",
                    // `null`, not `""`: the vendor's own encoding for an
                    // assistant turn that only called a tool.
                    "content": if text.is_empty() { Json::Null } else { json!(text) },
                    "tool_calls": calls
                        .iter()
                        .map(|(tc, id)| json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": arguments_string(&tc.arguments),
                            },
                        }))
                        .collect::<Vec<_>>(),
                }));
            }
            // The expand — the mirror image of the Anthropic arm. One message
            // per result, each naming the call it answers.
            Turn::Results(results) => out.extend(results.iter().map(|(m, id)| {
                json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": m.content.text(),
                })
            })),
        }
    }
    out
}

/// A turn's content for chat-completions.
///
/// OpenAI has no `thinking` concept, so a thinking block is **dropped** here
/// rather than approximated: sending it as text would put the model's private
/// scratch work into the conversation as if it had been said aloud.
fn openai_content(content: &MessageContent) -> Json {
    json!(content.text())
}

/// OpenAI's `function.arguments` is a JSON **string**, not an object.
///
/// This is the exact vendor split [`crate::openai::tool_calls_from_message`]
/// undoes on the way in; this is the same split re-applied on the way out. Two
/// edges of it:
///
/// - `null` — the default for a call written without arguments — becomes
///   `"{}"`, the empty-arguments spelling, not the literal `"null"`.
/// - a `Json::String` is what the inbound parser *keeps* when a vendor sent
///   arguments that were not valid JSON. Re-encoding it produces a quoted
///   string that parses back to the same value, so the pair round-trips.
fn arguments_string(args: &Json) -> String {
    if args.is_null() {
        "{}".to_string()
    } else {
        args.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(name: &str, args: Json) -> ToolCall {
        ToolCall {
            id: None,
            name: name.to_string(),
            arguments: args,
        }
    }

    fn assistant_with(calls: Vec<ToolCall>) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Text(String::new()),
            tool_calls: calls,
            tool_call_id: None,
        }
    }

    fn tool_result(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::Tool,
            content: MessageContent::Text(content.to_string()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// One parallel round, written the way an author would: no ids anywhere.
    fn parallel_round() -> Vec<ChatMessage> {
        vec![
            ChatMessage::text(ChatRole::User, "status of 1042 and 1043?"),
            assistant_with(vec![
                call("lookup", json!({"id": 1042})),
                call("lookup", json!({"id": 1043})),
            ]),
            tool_result("shipped"),
            tool_result("pending"),
        ]
    }

    // -- the cache tripwires ------------------------------------------------

    /// A tool-free, block-free transcript must map to exactly the bytes this
    /// code produced before either feature existed. Whole-value equality, so a
    /// stray added key fails.
    #[test]
    fn a_plain_transcript_maps_exactly_as_it_did_for_anthropic() {
        let msgs = vec![
            ChatMessage::text(ChatRole::System, "be terse"),
            ChatMessage::text(ChatRole::User, "hi"),
            ChatMessage::text(ChatRole::Assistant, "hello"),
        ];
        let (system, out) = anthropic_messages(&msgs);
        assert_eq!(system.as_deref(), Some("be terse"));
        assert_eq!(
            Json::Array(out),
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "hello"},
            ])
        );
    }

    #[test]
    fn a_plain_transcript_maps_exactly_as_it_did_for_openai() {
        let msgs = vec![
            ChatMessage::text(ChatRole::System, "be terse"),
            ChatMessage::text(ChatRole::User, "hi"),
        ];
        assert_eq!(
            Json::Array(openai_messages(&msgs)),
            json!([
                {"role": "system", "content": "be terse"},
                {"role": "user", "content": "hi"},
            ])
        );
    }

    // -- the mirror ---------------------------------------------------------

    /// Both vendors, one input: the coalesce and the expand are the same
    /// decision seen from two sides.
    #[test]
    fn parallel_results_coalesce_into_one_anthropic_user_message() {
        let (_, out) = anthropic_messages(&parallel_round());
        // user, assistant(2 tool_use), user(2 tool_result)
        assert_eq!(out.len(), 3);
        let results = &out[2];
        assert_eq!(results["role"], "user");
        assert_eq!(results["content"].as_array().unwrap().len(), 2);
        assert_eq!(results["content"][0]["type"], "tool_result");
        // Each result names the call it answers, positionally.
        assert_eq!(
            results["content"][0]["tool_use_id"],
            out[1]["content"][0]["id"]
        );
        assert_eq!(
            results["content"][1]["tool_use_id"],
            out[1]["content"][1]["id"]
        );
    }

    #[test]
    fn parallel_results_expand_into_two_openai_tool_messages() {
        let out = openai_messages(&parallel_round());
        // user, assistant(2 tool_calls), tool, tool
        assert_eq!(out.len(), 4);
        assert_eq!(out[2]["role"], "tool");
        assert_eq!(out[3]["role"], "tool");
        assert_eq!(out[2]["tool_call_id"], out[1]["tool_calls"][0]["id"]);
        assert_eq!(out[3]["tool_call_id"], out[1]["tool_calls"][1]["id"]);
    }

    /// The whole point of [`synthetic_id`]: an author writes no ids, and both
    /// vendors still get the correlation they require.
    #[test]
    fn a_transcript_with_no_ids_still_correlates_on_both_vendors() {
        let (_, anthropic) = anthropic_messages(&parallel_round());
        let openai = openai_messages(&parallel_round());
        assert_eq!(anthropic[1]["content"][0]["id"], "domarinn_call_1_0");
        assert_eq!(
            anthropic[2]["content"][0]["tool_use_id"],
            "domarinn_call_1_0"
        );
        assert_eq!(openai[1]["tool_calls"][0]["id"], "domarinn_call_1_0");
        assert_eq!(openai[2]["tool_call_id"], "domarinn_call_1_0");
    }

    #[test]
    fn an_explicit_tool_call_id_wins_over_the_derived_one() {
        let msgs = vec![
            assistant_with(vec![ToolCall {
                id: Some("call_abc".into()),
                name: "lookup".into(),
                arguments: json!({}),
            }]),
            tool_result("shipped"),
        ];
        let (_, out) = anthropic_messages(&msgs);
        assert_eq!(out[0]["content"][0]["id"], "call_abc");
        assert_eq!(out[1]["content"][0]["tool_use_id"], "call_abc");
    }

    // -- the round-trip pair ------------------------------------------------

    /// The highest-value test here: what we emit, the inbound parser reads back
    /// as the same calls. Any future divergence between the two directions
    /// fails immediately, which is the exact bug this feature invites.
    #[test]
    fn anthropic_blocks_round_trip_through_the_inbound_parser() {
        let calls = vec![call("lookup_order", json!({"order_id": 1042}))];
        let (_, out) = anthropic_messages(&[assistant_with(calls.clone())]);
        let blocks = out[0]["content"].as_array().unwrap();
        let parsed = crate::anthropic::tool_calls_from_blocks(blocks);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "lookup_order");
        assert_eq!(parsed[0].arguments, calls[0].arguments);
    }

    #[test]
    fn openai_tool_calls_round_trip_through_the_inbound_parser() {
        let calls = vec![call("lookup_order", json!({"order_id": 1042}))];
        let out = openai_messages(&[assistant_with(calls.clone())]);
        let parsed = crate::openai::tool_calls_from_message(&out[0]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "lookup_order");
        // The string went out and came back a decoded object.
        assert_eq!(parsed[0].arguments, calls[0].arguments);
    }

    // -- vendor details -----------------------------------------------------

    #[test]
    fn openai_arguments_go_out_as_a_json_string() {
        let out = openai_messages(&[assistant_with(vec![call("f", json!({"a": 1}))])]);
        let args = &out[0]["tool_calls"][0]["function"]["arguments"];
        assert!(args.is_string(), "expected a JSON string, got {args}");
        assert_eq!(args, &json!("{\"a\":1}"));
    }

    #[test]
    fn a_call_without_arguments_sends_an_empty_object_not_null() {
        let out = openai_messages(&[assistant_with(vec![call("f", Json::Null)])]);
        assert_eq!(
            out[0]["tool_calls"][0]["function"]["arguments"],
            json!("{}")
        );
        let (_, anth) = anthropic_messages(&[assistant_with(vec![call("f", Json::Null)])]);
        assert_eq!(anth[0]["content"][0]["input"], json!({}));
    }

    #[test]
    fn an_assistant_turn_with_only_calls_sends_null_content_to_openai() {
        let out = openai_messages(&[assistant_with(vec![call("f", json!({}))])]);
        assert_eq!(out[0]["content"], Json::Null);
    }

    #[test]
    fn thinking_maps_natively_on_anthropic_and_is_dropped_on_openai() {
        let msgs = vec![ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![
                ContentBlock::Thinking {
                    thinking: "private reasoning".into(),
                    signature: Some("ErUBCk".into()),
                },
                ContentBlock::Text {
                    text: "Let me look.".into(),
                },
            ]),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }];

        let (_, anth) = anthropic_messages(&msgs);
        assert_eq!(anth[0]["content"][0]["type"], "thinking");
        assert_eq!(anth[0]["content"][0]["signature"], "ErUBCk");
        assert_eq!(anth[0]["content"][1]["type"], "text");

        // OpenAI has no equivalent: the reasoning must not reappear as prose.
        let oai = openai_messages(&msgs);
        assert_eq!(oai[0]["content"], json!("Let me look."));
        assert!(!oai[0]["content"].as_str().unwrap().contains("private"));
    }

    /// A thinking block with no signature omits the key rather than sending
    /// `null`, which the API rejects.
    #[test]
    fn an_unsigned_thinking_block_omits_the_signature_key() {
        let msgs = vec![ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::Thinking {
                thinking: "hm".into(),
                signature: None,
            }]),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }];
        let (_, anth) = anthropic_messages(&msgs);
        assert!(anth[0]["content"][0].get("signature").is_none());
    }

    /// `ChatRole::Tool` must never reach a plain arm — Anthropic rejects
    /// `"role": "tool"` outright.
    #[test]
    fn a_tool_turn_never_emits_a_tool_role_to_anthropic() {
        let (_, out) = anthropic_messages(&parallel_round());
        for m in &out {
            assert_ne!(m["role"], "tool", "anthropic cannot accept a tool role");
        }
    }

    #[test]
    fn system_turns_still_hoist_around_tool_turns() {
        let mut msgs = vec![ChatMessage::text(ChatRole::System, "be terse")];
        msgs.extend(parallel_round());
        let (system, out) = anthropic_messages(&msgs);
        assert_eq!(system.as_deref(), Some("be terse"));
        assert!(out.iter().all(|m| m["role"] != "system"));
    }

    /// Anthropic rejects a text block holding only whitespace, so a turn
    /// written `content: " "` beside a call must drop the block, not send it.
    #[test]
    fn a_whitespace_only_text_block_is_dropped_for_anthropic() {
        let turn = ChatMessage {
            role: ChatRole::Assistant,
            content: MessageContent::Text("   ".into()),
            tool_calls: vec![call("lookup", json!({}))],
            tool_call_id: None,
        };
        let (_, out) = anthropic_messages(&[turn]);
        let blocks = out[0]["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1, "only the tool_use block: {blocks:?}");
        assert_eq!(blocks[0]["type"], "tool_use");
    }

    /// A results run that is not preceded by its own call turn must not reuse
    /// the previous round's ids — answering the wrong call silently is worse
    /// than an id the provider can see is unmatched.
    #[test]
    fn an_orphaned_results_run_does_not_reuse_the_previous_rounds_ids() {
        let msgs = vec![
            assistant_with(vec![call("a", json!({})), call("b", json!({}))]),
            tool_result("first"),
            ChatMessage::text(ChatRole::User, "meanwhile"),
            tool_result("orphan"),
        ];
        let (_, out) = anthropic_messages(&msgs);
        let answered: Vec<&str> = out
            .iter()
            .filter(|m| m["content"][0]["type"] == "tool_result")
            .map(|m| m["content"][0]["tool_use_id"].as_str().unwrap())
            .collect();
        assert_eq!(answered.len(), 2);
        assert_ne!(
            answered[0], answered[1],
            "two results must never claim the same call: {answered:?}"
        );
    }
}
