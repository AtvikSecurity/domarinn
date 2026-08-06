//! Why an output came back with nothing gradeable in it.
//!
//! An empty output is a *successful* provider call, so nothing upstream raises.
//! Four unrelated causes previously collapsed into the same `""` — a
//! thinking-only response, a `max_tokens` truncation before any text, a refusal,
//! and a genuinely blank answer — and each wants a different fix. This module
//! names them.
//!
//! # Why a string newtype and not an enum
//!
//! [`EmptyReason`] is an open type with associated constants, deliberately, and
//! it must stay that way.
//!
//! The values come from *vendor* finish reasons and grow at model-release
//! cadence, with no domarinn release in the loop. A closed enum would need a new
//! domarinn version every time a provider adds a stop reason — and worse, it
//! would fail *closed*: this value crosses three boundaries where an unknown
//! variant is not a dropped field but a hard failure. It rides in the stored
//! `CaseResult` blob, in the exec protocol response (where domarinn does not
//! control the writer in any version), and through the server's ingest, which
//! accepts a document and then deserializes it whole.
//!
//! `#[serde(other)]` is *not* sufficient either: it collapses unknown values to
//! a catch-all and discards the original string, which breaks the byte-stable
//! re-serialization the stored blob depends on.
//!
//! Every vendor-authored field already on `CaseResult` — `stop_reason`, `raw`,
//! `request` — is open for the same reason. This follows that precedent rather
//! than breaking it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::types::Output;

/// Why an output is empty. Open by construction — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct EmptyReason(pub String);

impl EmptyReason {
    /// The model reasoned but never emitted a final message. Fix: enable
    /// reasoning capture, or raise `max_tokens` so it reaches the answer.
    pub const THINKING_ONLY: &'static str = "thinking_only";
    /// Cut off by `max_tokens` before any text. Fix: raise `max_tokens`.
    pub const TRUNCATED: &'static str = "truncated";
    /// The model declined. Genuine model behavior, not a harness fault.
    pub const REFUSAL: &'static str = "refusal";
    /// A provider-side safety filter removed the content.
    pub const CONTENT_FILTER: &'static str = "content_filter";
    /// The response carried no content blocks at all — a protocol-shaped fault.
    pub const NO_CONTENT_BLOCKS: &'static str = "no_content_blocks";
    /// A 2xx with an empty body.
    pub const EMPTY_BODY: &'static str = "empty_body";
    /// An `output_expr` evaluated to nothing.
    pub const OUTPUT_EXPR_EMPTY: &'static str = "output_expr_empty";
    /// The model called a tool and said nothing else.
    pub const TOOL_USE_ONLY: &'static str = "tool_use_only";
    /// A genuinely blank answer — the model had its chance and said nothing.
    pub const BLANK: &'static str = "blank";

    pub fn new(reason: impl Into<String>) -> Self {
        EmptyReason(reason.into())
    }

    /// Longest reason any surface stores, logs or compares.
    ///
    /// Every real reason is under twenty characters; this is a bound on what an
    /// `exec` child can make the rest of the system carry, not a guess at what
    /// a vendor might name.
    pub const CLAMP_MAX: usize = 64;

    /// The comparable, safe-to-render form: control characters stripped and
    /// length-bounded.
    ///
    /// An `exec` child writes this field verbatim, so unclamped it is a newline
    /// away from forging a log record and unbounded in a facet list. It must
    /// also be the form *every* consumer compares on: the server promotes the
    /// clamped value into a column, so a disk-tier filter comparing the raw one
    /// would make `--empty-reason X` evict on one tier and silently miss on the
    /// other.
    pub fn clamped(&self) -> String {
        let cleaned: String = self.0.chars().filter(|c| !c.is_control()).collect();
        match cleaned.char_indices().nth(Self::CLAMP_MAX) {
            None => cleaned,
            Some((at, _)) => cleaned[..at].to_string(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EmptyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Precedence when a response satisfies more than one reason, most specific
/// first.
///
/// This is not cosmetic. A thinking-only Anthropic response that *also* carries
/// `stop_reason: "max_tokens"` matches both `THINKING_ONLY` and `TRUNCATED`, and
/// those point at **opposite** fixes — "capture the reasoning" versus "raise
/// max_tokens". Reporting whichever the parser happened to check first would
/// send people down the wrong path half the time.
///
/// Truncation wins: it explains *why* only thinking came back.
pub const PRECEDENCE: &[&str] = &[
    EmptyReason::REFUSAL,
    EmptyReason::CONTENT_FILTER,
    EmptyReason::TRUNCATED,
    EmptyReason::TOOL_USE_ONLY,
    EmptyReason::THINKING_ONLY,
    EmptyReason::NO_CONTENT_BLOCKS,
    EmptyReason::EMPTY_BODY,
    EmptyReason::OUTPUT_EXPR_EMPTY,
    EmptyReason::BLANK,
];

/// Pick the most specific of several candidate reasons.
pub fn most_specific(candidates: &[EmptyReason]) -> Option<EmptyReason> {
    PRECEDENCE
        .iter()
        .find_map(|want| candidates.iter().find(|c| c.as_str() == *want).cloned())
        // An unrecognized (future) reason still beats reporting nothing.
        .or_else(|| candidates.first().cloned())
}

/// The empty reason a vendor finish reason implies, for an output that is
/// *already* known to be blank.
///
/// Deliberately vendor-agnostic, and deliberately not exhaustive. An `exec`
/// child may echo one vendor's vocabulary, another's, or its own, and domarinn
/// cannot know which — so this recognizes the spellings both major APIs use and
/// returns `None` for everything else, including clean finishes (`end_turn`,
/// `stop`). Callers fall through to [`classify_blank`] on `None`.
///
/// Returning `None` rather than guessing is the whole contract: an unrecognized
/// finish reason must never be handed a diagnosis, because a wrong diagnosis
/// sends a reader after the wrong fix and is worse than no diagnosis at all.
pub fn from_stop_reason(stop_reason: &str) -> Option<EmptyReason> {
    let reason = match stop_reason.trim().to_ascii_lowercase().as_str() {
        // Anthropic | OpenAI | some gateways
        "max_tokens" | "length" | "content_length" => EmptyReason::TRUNCATED,
        "refusal" => EmptyReason::REFUSAL,
        "content_filter" | "safety" => EmptyReason::CONTENT_FILTER,
        "tool_use" | "tool_calls" | "function_call" => EmptyReason::TOOL_USE_ONLY,
        _ => return None,
    };
    Some(EmptyReason::new(reason))
}

/// The provider-agnostic fallback: is this output devoid of gradeable content?
///
/// `Output::Json` needs care. `Output::as_text()` renders `null` as `"null"` and
/// `{}` as `"{}"` — non-empty strings that carry no answer. Treating them as
/// text would report "not empty" for a response that is exactly the problem
/// being diagnosed.
pub fn classify_blank(output: &Output) -> Option<EmptyReason> {
    let empty = match output {
        Output::Text(s) => s.trim().is_empty(),
        Output::Json(v) => match v {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Array(a) => a.is_empty(),
            serde_json::Value::Object(o) => o.is_empty(),
            _ => false,
        },
    };
    empty.then(|| EmptyReason::new(EmptyReason::BLANK))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The whole point of the newtype: a value this binary has never heard of
    /// must survive a round-trip unchanged. `#[serde(other)]` would collapse it
    /// and break the stored blob's byte-stable re-serialization.
    #[test]
    fn an_unknown_reason_round_trips_unchanged() {
        let raw = r#""some_vendor_reason_invented_later""#;
        let parsed: EmptyReason = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.as_str(), "some_vendor_reason_invented_later");
        assert_eq!(serde_json::to_string(&parsed).unwrap(), raw);
    }

    #[test]
    fn known_reasons_serialize_as_bare_strings() {
        let r = EmptyReason::new(EmptyReason::TRUNCATED);
        assert_eq!(serde_json::to_string(&r).unwrap(), r#""truncated""#);
    }

    #[test]
    fn truncation_outranks_thinking_only() {
        // Opposite fixes: "raise max_tokens" vs "capture the reasoning". Getting
        // this backwards sends people down the wrong path.
        let picked = most_specific(&[
            EmptyReason::new(EmptyReason::THINKING_ONLY),
            EmptyReason::new(EmptyReason::TRUNCATED),
        ])
        .unwrap();
        assert_eq!(picked.as_str(), EmptyReason::TRUNCATED);
    }

    #[test]
    fn a_refusal_outranks_everything() {
        let picked = most_specific(&[
            EmptyReason::new(EmptyReason::BLANK),
            EmptyReason::new(EmptyReason::TRUNCATED),
            EmptyReason::new(EmptyReason::REFUSAL),
        ])
        .unwrap();
        assert_eq!(picked.as_str(), EmptyReason::REFUSAL);
    }

    #[test]
    fn an_unranked_reason_is_still_reported() {
        let picked = most_specific(&[EmptyReason::new("from_the_future")]).unwrap();
        assert_eq!(picked.as_str(), "from_the_future");
    }

    #[test]
    fn from_stop_reason_maps_both_vendor_vocabularies() {
        for (raw, want) in [
            ("max_tokens", EmptyReason::TRUNCATED),
            ("length", EmptyReason::TRUNCATED),
            ("refusal", EmptyReason::REFUSAL),
            ("content_filter", EmptyReason::CONTENT_FILTER),
            ("tool_use", EmptyReason::TOOL_USE_ONLY),
            ("tool_calls", EmptyReason::TOOL_USE_ONLY),
            // Case and padding come from whoever wrote the child, not from us.
            ("  MAX_TOKENS ", EmptyReason::TRUNCATED),
        ] {
            assert_eq!(
                from_stop_reason(raw).map(|r| r.0),
                Some(want.to_string()),
                "mapping {raw}"
            );
        }
    }

    /// A finished answer is not an empty one. Handing `end_turn` a reason would
    /// label every successful call.
    #[test]
    fn from_stop_reason_is_none_for_a_clean_finish() {
        assert!(from_stop_reason("end_turn").is_none());
        assert!(from_stop_reason("stop").is_none());
    }

    /// The open-set half: an unrecognized reason gets no diagnosis rather than
    /// a wrong one, and the caller falls back to `classify_blank`.
    #[test]
    fn from_stop_reason_is_none_for_an_unknown_value() {
        assert!(from_stop_reason("invented_next_year").is_none());
        assert!(from_stop_reason("").is_none());
    }

    #[test]
    fn blank_text_and_whitespace_are_empty() {
        assert!(classify_blank(&Output::Text(String::new())).is_some());
        assert!(classify_blank(&Output::Text("   \n\t ".into())).is_some());
        assert!(classify_blank(&Output::Text("hi".into())).is_none());
    }

    /// `Output::as_text()` renders these as "null" and "{}" — four and two
    /// characters of text that answer nothing.
    #[test]
    fn structurally_empty_json_is_empty_despite_rendering_as_text() {
        assert!(classify_blank(&Output::Json(json!(null))).is_some());
        assert!(classify_blank(&Output::Json(json!({}))).is_some());
        assert!(classify_blank(&Output::Json(json!([]))).is_some());
        assert!(classify_blank(&Output::Json(json!(""))).is_some());

        assert!(classify_blank(&Output::Json(json!({"a": 1}))).is_none());
        assert!(classify_blank(&Output::Json(json!(0))).is_none());
        assert!(classify_blank(&Output::Json(json!(false))).is_none());
    }
}
