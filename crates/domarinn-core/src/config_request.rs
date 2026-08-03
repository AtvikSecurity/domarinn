//! Transport-level customization of a vendor provider's outgoing request.
//!
//! The `anthropic`, `openai`, and `embeddings` providers know how to *shape* a
//! request — fold a prompt into that vendor's message list, read usage and tool
//! calls back out, price the result — but until this existed they also hardcoded
//! the envelope that carries it: two headers for Anthropic, one for OpenAI, and
//! a `format!` for the path. A gateway that wants a different credential scheme,
//! an extra header, or a different route was unreachable without abandoning the
//! vendor provider entirely for `type: http`, which means giving up all of the
//! shaping too.
//!
//! This is the escape hatch that does not cost that. Everything here is
//! transport: who you say you are, where you send it, and what rides alongside
//! the body the provider built.
//!
//! # Which env syntax
//!
//! Both work here, and the choice decides whether the value is part of the
//! question:
//!
//! | Syntax | Resolved | In the cache key | Use for |
//! |---|---|---|---|
//! | `${env:VAR}` | load time, before the provider is built | **yes** | selectors — a tier, a region, a route |
//! | `{{ env.VAR }}` | when the provider is built | **no** | credentials |
//!
//! A credential must *not* separate two teammates' cache entries — they are
//! asking the same question — so it belongs in the second column. A selector
//! must separate them, or the second value silently replays the first's answers.
//! [`crate::request_cfg`] warns when it cannot tell which you meant.
//!
//! # What is not here
//!
//! Case vars. These templates render against `env` alone, once, when the
//! provider is built: provider config is not case data, the same line
//! [`crate::interp`] draws for `${env:}`. A header that varies per case is what
//! `type: http` is for.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// How a provider presents the credential named by its `api_key_env`.
///
/// Each vendor has a default and most suites never name this. It exists for the
/// endpoints that accept the same credential a different way — most immediately,
/// an Anthropic OAuth access token, which `api.anthropic.com` rejects as
/// `x-api-key` and accepts as a bearer token.
///
/// Deliberately **not** part of the cache key. It is the same reasoning that
/// keeps `api_key_env` out of [`crate::provider::Provider::fingerprint`]: how
/// you present a credential does not change what the model answers, and keying
/// it would partition a shared cache by how each teammate authenticates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    /// `x-api-key: <credential>`. The `anthropic` default.
    ApiKey,
    /// `Authorization: Bearer <credential>`. The `openai` and `embeddings`
    /// default, and what an OAuth access token needs everywhere.
    Bearer,
    /// Send no credential header at all, and do not require `api_key_env` to
    /// resolve. For endpoints whose scheme is none of the above — supply it
    /// yourself through [`RequestCfg::headers`].
    None,
}

impl AuthMode {
    /// The header this mode sets, and how it formats the credential.
    ///
    /// `None` for [`AuthMode::None`], which sets nothing.
    pub fn header(self, credential: &str) -> Option<(&'static str, String)> {
        match self {
            AuthMode::ApiKey => Some(("x-api-key", credential.to_string())),
            AuthMode::Bearer => Some(("authorization", format!("Bearer {credential}"))),
            AuthMode::None => None,
        }
    }

    /// Whether this mode reads a credential at all.
    pub fn needs_credential(self) -> bool {
        !matches!(self, AuthMode::None)
    }
}

/// Overrides applied to the request a vendor provider would otherwise send.
///
/// Every string field is a minijinja template rendered against `env` (see the
/// module docs). Omitting the whole block leaves a provider byte-identical to
/// what it sent before this type existed — and, just as importantly, leaves its
/// cache entries keyed exactly as they were.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestCfg {
    /// How to present the credential. Defaults to the vendor's own scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthMode>,

    /// Replace the endpoint path this provider appends to `base_url`.
    ///
    /// `base_url` only ever controlled the prefix, so an endpoint that does not
    /// end in `/v1/messages`, `/chat/completions`, or `/embeddings` could not be
    /// reached at all. Must begin with `/`; the loader rejects one that does not,
    /// because a relative path would join `base_url` in a way that reads as a
    /// typo rather than an intent.
    ///
    /// **Keyed.** A different endpoint is a different question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Query parameters appended to the URL.
    ///
    /// Sorted by name on the wire, so two suites writing the same pairs in a
    /// different order produce one cache entry rather than two.
    ///
    /// **Keyed**, for the same reason as `path`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,

    /// Headers added to the request, overriding the vendor's own by name.
    ///
    /// Matching is case-insensitive, as HTTP requires: `Authorization` here
    /// replaces a vendor `authorization`, and the spelling written here is what
    /// goes on the wire.
    ///
    /// **Keyed only as a digest.** A header is where a literal credential sits,
    /// and both the cache key and the stored entry would publish it. See
    /// [`crate::request_cfg::headers_digest`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,

    /// Fields merged into the request body **last**, after the provider has
    /// built it.
    ///
    /// This is the difference between it and `params`, which merges *first* and
    /// is then overwritten by `model`, `messages`, and `system`. Those three are
    /// exactly the fields a gateway sometimes needs changed — a routed model
    /// name, an injected system prompt — and `params` structurally cannot reach
    /// them.
    ///
    /// A deep merge: an object value merges key-by-key, anything else replaces.
    /// Overwriting `messages` is possible and is almost always a mistake; the
    /// provider built them from your prompt for a reason.
    ///
    /// **Keyed**, verbatim, because it is part of the body already keyed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Json>,
}

impl RequestCfg {
    /// Whether this block asks for nothing, and so can be skipped entirely.
    ///
    /// Load-bearing rather than cosmetic: the callers use it to decide whether
    /// to add a member to [`crate::provider::Provider::canonical_request`] at
    /// all, and an empty block must add none — a member that is present-but-empty
    /// hashes differently from an absent one, which would re-key every entry
    /// written before this type existed.
    pub fn is_empty(&self) -> bool {
        self.auth.is_none()
            && self.path.is_none()
            && self.query.is_empty()
            && self.headers.is_empty()
            && self.body.is_none()
    }
}
