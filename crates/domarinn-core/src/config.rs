//! The declarative suite schema (the YAML a user writes).
//!
//! Every type here derives [`schemars::JsonSchema`] so the published JSON Schema
//! is generated from the same structs the loader deserializes into — the schema
//! cannot drift from the code.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

pub use crate::config_history::{HistoryMarker, HistorySpec, PromptEntry};
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
    /// Tools every provider in this suite may offer the model.
    ///
    /// Declaring a tool does not make domarinn run one — it never executes a
    /// tool and never feeds a result back. What it wants is the model's
    /// *decision*, reported as `tool_calls` and graded by `tool-call`
    /// assertions. Suite-level rather than per-test because the tool surface is
    /// a property of the system being evaluated, and varying it per case would
    /// make two cases incomparable while looking like the same suite.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
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

/// A tool the model may call.
///
/// Anthropic's field names: `input_schema` says what the value is (a JSON
/// Schema) where OpenAI's `parameters` does not, and one shape had to win. The
/// mapping to the other vendor is mechanical and lives in `openai.rs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON Schema for the tool's arguments. Passed to the provider verbatim
    /// and never rendered — it is a contract, not a per-case value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Json>,
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
    /// do not set their own.
    ///
    /// A `cache_salt` joins the cache key of every case that does not set its
    /// own salt, generator-produced cases included; change the salt, re-run
    /// exactly those requests.
    ///
    /// A *constant* value here busts the whole suite on every change — it is a
    /// fallback, not the granularity mechanism.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_salt: Option<String>,
    /// Suite-wide fallback for [`TestCase::history`], used only by cases that
    /// do not set their own (fill-if-unset, never concatenated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistorySpec>,
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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
        /// Version pin for the program behind `command` — set it when you
        /// rebuild that program and want the old answers thrown away.
        ///
        /// A `cache_salt` joins the cache key of every request this provider
        /// answers; change the salt, re-run exactly those requests.
        ///
        /// The cache key hashes the request this provider would send — the
        /// `command` and its args, the stdin document (the prompt, the vars,
        /// the params, the tools), and a digest of any declared `env` — and the
        /// salt joins that hash as its own member. It deliberately says nothing
        /// about the program's *bytes*, so that a key is identical on every
        /// machine and a shared cache actually gets shared. The price is that
        /// domarinn cannot tell one build of `./sut` from the next, and this
        /// field is how you tell it: a git SHA, a release tag, or
        /// `"$digest: src/**/*.rs"`.
        ///
        /// A `$digest:` salt is resolved to the blake3 of the matched files, in
        /// sorted order, with their relative paths interleaved — so it pins to
        /// the sources rather than to what the compiler produced (Rust builds are
        /// not byte-reproducible, so two machines compiling identical source
        /// disagree about the artifact while agreeing about the version). Unlike
        /// a *case's* `$digest:`, the glob is **not templated** — a provider has
        /// no vars to render against — and it resolves against the suite
        /// directory, which it may not escape. A glob matching nothing is an
        /// error, not an empty digest.
        ///
        /// Leave it unset for a program that is not changing under you. A run
        /// warns when a cached answer came from a different build than the one
        /// on disk, so a forgotten pin is reported rather than silent.
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
        /// Rate override. Only `input_per_mtok` is read: an embedding call has
        /// no output tokens and reports no cache counters, so the other fields
        /// would price components that do not exist.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pricing: Option<Box<PricingCfg>>,
    },
}

/// A prompt template. Exactly one of `template` / `messages` must be set.
///
/// A `messages:` entry is either a `{role, content}` turn or the bare-string
/// `history` marker naming where each case's [`TestCase::history`] turns
/// splice in (at most one marker; without one, history lands after the
/// leading run of `system` turns). See [`crate::config_history`].
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prompt {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<PromptEntry>>,
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
    /// Opaque per-case cache-busting token. Use it when the system under test
    /// loads content domarinn cannot see (its own prompt files), so editing
    /// that content busts only the cases that use it. Never sent to the
    /// provider.
    ///
    /// A `cache_salt` joins the cache key of this case's provider request(s);
    /// change the salt, re-run exactly those requests.
    ///
    /// This keys *one case*, and is a different lever from a provider's own
    /// `cache_salt`, which pins the program behind a command. Neither is a
    /// prerequisite for the other: every provider is cached by default.
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
    /// This case's prior conversation, spliced into the prompt at its
    /// `history` marker (or after the leading `system` turns when the prompt
    /// has no marker). Inline turns or `file://transcript.yaml`; each turn's
    /// `content` is templated against the case's vars like a prompt turn.
    /// Kept out of the serialized config when unset (digest stability).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<HistorySpec>,
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
        /// Version pin for this grader — set it when you rebuild the program
        /// behind `command` and want its old verdicts thrown away.
        ///
        /// A `cache_salt` joins the cache key of this assertion's grading
        /// requests; change the salt, re-run exactly those requests.
        ///
        /// Same rule as an `exec` *provider*'s: verdicts are cached by default,
        /// and the key says nothing about the grader program's bytes, so a
        /// rebuilt grader keeps answering from the entries it already wrote.
        /// This field is how you retire them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
    },
    /// Assert that the model decided to call a tool.
    ///
    /// A *decision*, never an execution: domarinn does not run tools. This
    /// grades what the model chose to do, which for a whole class of cases is
    /// the only correct answer there is — before this, such a case produced no
    /// prose, scored zero against every text assertion, and read as a model
    /// failure rather than an evaluation that could not see the answer.
    ///
    /// Combine with `negate` (or the `not-tool-call` sugar) for the equally
    /// important negative: the model must *not* have called `delete_account`.
    ToolCall {
        /// The tool that must have been called.
        name: String,
        /// Argument values that must all be present and equal. A subset match,
        /// not an equality check — an assertion should not have to restate
        /// every argument to pin the one that matters.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Val>,
        /// A JSON Schema the call's arguments must satisfy. Not rendered, for
        /// the same reason `contains-json`'s is not: a schema is a contract,
        /// not a per-case value.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<Val>,
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
    /// Whether the judge is shown the tool calls the model made. Default false.
    ///
    /// Opt-in rather than automatic because the grading prompt *is* the judge's
    /// cache key: turning this on for everyone would re-grade every warm entry
    /// in every store, and pay for it, the first time a suite reported a call.
    /// Left unset, the prompt — and therefore the key — is byte-identical to
    /// what it was before this field existed.
    ///
    /// Set it on the `grader:` block the assertion actually resolves to. A
    /// per-assert `grader:` replaces the suite-level block whole rather than
    /// merging field by field, so an assertion that overrides the grader at all
    /// must restate this to keep it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_tool_calls: Option<bool>,
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
    /// Empty reasons that mark a case `skip` instead of grading it.
    ///
    /// A blank output is a *successful* call, so it gets graded and scores zero
    /// against every assertion — for a reason that may have nothing to do with
    /// the prompt. `skip` is the status for "this cell was not gradeable, and
    /// that is not a verdict about the prompt": it is counted separately, and
    /// does not drag a pass rate down.
    ///
    /// Opt-in and empty by default, because which reasons qualify is genuinely
    /// suite-specific and domarinn should not invent the policy. A refusal is
    /// usually a real result you want graded; `tool_use_only` against a harness
    /// that declares no tools usually is not.
    ///
    /// ```yaml
    /// runner:
    ///   skip_on_empty_reason: [tool_use_only]
    /// ```
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_on_empty_reason: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CacheCfg {
    #[serde(default)]
    pub backend: CacheBackendKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub s3: Option<S3Cfg>,
    /// Deprecated: prefer the `--no-grader-cache` run flag. When set, `false`
    /// disables grader-request caching for every run of this suite. Accepted
    /// with a warning.
    ///
    /// Grader requests are cached by default because they are the dominant
    /// recurring cost of an LLM-graded suite: without it the judge is re-paid on
    /// every run even when every provider response was a cache hit. Every way a
    /// verdict could go stale is in the key — the rubric, the grader's model and
    /// endpoint, its params, the system prompt, and the graded output itself.
    //
    // Not serialized when unset, where the old `bool` always wrote `true`. That
    // moves `config_digest` (a hash of the serialized suite) once, for suites
    // that have a `cache:` block and never wrote this key — so one `--against`
    // comparison across the upgrade reports config drift. Cache keys are built
    // from the outgoing request — a grader-originated one like any other — not
    // from this digest, so nothing is invalidated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grader: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CacheBackendKind {
    /// Local content-addressed cache only.
    #[default]
    Disk,
    /// Local disk fronted by a shared remote: auto-picks S3 when `cache.s3` is
    /// set, else the HTTP results server (via
    /// `--server-url`/`DOMARINN_SERVER_URL`).
    Layered,
    /// Deprecated alias for `layered`, removed in a future release. It names the
    /// results server outright, so it ignores a `cache.s3` block that `layered`
    /// would have used.
    #[deprecated(note = "use `layered`; `http` ignores a `cache.s3` block")]
    Http,
    /// Deprecated alias for `layered`, removed in a future release. It names the
    /// object store outright, so without a `cache.s3` block it degrades to local
    /// disk alone where `layered` would have used the results server.
    #[deprecated(note = "use `layered`; `s3` without `cache.s3` is local disk alone")]
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

    /// Deprecating the aliases must not retire them: a suite that says
    /// `backend: http` still loads, and still loads as `Http` — remapping it to
    /// `Layered` here would change how a missing `cache.s3` degrades and which
    /// warning says so.
    #[test]
    #[allow(deprecated)]
    fn the_deprecated_backend_aliases_still_deserialize_to_their_own_variants() {
        for (wire, variant) in [
            ("disk", CacheBackendKind::Disk),
            ("layered", CacheBackendKind::Layered),
            ("http", CacheBackendKind::Http),
            ("s3", CacheBackendKind::S3),
        ] {
            let parsed: CacheBackendKind = serde_json::from_value(serde_json::json!(wire)).unwrap();
            assert_eq!(
                std::mem::discriminant(&parsed),
                std::mem::discriminant(&variant),
                "`backend: {wire}` must keep parsing as itself"
            );
            assert_eq!(
                serde_json::to_value(&variant).unwrap(),
                serde_json::json!(wire),
                "and must round-trip back to the same spelling"
            );
        }
    }

    /// The tri-state that makes the deprecation warning possible: "unset" and
    /// "explicitly true" are the same *behavior* but not the same *value*, and
    /// only the second is worth warning about.
    #[test]
    fn cache_grader_distinguishes_unset_from_explicitly_set() {
        let unset: CacheCfg = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(unset.grader, None);
        let on: CacheCfg = serde_json::from_value(serde_json::json!({"grader": true})).unwrap();
        assert_eq!(on.grader, Some(true));
        let off: CacheCfg = serde_json::from_value(serde_json::json!({"grader": false})).unwrap();
        assert_eq!(off.grader, Some(false));
        // An unset field stays out of the serialized config, so a suite that
        // never mentioned it does not grow a deprecated key on a round-trip.
        assert_eq!(
            serde_json::to_value(&unset).unwrap(),
            serde_json::json!({"backend": "disk"})
        );
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
