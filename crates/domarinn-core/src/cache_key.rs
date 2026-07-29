//! Computing the cache key for a provider call.
//!
//! The key hashes the provider fingerprint, the rendered request, and the repeat
//! index, canonically (see [`crate::cache::canonical_json`]). `repeat` is always
//! present (default 0) so N=1 and repeat-0 of N=3 agree.
//!
//! A case's `cache_salt` joins the hash as `case_salt`, but **only when set**:
//! canonical JSON emits every member that is present, so a null member hashes
//! differently from an absent one. Inserting it conditionally is what keeps
//! every key written before the field existed valid. Any future member must be
//! added the same way, or it invalidates every cache entry in every store at
//! once — see `key_unchanged_when_case_salt_is_none`.
//!
//! Two things are deliberately **not** hashed here:
//!
//! - **The source text of prompt templates.** The *rendered* prompt is already a
//!   member, so hashing the template too would only bust on edits that render
//!   identically for that case — a change with no effect on the provider. It
//!   also cannot help the case the salt exists for, where the system under test
//!   resolves its own prompts across a process boundary and domarinn never sees
//!   them. Use a per-case `cache_salt` for that; keep this a pure hash of what
//!   it is handed, with no filesystem or prompt-source resolution.
//! - **The model a provider *reports* having used.** This one is enforced by
//!   the signature rather than by discipline: [`provider_cache_key`] is handed
//!   a request, and the reported model only exists on a response, so a lookup
//!   could not depend on it even if that were wanted. Nor should it — the
//!   *requested* model is already covered (it is in the `anthropic`/`openai`
//!   fingerprints, and inside `command` for `exec`). Hashing the reported one
//!   would silently discard every cache entry on the day a vendor rolls a
//!   snapshot, which is the opposite of useful; `CaseResult.model` makes that
//!   drift visible and diffable instead, which is the right lever.
//! - **`req.test` (the test id and tags).** Adding it would change every
//!   existing key, and identity is not what makes two calls interchangeable —
//!   the request is. Two cases with identical vars and no prompt therefore share
//!   an entry by design; a per-case `cache_salt` is the supported way to
//!   separate them.

use serde_json::Value as Json;

use crate::cache::CacheKey;
use crate::provider::ProviderRequest;

/// Compute the cache key for one grading call.
///
/// **The "only when set" discipline above does not apply here.** That rule
/// exists to keep keys written before a member existed valid, and this key
/// space is new — it has no legacy entries to preserve. Every member is
/// therefore included unconditionally, which is simpler and is the right
/// default for a fresh key. Do not copy the conditional-insert pattern from
/// `provider_cache_key` into this function; it would be cargo-culting a
/// constraint that does not exist here.
///
/// `kind` discriminates the two key spaces, so a grader key can never collide
/// with a provider key even if their other members coincided.
pub fn grader_cache_key(fingerprint: &Json, graded: &Json, repeat: u32) -> CacheKey {
    CacheKey::compute(&serde_json::json!({
        "kind": "grader-verdict",
        "fingerprint": fingerprint,
        "graded": graded,
        "repeat": repeat,
    }))
}

/// Compute the cache key for one outgoing request — the one rule.
///
/// `sha256(canonical_json({request, repeat, provider_salt?, case_salt?}))`,
/// where `request` is the redacted canonical request the call would send (see
/// [`crate::provider::Provider::canonical_request`]). Every cached call — a
/// provider response, a judge verdict, an embedding, an exec grading — is keyed
/// by this one function, so there is a single place where "what makes two calls
/// interchangeable" is decided.
///
/// The salts are inserted **only when set**, under the discipline the module
/// docs spell out: canonical JSON emits every member that is present, so a
/// `null` member hashes differently from an absent one. An empty string is a
/// real salt value and is deliberately not normalized to "unset".
///
/// Two salt members rather than one merged string, because a provider-level and
/// a case-level salt can both be set on one call: merging them would let
/// `("a", "b")` and `("ab", None)` collide.
///
/// Collision with the legacy [`provider_cache_key`] / [`grader_cache_key`] key
/// spaces is structurally impossible rather than merely unlikely: `request` is
/// not a member of either legacy object, and `fingerprint`/`prompt`/`vars` are
/// not members of this one, so no pair of inputs can produce one canonical
/// string.
pub fn request_cache_key(
    request: &Json,
    repeat: u32,
    provider_salt: Option<&str>,
    case_salt: Option<&str>,
) -> CacheKey {
    let mut parts = serde_json::json!({
        "request": request,
        "repeat": repeat,
    });
    let members = parts.as_object_mut().expect("json! object literal");
    if let Some(salt) = provider_salt {
        members.insert("provider_salt".to_string(), Json::String(salt.to_string()));
    }
    if let Some(salt) = case_salt {
        members.insert("case_salt".to_string(), Json::String(salt.to_string()));
    }
    CacheKey::compute(&parts)
}

/// Compute the cache key for one provider call.
pub fn provider_cache_key(fingerprint: &Json, req: &ProviderRequest, repeat: u32) -> CacheKey {
    let prompt = req
        .prompt
        .as_ref()
        .map(|p| serde_json::to_value(p).unwrap_or(Json::Null))
        .unwrap_or(Json::Null);
    let mut parts = serde_json::json!({
        "fingerprint": fingerprint,
        "prompt": prompt,
        "vars": Json::Object(req.vars.clone().into_iter().collect()),
        "params": Json::Object(req.params.clone()),
        "repeat": repeat,
    });
    // Same "only when present" discipline as `case_salt` below, and for the
    // same reason: every entry written before tools existed must keep its key.
    // An empty list is not a declaration, it is the absence of one.
    if !req.tools.is_empty() {
        parts.as_object_mut().expect("json! object literal").insert(
            "tools".to_string(),
            serde_json::to_value(&req.tools).unwrap_or(Json::Null),
        );
    }
    // Only when set — see the module docs. An empty salt is a real value and is
    // deliberately not normalized to "unset".
    if let Some(salt) = &req.case_salt {
        parts
            .as_object_mut()
            .expect("json! object literal")
            .insert("case_salt".to_string(), Json::String(salt.clone()));
    }
    CacheKey::compute(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::TestMeta;
    use std::collections::BTreeMap;

    fn req(var: &str) -> ProviderRequest {
        salted(var, None)
    }

    fn salted(var: &str, case_salt: Option<&str>) -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert("x".to_string(), Json::String(var.into()));
        ProviderRequest {
            tools: Vec::new(),
            prompt: None,
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: case_salt.map(String::from),
        }
    }

    fn fp() -> Json {
        serde_json::json!({"type": "exec"})
    }

    #[test]
    fn key_is_stable_for_same_inputs() {
        let fp = serde_json::json!({"type": "exec"});
        assert_eq!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("a"), 0)
        );
    }

    #[test]
    fn key_changes_with_vars_and_repeat() {
        let fp = serde_json::json!({"type": "exec"});
        assert_ne!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("b"), 0)
        );
        assert_ne!(
            provider_cache_key(&fp, &req("a"), 0),
            provider_cache_key(&fp, &req("a"), 1)
        );
    }

    /// The load-bearing backward-compatibility test. An unsalted case must hash
    /// exactly like the pre-`case_salt` parts shape — otherwise shipping this
    /// field silently invalidates every cache entry in every disk, S3, and
    /// server-side store at once. Reconstructing the legacy object inline keeps
    /// the assertion honest without a magic constant.
    #[test]
    fn key_unchanged_when_case_salt_is_none() {
        let legacy = CacheKey::compute(&serde_json::json!({
            "fingerprint": fp(),
            "prompt": Json::Null,
            "vars": {"x": "a"},
            "params": {},
            "repeat": 0,
        }));
        assert_eq!(provider_cache_key(&fp(), &req("a"), 0), legacy);
    }

    #[test]
    fn key_changes_when_case_salt_is_set() {
        assert_ne!(
            provider_cache_key(&fp(), &salted("a", None), 0),
            provider_cache_key(&fp(), &salted("a", Some("d1")), 0)
        );
    }

    #[test]
    fn key_changes_between_case_salt_values() {
        assert_ne!(
            provider_cache_key(&fp(), &salted("a", Some("d1")), 0),
            provider_cache_key(&fp(), &salted("a", Some("d2")), 0)
        );
    }

    #[test]
    fn key_is_stable_for_same_case_salt() {
        assert_eq!(
            provider_cache_key(&fp(), &salted("a", Some("d1")), 0),
            provider_cache_key(&fp(), &salted("a", Some("d1")), 0)
        );
    }

    /// An empty salt is a real value, deliberately not normalized to "unset".
    #[test]
    fn empty_case_salt_differs_from_unset() {
        assert_ne!(
            provider_cache_key(&fp(), &salted("a", Some("")), 0),
            provider_cache_key(&fp(), &salted("a", None), 0)
        );
    }

    /// Cases with identical vars and no prompt share a key today, because
    /// `req.test` never enters it — the exact shape of an `exec` suite whose
    /// system under test resolves its own prompt from the test id. The per-case
    /// salt is what tells them apart.
    #[test]
    fn case_salt_separates_cases_that_would_otherwise_collide() {
        let mut a = req("same");
        a.test = TestMeta {
            id: "case-a".into(),
            tags: vec![],
        };
        let mut b = req("same");
        b.test = TestMeta {
            id: "case-b".into(),
            tags: vec![],
        };
        assert_eq!(
            provider_cache_key(&fp(), &a, 0),
            provider_cache_key(&fp(), &b, 0),
            "identical vars collide without a salt"
        );

        a.case_salt = Some("digest-a".into());
        b.case_salt = Some("digest-b".into());
        assert_ne!(
            provider_cache_key(&fp(), &a, 0),
            provider_cache_key(&fp(), &b, 0)
        );
    }

    fn request() -> Json {
        serde_json::json!({"transport": "http", "url": "https://x.test/v1", "body": {"q": 1}})
    }

    #[test]
    fn request_key_is_stable_for_same_inputs() {
        assert_eq!(
            request_cache_key(&request(), 0, None, None),
            request_cache_key(&request(), 0, None, None)
        );
        assert_ne!(
            request_cache_key(&request(), 0, None, None),
            request_cache_key(&serde_json::json!({"transport": "exec"}), 0, None, None)
        );
    }

    #[test]
    fn request_key_separates_repeats() {
        assert_ne!(
            request_cache_key(&request(), 0, None, None),
            request_cache_key(&request(), 1, None, None)
        );
    }

    /// Each salt is inserted only when set, so a `None` hashes exactly like a
    /// call made before that member existed. Reconstructing the object inline
    /// keeps the assertion honest without a magic constant.
    #[test]
    fn an_unset_salt_leaves_the_salt_free_key_alone() {
        let salt_free = CacheKey::compute(&serde_json::json!({
            "request": request(),
            "repeat": 0,
        }));
        assert_eq!(request_cache_key(&request(), 0, None, None), salt_free);

        let provider_only = CacheKey::compute(&serde_json::json!({
            "request": request(),
            "repeat": 0,
            "provider_salt": "p1",
        }));
        assert_eq!(
            request_cache_key(&request(), 0, Some("p1"), None),
            provider_only
        );

        let case_only = CacheKey::compute(&serde_json::json!({
            "request": request(),
            "repeat": 0,
            "case_salt": "c1",
        }));
        assert_eq!(
            request_cache_key(&request(), 0, None, Some("c1")),
            case_only
        );
    }

    /// An empty salt is a real value, deliberately not normalized to "unset" —
    /// for both members.
    #[test]
    fn an_empty_salt_differs_from_an_unset_one() {
        assert_ne!(
            request_cache_key(&request(), 0, Some(""), None),
            request_cache_key(&request(), 0, None, None)
        );
        assert_ne!(
            request_cache_key(&request(), 0, None, Some("")),
            request_cache_key(&request(), 0, None, None)
        );
    }

    /// Two separate members rather than one merged salt: a provider-level and a
    /// case-level salt can both be set on one call, and all four presence
    /// combinations must stay distinct — a merged salt would let
    /// `provider="a", case="b"` collide with `provider="ab"`.
    #[test]
    fn the_two_salts_compose_without_ambiguity() {
        let keys = [
            request_cache_key(&request(), 0, None, None),
            request_cache_key(&request(), 0, Some("a"), None),
            request_cache_key(&request(), 0, None, Some("b")),
            request_cache_key(&request(), 0, Some("a"), Some("b")),
        ];
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                assert_ne!(a, b, "salt presence combinations must stay distinct");
            }
        }
        assert_ne!(
            request_cache_key(&request(), 0, Some("a"), Some("b")),
            request_cache_key(&request(), 0, Some("ab"), None)
        );
    }

    /// The two key spaces cannot collide, and it is structural rather than
    /// probabilistic: `request` is not a member of the legacy object and
    /// `fingerprint`/`prompt`/`vars` are not members of this one, so no pair of
    /// inputs can ever produce one canonical string. Fed the legacy key's own
    /// parts as the request — the most adversarial input available — they still
    /// differ.
    #[test]
    fn a_request_key_can_never_equal_a_legacy_provider_key() {
        let legacy_parts = serde_json::json!({
            "fingerprint": fp(),
            "prompt": Json::Null,
            "vars": {"x": "a"},
            "params": {},
            "repeat": 0,
        });
        assert_ne!(
            request_cache_key(&legacy_parts, 0, None, None),
            provider_cache_key(&fp(), &req("a"), 0)
        );
        assert_ne!(
            request_cache_key(&fp(), 0, None, Some("d1")),
            provider_cache_key(&fp(), &salted("a", Some("d1")), 0)
        );
    }

    /// Tools are part of the *request*: a call offering a tool and one that does
    /// not are different questions, and replaying one for the other would report
    /// "no tools were called" for a call that was never given any.
    #[test]
    fn declared_tools_change_the_key_but_declaring_none_does_not() {
        let with_tools = |names: &[&str]| {
            let mut r = req("a");
            r.tools = names
                .iter()
                .map(|n| crate::config::ToolDef {
                    name: (*n).to_string(),
                    description: None,
                    input_schema: None,
                })
                .collect();
            r
        };
        assert_ne!(
            provider_cache_key(&fp(), &req("a"), 0),
            provider_cache_key(&fp(), &with_tools(&["get_weather"]), 0)
        );
        assert_ne!(
            provider_cache_key(&fp(), &with_tools(&["get_weather"]), 0),
            provider_cache_key(&fp(), &with_tools(&["get_weather", "get_time"]), 0)
        );
        // The load-bearing negative: an empty declaration is the absence of one,
        // so every entry written before tools existed keeps its key.
        assert_eq!(
            provider_cache_key(&fp(), &req("a"), 0),
            provider_cache_key(&fp(), &with_tools(&[]), 0)
        );
    }
}
