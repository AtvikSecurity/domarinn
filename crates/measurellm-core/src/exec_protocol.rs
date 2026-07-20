//! The exec JSON protocol shared by providers, asserts, and generators.
//!
//! One protocol, three kinds. v1 is one-shot: measurellm writes exactly one JSON
//! request to the child's stdin, closes it, and reads one JSON document from
//! stdout. The `measurellm.protocol` field makes the envelope evolvable.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_ENV: &str = "MEASURELLM_PROTOCOL";

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
    #[serde(rename = "measurellm")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub message: String,
    #[serde(default)]
    pub retriable: bool,
}

/// An assert request (GradingResult-shaped response).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertReq {
    #[serde(rename = "measurellm")]
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
    #[serde(rename = "measurellm")]
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
        assert!(s.contains("\"measurellm\""));
        assert!(s.contains("\"protocol\":1"));
        let back: ProviderReq = serde_json::from_str(&s).unwrap();
        assert_eq!(back.test.id, "t");
    }
}
