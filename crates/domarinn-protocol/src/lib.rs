//! The exec JSON protocol shared by providers, asserts, and generators.
//!
//! One protocol, three kinds. v1 is one-shot: domarinn writes exactly one JSON
//! request to the child's stdin, closes it, and reads one JSON document from
//! stdout. The `domarinn.protocol` field makes the envelope evolvable.
//!
//! # Compatibility
//!
//! Every optional field is `skip_serializing_if`, so a program written against
//! an earlier build of this crate emits a byte-identical document and is parsed
//! identically. New fields are added the same way and the wire version stays
//! `1`. Both sides ignore unknown fields; that is the whole forward-compat
//! story, and it is why none of these types use `deny_unknown_fields`.
//!
//! # Scope
//!
//! Serde shapes only — no I/O, no engine, no schema generation. See this
//! crate's README for why it is separate from `domarinn-types`, and
//! `docs/protocol.md` in the repository for the normative field tables.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_ENV: &str = "DOMARINN_PROTOCOL";

/// The envelope every request carries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol: u32,
    pub kind: Kind,
}

impl Envelope {
    pub fn new(kind: Kind) -> Self {
        Envelope {
            protocol: PROTOCOL_VERSION,
            kind,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Provider,
    Assert,
    GenerateTests,
}

/// A provider request written to the child's stdin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderReq {
    #[serde(rename = "domarinn")]
    pub envelope: Envelope,
    /// `None` when the suite has no prompts (self-input case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Json>,
    #[serde(default)]
    pub vars: Json,
    #[serde(default)]
    pub params: Json,
    pub test: TestRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRef {
    pub id: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A provider response read from the child's stdout. Only `output` is required.
///
/// `Default` is derived so a child can write `ProviderResp { output, ..Default::default() }`
/// and stay correct as optional fields are added — the shape a provider author
/// reaches for first should not have to enumerate fields it does not use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderResp {
    pub output: Json,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Json>,

    /// The vendor's finish reason, verbatim (`end_turn`, `length`, `refusal`,
    /// …). Free-form: this crate never checks it against a list, because the
    /// list grows at model-release cadence with no domarinn release in the loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,

    /// Why `output` has nothing gradeable in it, when it does not.
    ///
    /// Set this when you know — a refusal, a truncation, a tool call with no
    /// prose — rather than returning an empty string and letting every
    /// assertion score zero for a reason that has nothing to do with the
    /// prompt. See `docs/protocol.md` for the known values; an unrecognized one
    /// is carried through verbatim and is never an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub empty_reason: Option<String>,

    /// The model's reasoning or thinking text, when the child can expose it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,

    /// The model actually used, as opposed to the one requested — an alias that
    /// silently repoints to a new snapshot has no other signal.
    ///
    /// Response metadata, not request identity: it never enters a cache key,
    /// because a key cannot depend on something learned after the call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    /// Input tokens served from a provider-side prompt cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    /// Input tokens written *into* a provider-side prompt cache. Billed at a
    /// premium over an ordinary input token, so a harness that cannot see this
    /// under-reports cost on exactly the calls that populate the cache.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    /// The subset of `cache_write_tokens` written at a longer-lived TTL, when
    /// the provider reports the split. Absent means "all at the default TTL".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProtocolError {
    pub message: String,
    #[serde(default)]
    pub retriable: bool,

    /// Structured diagnostics for the failure — the machine-readable half of
    /// `message`.
    ///
    /// Worth its own field because `message` is prose: without this, a child
    /// with anything structured to say has to format JSON into a sentence and
    /// hope the reader parses it back out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Json>,

    /// What kind of failure this was, using domarinn's error-class vocabulary
    /// (`provider_auth`, `provider_rate_limit`, `provider_timeout`, …).
    ///
    /// Without it every exec failure is indistinguishable from every other, so
    /// a child that knows perfectly well its credential was rejected cannot say
    /// so. Unrecognized values are kept verbatim, same as `empty_reason`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,

    /// How long to wait before retrying, in milliseconds — a `Retry-After` the
    /// child received and would otherwise have to swallow. Only meaningful
    /// alongside `retriable: true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

/// An assert request (GradingResult-shaped response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertReq {
    #[serde(rename = "domarinn")]
    pub envelope: Envelope,
    pub output: Json,
    pub test: TestRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Json>,
    pub provider: ProviderRef,
    #[serde(default)]
    pub config: Json,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderRef {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertResp {
    pub pass: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Json>,
}

/// A generate-tests request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateReq {
    #[serde(rename = "domarinn")]
    pub envelope: Envelope,
    #[serde(default)]
    pub config: Json,
}

/// A generate-tests response (JSON object form; JSONL is also accepted).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateResp {
    pub tests: Vec<Json>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_request_round_trips() {
        let req = ProviderReq {
            envelope: Envelope::new(Kind::Provider),
            prompt: None,
            vars: serde_json::json!({"x": 1}),
            params: serde_json::json!({}),
            test: TestRef {
                id: "t".into(),
                tags: vec!["a".into()],
            },
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"domarinn\""));
        assert!(s.contains("\"protocol\":1"));
        let back: ProviderReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.test.id, "t");
    }

    /// The byte-shape guard behind the compatibility promise in the module doc:
    /// a response that sets nothing but `output` must not grow keys on the wire.
    /// Every future optional field has to keep this passing.
    #[test]
    fn a_minimal_response_serializes_to_exactly_output() {
        let resp = ProviderResp {
            output: Json::String("hi".into()),
            ..Default::default()
        };
        assert_eq!(serde_json::to_string(&resp).unwrap(), r#"{"output":"hi"}"#);
    }

    /// The other half of forward compatibility: a document from a *newer*
    /// domarinn must parse here rather than erroring, or an upgrade on one side
    /// breaks every provider built against the other.
    #[test]
    fn unknown_response_fields_are_ignored() {
        let resp: ProviderResp =
            serde_json::from_str(r#"{"output":"hi","invented_later":{"a":1}}"#).unwrap();
        assert_eq!(resp.output, Json::String("hi".into()));
    }

    #[test]
    fn usage_defaults_both_counts_to_zero() {
        let usage: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
    }
}
