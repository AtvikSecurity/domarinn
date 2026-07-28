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
    /// Suite-wide fallback for [`TestCase::cache_salt`], used only by cases that
    /// do not set their own. A *constant* value here busts the whole suite on
    /// every change — it is a fallback, not the granularity mechanism. Note it
    /// does not reach generator-produced cases (they are appended after the
    /// defaults merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_salt: Option<String>,
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

/// Which tokens a `tokens` assertion counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenCount {
    /// Input plus output. The default, and what every existing suite means.
    Total,
    /// Everything the provider bills for, cache reads and writes included. On a
    /// cache-heavy workload this can be several times `total`.
    Billable,
}

/// A per-provider rate override, in USD per million tokens.
///
/// Merged field-wise over the built-in rate for the provider's model, so a
/// suite can correct one stale number without restating a whole price sheet.
/// Exists for the cases a shipped table cannot cover: a proxy or gateway with
/// negotiated rates, a fine-tune, or a model the table has never heard of.
///
/// `f64` is right *here* and nowhere else — a human writes `3.00` in YAML, and
/// it converts to integer micro-dollars once when the provider is built.
///
/// Never reaches a provider's `fingerprint()`, so setting it does not
/// invalidate a single cache entry: cost is not request identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PricingCfg {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_per_mtok: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_1h_per_mtok: Option<f64>,
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
        api_key_env: Option<EnvNames>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
        /// Rate override for this provider's model. See [`PricingCfg`].
        ///
        /// Boxed because `ProviderKind` is reachable from `AssertKind` (an
        /// `llm-rubric` can carry its own grader, which carries a provider), so
        /// five inline `Option<f64>` here would inflate every assertion in
        /// every suite by 80 bytes. Transparent to serde and schemars — the
        /// YAML is unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pricing: Option<Box<PricingCfg>>,
    },
    /// OpenAI-compatible chat-completions client.
    Openai {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base_url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key_env: Option<EnvNames>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        params: Option<ParamMap>,
        /// Rate override for this provider's model. See [`PricingCfg`].
        ///
        /// Boxed because `ProviderKind` is reachable from `AssertKind` (an
        /// `llm-rubric` can carry its own grader, which carries a provider), so
        /// five inline `Option<f64>` here would inflate every assertion in
        /// every suite by 80 bytes. Transparent to serde and schemars — the
        /// YAML is unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pricing: Option<Box<PricingCfg>>,
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
        api_key_env: Option<EnvNames>,
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
    /// Parameter sweep: each axis maps a var name to the list of values it takes.
    /// The case fans out over the cartesian product of its axes (one case per
    /// combination), with each axis value merged into `vars`. See
    /// [`crate::matrix`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub matrix: BTreeMap<String, Vec<Val>>,
    /// Optional minijinja template for a matrix cell's id, rendered against the
    /// axis values (e.g. `"{{ style }}-{{ temperature }}"`). Defaults to
    /// `<base-id>[key=value,…]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assert: Vec<Assert>,
    /// If set, the case passes when its weighted-mean score >= threshold;
    /// otherwise it passes only if every assert passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threshold: Option<f64>,
    /// Opaque per-case cache-busting token, folded into this case's provider
    /// cache key. Use it when the system under test loads content domarinn
    /// cannot see (its own prompt files), so editing that content busts only
    /// the cases that use it. Never sent to the provider. It does not make a
    /// provider cacheable on its own — an `exec` provider still needs its own
    /// `cache_salt` for that.
    ///
    /// Used **verbatim**, and deliberately not templated: a useful salt is a
    /// digest of something domarinn cannot see, so it could only be derived from
    /// the environment — and `env` is deliberately kept out of the request
    /// identity (see [`crate::render`]) so that unrelated environment drift
    /// never busts a shared cache. Keeping it a literal also means it can never
    /// fail to render and is never a template-injection surface. Compute the
    /// digest outside the suite and write the value in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_salt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only_providers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_providers: Vec<String>,
}

/// A single assertion with its common controls. `type: not-<kind>` is sugar for
/// `negate: true` and works in every test source.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Assert {
    #[serde(default = "default_weight")]
    pub weight: f64,
    #[serde(default)]
    pub negate: bool,
    #[serde(flatten)]
    pub kind: AssertKind,
}

/// The un-sugared shape [`Assert`]'s `Deserialize` delegates to, so the impl
/// does not recurse into itself.
#[derive(Deserialize)]
struct AssertRepr {
    #[serde(default = "default_weight")]
    weight: f64,
    #[serde(default)]
    negate: bool,
    #[serde(flatten)]
    kind: AssertKind,
}

// `Deserialize` is hand-written so `type: not-<kind>` is desugared *here*,
// during deserialization, rather than by a walk over the loaded YAML document.
// That walk only ever ran on the composed suite file, so the sugar worked
// inline and failed with `unknown variant \`not-contains\`` from a `file://`
// glob, a JSON or JSONL test file, a CSV `__assert` column, or generator output
// — five paths, against documentation promising it worked for any assertion
// type. Doing it in the impl is reachable by construction: there is no way to
// produce an `Assert` from serialized input that skips it.
//
// It also *narrows* the rewrite. The document walk recursed into every mapping
// with a `type` key, so an `http` provider's `body: {type: "not-null"}`, an
// `exec` assert's `config`, and a generator's `config` were all silently
// rewritten. Now only assertions are.
impl<'de> Deserialize<'de> for Assert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        // Buffered through `serde_json::Value` the same way `TestSource`'s
        // hand-written impl is, so one implementation covers every source
        // format instead of one per serializer.
        let mut value = Json::deserialize(deserializer).map_err(D::Error::custom)?;

        let mut negated_by_sugar = false;
        if let Json::Object(map) = &mut value {
            if let Some(Json::String(ty)) = map.get("type") {
                if let Some(stripped) = ty.strip_prefix("not-").map(str::to_string) {
                    map.insert("type".into(), Json::String(stripped));
                    negated_by_sugar = true;
                }
            }
        }

        let repr: AssertRepr = serde_json::from_value(value).map_err(D::Error::custom)?;
        Ok(Assert {
            weight: repr.weight,
            // `not-` wins over an explicit `negate:`. Two spellings of the same
            // intent disagreeing is a config bug, and `not-` is the more
            // specific one.
            negate: negated_by_sugar || repr.negate,
            kind: repr.kind,
        })
    }
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
        /// Opt this assertion's verdicts into the cache, and bust them when the
        /// value changes.
        ///
        /// Required for the same reason an `exec` *provider* needs one: `command`
        /// does not move when the program behind it is rebuilt, so caching by
        /// default would serve stale verdicts after a rebuild — silently, and in
        /// CI. Set it to something that tracks the program (a git SHA, a build
        /// id, a content digest).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
    },
    LlmRubric {
        value: String,
        /// Boxed because it is by far the largest thing an assertion can carry
        /// (a whole `Grader`, including a `ProviderKind`), and every other
        /// `AssertKind` variant pays for it inline otherwise. Transparent to
        /// serde and schemars — the YAML is unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        grader: Option<Box<Grader>>,
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
    /// Token count must be <= max. See [`TokenCount`] for which tokens.
    Tokens {
        max: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        count: Option<TokenCount>,
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
    /// How long to wait for a verdict, in milliseconds. Defaults to 120s.
    ///
    /// Reachable from config because the ceiling interacts with the grader's
    /// own `max_tokens`: a reasoning grader given room to think can take
    /// longer than a fixed constant allows, and the failure looks like a
    /// transport fault rather than a budget one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
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
    /// Spread concurrent retries so rate-limited cells do not all wake at once.
    #[serde(default)]
    pub jitter: bool,
    /// Largest server-supplied `Retry-After` to honor, in milliseconds
    /// (default 120000).
    ///
    /// Separate from `max_ms` on purpose: `max_ms` caps domarinn's own
    /// exponential growth, whereas `Retry-After` is a directive describing the
    /// server's rate window. A hint above this ceiling errors the case rather
    /// than retrying early, which would only earn another 429.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_max_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RateLimit {
    pub rps: f64,
}

/// One or more environment variable names holding an API key.
///
/// A bare string is the single-name form and serializes back to a bare string,
/// so every existing suite's `config_digest` is unchanged and no `--against`
/// comparison shows spurious drift. Pinned by
/// `a_single_env_name_serializes_as_a_bare_string`.
///
/// The first name that resolves to a non-empty value wins. Which one that was
/// is logged at debug rather than stored: the run document is shareable, and a
/// variable name is a weak but real signal about someone's environment. That
/// does mean two environments resolving different names produce runs that
/// `config_digest` cannot tell apart — check the log if that matters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvNames {
    One(String),
    Many(Vec<String>),
}

impl EnvNames {
    pub fn iter(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            EnvNames::One(name) => Box::new(std::iter::once(name.as_str())),
            EnvNames::Many(names) => Box::new(names.iter().map(String::as_str)),
        }
    }
}

impl From<&str> for EnvNames {
    fn from(name: &str) -> Self {
        EnvNames::One(name.to_string())
    }
}

/// `serde(default)` helper for a flag that is on unless explicitly disabled.
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default)]
    pub backend: CacheBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Cfg>,
    /// Cache grader verdicts as well as provider responses. Default `true`.
    ///
    /// On by default because it is the dominant recurring cost of an LLM-graded
    /// suite: without it the judge is re-paid on every run even when every
    /// provider response was a cache hit. Every way a verdict could go stale is
    /// in the key — the rubric, the grader's model and endpoint, its params, the
    /// system prompt, and the graded output itself.
    ///
    /// Turn it off to measure judge variance deliberately, or use
    /// `--no-grader-cache` for one run.
    #[serde(default = "default_true")]
    pub grader: bool,
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

#[cfg(test)]
mod env_names_tests {
    use super::*;

    /// The load-bearing back-compat property: a single name must serialize back
    /// to a bare string, or every existing suite's `config_digest` moves and
    /// every `--against` comparison reports drift that did not happen.
    #[test]
    fn a_single_env_name_serializes_as_a_bare_string() {
        let one: EnvNames = serde_json::from_str(r#""ANTHROPIC_API_KEY""#).unwrap();
        assert_eq!(
            serde_json::to_string(&one).unwrap(),
            r#""ANTHROPIC_API_KEY""#
        );
    }

    #[test]
    fn a_list_round_trips_and_preserves_order() {
        let raw = r#"["PRIMARY","FALLBACK"]"#;
        let many: EnvNames = serde_json::from_str(raw).unwrap();
        assert_eq!(serde_json::to_string(&many).unwrap(), raw);
        assert_eq!(many.iter().collect::<Vec<_>>(), vec!["PRIMARY", "FALLBACK"]);
    }

    #[test]
    fn both_forms_iterate_uniformly() {
        assert_eq!(EnvNames::from("ONLY").iter().count(), 1);
        assert_eq!(
            EnvNames::Many(vec!["A".into(), "B".into()]).iter().count(),
            2
        );
    }
}
