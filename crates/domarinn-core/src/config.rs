//! The declarative suite schema (the YAML a user writes).
//!
//! Every type here derives [`schemars::JsonSchema`] so the published JSON Schema
//! is generated from the same structs the loader deserializes into — the schema
//! cannot drift from the code.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::types::ChatRole;
use crate::val::Val;

/// A free-form bag of provider/grader parameters passed to the model verbatim.
pub type ParamMap = serde_json::Map<String, Json>;

fn default_weight() -> f64 {
    1.0
}

/// The top-level eval suite.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    /// Config schema version. Currently always `1`.
    pub version: u32,
    /// Project namespace (groups runs on the server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Suite name (names the run's suite).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suite: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A base suite to deep-merge on top of (composition).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<String>,
    /// Reusable fragments (named assert-sets, shared providers) to import.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// The systems under test. At least one is required.
    pub providers: Vec<Provider>,
    /// Optional prompts. Omit when a provider constructs its own input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<Prompt>,
    /// Test sources: inline cases, `file://` globs, or generator commands.
    #[serde(default)]
    pub tests: Vec<TestSource>,
    /// Values merged into every test.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<Defaults>,
    /// Default grader for `llm-rubric` assertions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grader: Option<Grader>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<Runner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheCfg>,
}

/// Values merged into every test before it runs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    pub vars: BTreeMap<String, Val>,
    #[serde(default)]
    pub assert: Vec<Assert>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
}

/// A system under test.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Provider {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(flatten)]
    pub kind: ProviderKind,
}

/// The HTTP method for a `type: http` provider. Authors may write either case
/// (`get` or `GET`) in YAML; the wire method (and the request/cache
/// fingerprint) is always the uppercase form.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    #[serde(alias = "get")]
    Get,
    #[default]
    #[serde(alias = "post")]
    Post,
    #[serde(alias = "put")]
    Put,
    #[serde(alias = "patch")]
    Patch,
    #[serde(alias = "delete")]
    Delete,
    #[serde(alias = "head")]
    Head,
}

/// The behavior of a provider, selected by `type`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderKind {
    /// Spawn an external command speaking the exec JSON protocol.
    Exec {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_ms: Option<u64>,
        /// Cache-busting token (e.g. a git SHA or binary hash). Without it, exec
        /// providers default to no-cache so a rebuilt binary is never stale.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
    },
    /// Native Anthropic Messages API client.
    Anthropic {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_env: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
    },
    /// OpenAI-compatible chat-completions client.
    Openai {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_env: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
    },
    /// Arbitrary HTTP endpoint.
    Http {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        method: Option<HttpMethod>,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        body: Option<Json>,
        /// minijinja expression selecting the output from the response.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_expr: Option<String>,
    },
    /// Embeddings endpoint used by the `similar` assertion.
    Embeddings {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_env: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
    },
}

/// A prompt template. Exactly one of `template` / `messages` must be set.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<Message>>,
}

/// A chat message; `content` may be `file://path` to load from disk.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: ChatRole,
    pub content: String,
}

/// An entry in the `tests:` list.
///
/// `Deserialize` is hand-written (rather than derived as `#[serde(untagged)]`)
/// so a typo inside an inline test case surfaces the precise unknown-field
/// error from [`TestCase`]'s `deny_unknown_fields` instead of the opaque
/// "data did not match any variant of untagged enum" an untagged derive emits.
/// `Serialize`/`JsonSchema` keep the untagged shape (a glob string, a
/// `{generator: ...}` mapping, or an inline case mapping).
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum TestSource {
    /// A `file://glob` reference to YAML/JSON/CSV/JSONL test files.
    Glob(String),
    /// A generator command that emits test cases as JSON.
    Generator(GeneratorWrap),
    /// An inline test case.
    Inline(TestCase),
}

impl<'de> Deserialize<'de> for TestSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        // Peek the value's shape, then deserialize the chosen variant directly
        // so its own (deny-guarded) error message propagates verbatim.
        let value = Json::deserialize(deserializer)?;
        match value {
            Json::String(s) => Ok(TestSource::Glob(s)),
            Json::Object(ref map) if map.contains_key("generator") => {
                GeneratorWrap::deserialize(value)
                    .map(TestSource::Generator)
                    .map_err(D::Error::custom)
            }
            Json::Object(_) => TestCase::deserialize(value)
                .map(TestSource::Inline)
                .map_err(D::Error::custom),
            other => Err(D::Error::custom(format!(
                "a test source must be a file:// glob string or a mapping, found {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratorWrap {
    pub generator: GeneratorSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GeneratorSpec {
    pub command: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// A single test case.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, Val>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assert: Vec<Assert>,
    /// If set, the case passes when its weighted-mean score >= threshold;
    /// otherwise it passes only if every assert passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_providers: Vec<String>,
}

/// A single assertion with its common controls.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Assert {
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub negate: bool,
    #[serde(flatten)]
    pub kind: AssertKind,
}

/// The assertion behavior, selected by `type`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AssertKind {
    Contains {
        value: String,
    },
    Icontains {
        value: String,
    },
    IcontainsAny {
        values: Vec<String>,
    },
    Regex {
        value: String,
    },
    Equals {
        value: Val,
    },
    StartsWith {
        value: String,
    },
    IsJson,
    ContainsJson {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Val>,
    },
    Length {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u64>,
    },
    Jinja {
        value: String,
    },
    Exec {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<Json>,
    },
    LlmRubric {
        value: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grader: Option<Grader>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
    },
    /// Total cost in USD must be <= max. Bypasses the cache.
    Cost {
        max: f64,
    },
    /// Latency in ms must be <= max. Bypasses the cache.
    Latency {
        max: u64,
    },
    /// Total tokens must be <= max.
    Tokens {
        max: u64,
    },
    /// Embedding cosine similarity to a reference must be >= threshold.
    Similar {
        value: Val,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        threshold: Option<f64>,
    },
}

/// How an `llm-rubric` grader's structured verdict is obtained. `Forced` is
/// the default; `Auto` is documented but currently unread by the grader.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VerdictMode {
    #[default]
    Forced,
    Auto,
}

/// The LLM grader for `llm-rubric`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Grader {
    /// The grading model. Should be a different model family than the SUT.
    pub provider: ProviderKind,
    /// Optional override of the built-in grading-prompt template (`file://`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// `forced` (default) or `auto` — how the structured verdict is obtained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict_mode: Option<VerdictMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<RetryCfg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Whether deterministic asserts short-circuit the grader. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short_circuit: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RetryCfg {
    pub max: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<u64>,
    #[serde(default)]
    pub jitter: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub rps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default)]
    pub backend: CacheBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Cfg>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackendKind {
    #[default]
    Disk,
    Layered,
    Http,
    S3,
}

/// Non-secret S3 settings; credentials come from the environment / AWS chain.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct S3Cfg {
    pub bucket: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    #[serde(default)]
    pub force_path_style: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_method_defaults_to_post() {
        assert_eq!(HttpMethod::default(), HttpMethod::Post);
    }

    #[test]
    fn http_method_emits_uppercase_and_accepts_lowercase_alias() {
        for (variant, upper, lower) in [
            (HttpMethod::Get, "GET", "get"),
            (HttpMethod::Post, "POST", "post"),
            (HttpMethod::Put, "PUT", "put"),
            (HttpMethod::Patch, "PATCH", "patch"),
            (HttpMethod::Delete, "DELETE", "delete"),
            (HttpMethod::Head, "HEAD", "head"),
        ] {
            assert_eq!(
                serde_json::to_value(variant).unwrap(),
                serde_json::json!(upper)
            );
            let from_upper: HttpMethod = serde_json::from_value(serde_json::json!(upper)).unwrap();
            assert_eq!(from_upper, variant);
            let from_lower: HttpMethod = serde_json::from_value(serde_json::json!(lower)).unwrap();
            assert_eq!(from_lower, variant);
        }
        assert!(serde_json::from_value::<HttpMethod>(serde_json::json!("Trace")).is_err());
    }

    #[test]
    fn verdict_mode_round_trips_and_defaults_to_forced() {
        assert_eq!(VerdictMode::default(), VerdictMode::Forced);
        for (variant, wire) in [(VerdictMode::Forced, "forced"), (VerdictMode::Auto, "auto")] {
            assert_eq!(
                serde_json::to_value(variant).unwrap(),
                serde_json::json!(wire)
            );
            let parsed: VerdictMode = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(parsed, variant);
        }
        assert!(serde_json::from_value::<VerdictMode>(serde_json::json!("manual")).is_err());
    }
}
