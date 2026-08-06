//! Provider config: the `providers:` entries and the types they are built from.
//!
//! Split out of [`crate::config`] purely for size — that file carries the whole
//! suite schema and this is the largest self-contained piece of it. Everything
//! here is re-exported from `config`, so the YAML and every path that names
//! these types are unchanged.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::config::ParamMap;
use crate::config_request::RequestCfg;

/// A system under test.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Provider {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Providers to try, in order, when this one refuses or fails to answer.
    ///
    /// **Not chained.** A fallback provider's own `fallback:` is ignored when it
    /// is reached as one, which makes a cycle unconstructible rather than
    /// something to detect at run time. `domarinn validate` warns when a target
    /// declares one, so the rule is discoverable while you are writing the
    /// suite rather than at three in the morning.
    ///
    /// `cell.provider_id` stays *this* provider's id whichever link answers, so
    /// `case_key` is stable and an `--against` baseline still joins the same
    /// row. [`crate::result::CaseResult::answered_by_provider_id`] records who
    /// actually replied, and `provider_digest` reflects them — so a fallback
    /// shows up as a `ProviderChanged` in a diff, which is what it is.
    ///
    /// A test's `only_providers` / `skip_providers` is honoured for candidates:
    /// a test that excludes a provider must not reach it through another
    /// provider's back door. `--provider` is not — it chooses which cells run,
    /// and a cell that runs is entitled to its whole chain.
    //
    // Deliberately on the outer struct rather than inside the flattened
    // `ProviderKind`, and `fallback_only` below sits here for the same reason.
    // Two things depend on that: `check_unknown_flatten_keys` picks them up as
    // common keys, and — load-bearing — `build_provider` destructures only
    // `kind` and `id`, so an outer field cannot reach `fingerprint()` or
    // `canonical_request()` and therefore cannot move a single cache key.
    // Moving either inside a variant would invalidate every entry in every
    // store at once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<String>,
    /// Exclude this provider from the run matrix. It expands into no cells and
    /// never faces the credential preflight; it exists only to be reached as
    /// another provider's `fallback:` target.
    ///
    /// This is the third selection axis, independent of the other two: a test's
    /// `only_providers` / `skip_providers` still governs whether a chain may
    /// reach it, and `--provider` cannot name it (that is a usage error — the
    /// flag chooses among cells, and a `fallback_only` provider has none).
    /// `--no-fallback` does not readmit it either: that flag disables chain
    /// *walking*, while matrix membership is static suite configuration.
    //
    // `skip_serializing_if` is load-bearing, not cosmetic: `Provider` is
    // serialized whole into `config_snapshot`, `config_digest` and
    // `digests.providers`, so a bool that always emits `false` would flip
    // every pre-existing suite to `ConfigChanged` under `--against`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fallback_only: bool,
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
        /// Transport-level overrides for the request this provider sends:
        /// headers, how the credential is presented, the path, query parameters,
        /// and fields merged into the body last. See [`RequestCfg`].
        ///
        /// Boxed for the same reason `pricing` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<Box<RequestCfg>>,
        /// Cache pin for this provider — change it to throw away every answer it
        /// has cached.
        ///
        /// A plain string, unlike the `exec` field of the same name: there is no
        /// local program here to take a `$digest:` of, only a vendor's endpoint.
        ///
        /// The reason to set one is that `base_url` is deliberately **not** part
        /// of the cache key. It is recorded on each entry and reported when it
        /// changes — the way `exec` treats a rebuilt program — so that a gateway
        /// and a direct connection to the same API share a cache instead of
        /// paying for the same answers twice. When two `base_url`s answer
        /// *differently*, though — a local stub standing in for a vendor, two
        /// gateways routing one model name to different weights — a report after
        /// the fact is not enough. Give them different salts and they stop
        /// sharing entirely.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
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
        /// Transport-level overrides for the request this provider sends:
        /// headers, how the credential is presented, the path, query parameters,
        /// and fields merged into the body last. See [`RequestCfg`].
        ///
        /// Boxed for the same reason `pricing` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<Box<RequestCfg>>,
        /// Cache pin for this provider — change it to throw away every answer it
        /// has cached.
        ///
        /// A plain string, unlike the `exec` field of the same name: there is no
        /// local program here to take a `$digest:` of, only a vendor's endpoint.
        ///
        /// The reason to set one is that `base_url` is deliberately **not** part
        /// of the cache key. It is recorded on each entry and reported when it
        /// changes — the way `exec` treats a rebuilt program — so that a gateway
        /// and a direct connection to the same API share a cache instead of
        /// paying for the same answers twice. When two `base_url`s answer
        /// *differently*, though — a local stub standing in for a vendor, two
        /// gateways routing one model name to different weights — a report after
        /// the fact is not enough. Give them different salts and they stop
        /// sharing entirely.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
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
        /// Transport-level overrides for the request this provider sends:
        /// headers, how the credential is presented, the path, query parameters,
        /// and fields merged into the body last. See [`RequestCfg`].
        ///
        /// Boxed for the same reason `pricing` is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<Box<RequestCfg>>,
        /// Cache pin for this provider — change it to throw away every answer it
        /// has cached.
        ///
        /// A plain string, unlike the `exec` field of the same name: there is no
        /// local program here to take a `$digest:` of, only a vendor's endpoint.
        ///
        /// The reason to set one is that `base_url` is deliberately **not** part
        /// of the cache key. It is recorded on each entry and reported when it
        /// changes — the way `exec` treats a rebuilt program — so that a gateway
        /// and a direct connection to the same API share a cache instead of
        /// paying for the same answers twice. When two `base_url`s answer
        /// *differently*, though — a local stub standing in for a vendor, two
        /// gateways routing one model name to different weights — a report after
        /// the fact is not enough. Give them different salts and they stop
        /// sharing entirely.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_salt: Option<String>,
    },
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
