//! Canned vendor responses for the networked examples.
//!
//! Cut to the fields the parsers actually read. A body copied wholesale from a
//! real API response documents nothing about which parts are load-bearing, and
//! the next person adding an example cannot tell what they may drop — so every
//! field below is here because some named line of the client reads it.
//!
//! Routing fragments, matched against the request line:
//!
//! | Fragment             | Reached by                                        |
//! |----------------------|---------------------------------------------------|
//! | `/v1/messages`       | `type: anthropic`, and an Anthropic `llm-rubric`   |
//! | `/chat/completions`  | `type: openai`, and an OpenAI `llm-rubric`         |
//! | `/embeddings`        | the `similar` assertion's embeddings provider      |
//!
//! Note the asymmetry that trips people up: the Anthropic client appends
//! `/v1/messages` to a bare base url, while the OpenAI client appends only
//! `/chat/completions` because its real default base already ends in `/v1`.
//! That is why the table has both `Env::StubBase` and `Env::StubBaseV1`.

#![allow(dead_code)]

/// Anthropic Messages, plain text.
///
/// Bare minimum: `{"content":[{"type":"text","text":"…"}]}`.
///
/// `usage` is what makes a `tokens:` assertion mean anything and is the only
/// way `cost_usd` comes to exist — without it a `cost:` budget passes as "cost
/// not reported; budget not enforced", which is green and enforces nothing.
pub const ANTHROPIC_TEXT: &str = r#"{
  "id": "msg_stub",
  "type": "message",
  "role": "assistant",
  "model": "claude-haiku-4-5",
  "content": [{"type": "text", "text": "Returns are accepted within 30 days of delivery."}],
  "stop_reason": "end_turn",
  "usage": {"input_tokens": 24, "output_tokens": 11}
}"#;

/// Anthropic Messages, refusing.
pub const ANTHROPIC_REFUSAL: &str = r#"{
  "id": "msg_stub",
  "type": "message",
  "role": "assistant",
  "model": "claude-haiku-4-5",
  "content": [{"type": "text", "text": "I can't help with that — it falls outside what this account covers."}],
  "stop_reason": "end_turn",
  "usage": {"input_tokens": 26, "output_tokens": 15}
}"#;

/// An Anthropic `llm-rubric` verdict, passing.
///
/// The grader asks for a tool call and reads the first `tool_use` block's
/// `input`, which must carry `pass` (bool) and `score` (number); `reasoning`
/// defaults to empty.
///
/// `stop_reason` must NOT be `max_tokens` — the grader treats a truncated
/// verdict as a fail-closed error before it ever looks at the blocks.
pub const ANTHROPIC_VERDICT_PASS: &str = r#"{
  "id": "msg_stub",
  "type": "message",
  "role": "assistant",
  "model": "claude-haiku-4-5",
  "content": [{
    "type": "tool_use",
    "id": "toolu_stub",
    "name": "submit_verdict",
    "input": {
      "reasoning": "States the 30-day window and offers a next step, without inventing a policy.",
      "pass": true,
      "score": 1.0
    }
  }],
  "stop_reason": "tool_use",
  "usage": {"input_tokens": 180, "output_tokens": 42}
}"#;

/// An Anthropic `llm-rubric` verdict, failing — with a fractional score, so a
/// `threshold` on the assertion is exercised rather than merely present.
pub const ANTHROPIC_VERDICT_PARTIAL: &str = r#"{
  "id": "msg_stub",
  "type": "message",
  "role": "assistant",
  "model": "claude-haiku-4-5",
  "content": [{
    "type": "tool_use",
    "id": "toolu_stub",
    "name": "submit_verdict",
    "input": {
      "reasoning": "Declines correctly but offers no alternative, so it is only half the required behaviour.",
      "pass": false,
      "score": 0.5
    }
  }],
  "stop_reason": "tool_use",
  "usage": {"input_tokens": 176, "output_tokens": 51}
}"#;

/// Anthropic Messages, calling a tool — the native-API sibling of example 15's
/// exec `tool_calls`.
///
/// `content` carries one `tool_use` block; the client reads its `name` and
/// its already-decoded `input` (Anthropic sends the arguments as JSON, not a
/// string) into a `ToolCall`, in block order. `stop_reason` is `tool_use`
/// rather than `end_turn` — expected here, not an error, since the model's
/// whole answer is the call.
pub const ANTHROPIC_TOOL_USE: &str = r#"{
  "id": "msg_stub",
  "type": "message",
  "role": "assistant",
  "model": "claude-haiku-4-5",
  "content": [{
    "type": "tool_use",
    "id": "toolu_stub",
    "name": "lookup_order",
    "input": {"order_id": 1042}
  }],
  "stop_reason": "tool_use",
  "usage": {"input_tokens": 210, "output_tokens": 24}
}"#;

/// OpenAI chat-completions, plain text. The client reads
/// `choices[0].message.content`.
///
/// Bare minimum: `{"choices":[{"message":{"content":"…"}}]}`.
pub const OPENAI_TEXT: &str = r#"{
  "id": "chatcmpl-stub",
  "object": "chat.completion",
  "model": "gpt-4o-mini",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Paris is the capital of France."},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 18, "completion_tokens": 7, "total_tokens": 25}
}"#;

/// A second OpenAI answer, so a two-case example is not served the same body
/// twice and cannot pass by coincidence.
pub const OPENAI_TEXT_ALT: &str = r#"{
  "id": "chatcmpl-stub",
  "object": "chat.completion",
  "model": "gpt-4o-mini",
  "choices": [{
    "index": 0,
    "message": {"role": "assistant", "content": "Oslo is the capital of Norway."},
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 18, "completion_tokens": 7, "total_tokens": 25}
}"#;

/// An OpenAI `llm-rubric` verdict, passing — the `type: openai` grader's
/// counterpart to [`ANTHROPIC_VERDICT_PASS`].
///
/// The grader's `response_format` forces strict `json_schema` mode, so unlike
/// [`OPENAI_TEXT`] the content is not prose: `choices[0].message.content` is
/// the verdict itself, already JSON — but still carried as a STRING (OpenAI's
/// structured-output modes return a JSON string, not a nested object), which
/// is why the grader parses it with `serde_json::from_str` rather than
/// reading it as a value directly.
///
/// `finish_reason` must not be `length` — the grader treats that as a
/// truncated, fail-closed error before it ever parses the content.
pub const OPENAI_VERDICT_PASS: &str = r#"{
  "id": "chatcmpl-stub",
  "object": "chat.completion",
  "model": "gpt-4o-mini",
  "choices": [{
    "index": 0,
    "message": {
      "role": "assistant",
      "content": "{\"reasoning\": \"States the 30-day window and offers a separate partial-credit check, without promising any exception.\", \"pass\": true, \"score\": 1.0}"
    },
    "finish_reason": "stop"
  }],
  "usage": {"prompt_tokens": 210, "completion_tokens": 48, "total_tokens": 258}
}"#;

/// An embeddings vector. The client reads `data[0].embedding` and rejects an
/// empty array.
///
/// [`EMBED_A`] and [`EMBED_NEAR_A`] are deliberately DIFFERENT: `similar`
/// embeds the output and the reference and takes their cosine, so one repeated
/// body would score 1.0 against itself and the threshold on the page would go
/// untested at every value. cos(EMBED_A, EMBED_NEAR_A) ≈ 0.9487 — above a 0.85
/// threshold, below 0.97.
pub const EMBED_A: &str = r#"{
  "object": "list",
  "model": "text-embedding-3-small",
  "data": [{"object": "embedding", "index": 0, "embedding": [1.0, 0.0, 0.0]}],
  "usage": {"prompt_tokens": 8, "total_tokens": 8}
}"#;

pub const EMBED_NEAR_A: &str = r#"{
  "object": "list",
  "model": "text-embedding-3-small",
  "data": [{"object": "embedding", "index": 0, "embedding": [3.0, 1.0, 0.0]}],
  "usage": {"prompt_tokens": 9, "total_tokens": 9}
}"#;

/// A plain JSON service response, for the `http` provider — which needs a live
/// socket like any other network provider but, unlike them, no vendor-shaped
/// body: `output_expr` selects the answer out of whatever shape you return.
pub const SERVICE_REPLY: &str = r#"{
  "request_id": "req_stub",
  "result": {
    "reply": "Your order 1042 shipped on Tuesday and arrives Thursday.",
    "confidence": 0.93
  }
}"#;
