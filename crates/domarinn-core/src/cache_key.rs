//! Computing the cache key for a provider call.
//!
//! Two key spaces live here, and every key in both is a canonical hash (see
//! [`crate::cache::canonical_json`]) of an object whose members are named below.
//! [`request_cache_key`] hashes the canonical outgoing request itself, under one
//! rule for every kind of cached call — it is what the provider path keys on.
//! [`grader_cache_key`] still hashes a grader fingerprint alongside the graded
//! document. Their member names are disjoint, so no input can produce a key in
//! both. `repeat` is always present in both (default 0) so N=1 and repeat-0 of
//! N=3 agree.
//!
//! The key space these replaced — a provider fingerprint hashed alongside the
//! pieces of the request — is frozen in [`crate::cache_migrate`], which reads it
//! to adopt entries written by 0.4.x and earlier. It is history now; nothing
//! here may grow a dependency on it.
//!
//! A salt joins the hash **only when set**: canonical JSON emits every member
//! that is present, so a null member hashes differently from an absent one.
//! Inserting it conditionally is what keeps every key written before the member
//! existed valid. Any future member must be added the same way, or it
//! invalidates every cache entry in every store at once — see
//! `an_unset_salt_leaves_the_salt_free_key_alone`.
//!
//! Two things are deliberately **not** hashed here:
//!
//! - **The source text of prompt templates.** The *rendered* prompt is already
//!   inside the canonical request, so hashing the template too would only bust
//!   on edits that render identically for that case — a change with no effect on
//!   the provider. It also cannot help the case the salt exists for, where the
//!   system under test resolves its own prompts across a process boundary and
//!   domarinn never sees them. Use a per-case `cache_salt` for that; keep this a
//!   pure hash of what it is handed, with no filesystem or prompt-source
//!   resolution.
//! - **The model a provider *reports* having used.** This one is enforced by
//!   the signature rather than by discipline: [`request_cache_key`] is handed
//!   the outgoing request, and the reported model only exists on a response, so
//!   a lookup could not depend on it even if that were wanted. Nor should it —
//!   the *requested* model is already in the request body. Hashing the reported
//!   one would silently discard every cache entry on the day a vendor rolls a
//!   snapshot, which is the opposite of useful; `CaseResult.model` makes that
//!   drift visible and diffable instead, which is the right lever.
//! - **`req.test` (the test id and tags).** Identity is not what makes two calls
//!   interchangeable — the request is. Two cases with identical vars and no
//!   prompt therefore share an entry by design; a per-case `cache_salt` is the
//!   supported way to separate them. (`exec` sends `test` to its child and
//!   [`crate::provider::Provider::canonical_request`] strips it back out for
//!   exactly this reason.)

use serde_json::Value as Json;

use crate::cache::CacheKey;

/// Compute the cache key for one grading call.
///
/// **The "only when set" discipline above does not apply here.** That rule
/// exists to keep keys written before a member existed valid, and this key
/// space is new — it has no legacy entries to preserve. Every member is
/// therefore included unconditionally, which is simpler and is the right
/// default for a fresh key. Do not copy the conditional-insert pattern from
/// [`crate::cache_migrate::legacy_provider_key`] into this function; it would be
/// cargo-culting a constraint that does not exist here.
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
/// Collision with the [`crate::cache_migrate::legacy_provider_key`] /
/// [`grader_cache_key`] key spaces is structurally impossible rather than merely
/// unlikely: `request` is not a member of either of those objects, and
/// `fingerprint`/`prompt`/`vars` are not members of this one, so no pair of
/// inputs can produce one canonical string.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Provider, ProviderRequest, TestMeta};
    use crate::types::RenderedPrompt;
    use std::collections::BTreeMap;

    fn request() -> Json {
        serde_json::json!({"transport": "http", "url": "https://x.test/v1", "body": {"q": 1}})
    }

    // ── Golden literals for the live key space ───────────────────────────────
    //
    // Every other test on this path is *relative*: canonical-equals-preview, key
    // inequality, share-and-bust pairs. All of them keep passing if the whole
    // shape moves together — a renamed `transport`, a member added to an
    // envelope, a provider that starts folding one more thing into its body.
    // That failure is silent and total: every 0.5.0 key in every store moves and
    // no test says so. These are the magic constants that say so.

    /// The call every golden below is keyed on. Fixed and boring on purpose.
    fn keyed_call() -> ProviderRequest {
        let mut vars = BTreeMap::new();
        vars.insert("x".to_string(), Json::String("a".into()));
        ProviderRequest {
            tools: Vec::new(),
            prompt: Some(RenderedPrompt::Text("hi".into())),
            vars,
            params: serde_json::Map::new(),
            test: TestMeta::default(),
            case_salt: None,
        }
    }

    fn openai() -> crate::openai::OpenAiProvider {
        crate::openai::OpenAiProvider::new("p", "gpt-x", None, None, None, None)
    }
    fn anthropic() -> crate::anthropic::AnthropicProvider {
        crate::anthropic::AnthropicProvider::new("p", "claude-x", None, None, None, None)
    }
    fn http() -> crate::http_provider::HttpProvider {
        crate::http_provider::HttpProvider::new(
            "p",
            "https://sut.test/generate",
            None,
            BTreeMap::new(),
            Some(serde_json::json!({"prompt": "{{ x }}"})),
            None,
        )
    }
    fn exec(cache_salt: Option<&str>) -> crate::exec_provider::ExecProvider {
        crate::exec_provider::ExecProvider::new(
            "p",
            vec!["./sut".to_string()],
            Default::default(),
            None,
            cache_salt.map(String::from),
            None,
        )
    }

    /// The key each provider kind produces for one fixed call.
    ///
    /// If one of these moves, every entry that provider kind ever wrote under
    /// 0.5.x is unreachable. That is a migration to plan — a new legacy shape in
    /// `cache_migrate.rs` — not a constant to update.
    #[test]
    fn golden_live_key_per_provider_kind() {
        let (openai, anthropic, http, exec) = (openai(), anthropic(), http(), exec(None));
        for (kind, provider, expected) in [
            (
                "openai",
                &openai as &dyn Provider,
                "sha256:15324316a2b3aa5a9f959a829d60b964eccfc26d5a14ce599d80e31007047b66",
            ),
            (
                "anthropic",
                &anthropic,
                "sha256:966226683d9a9c8564d025eae2f6e3ee9b1b20192ddd849eed436d1051178e39",
            ),
            (
                "http",
                &http,
                "sha256:894c2d308a88738d6aed73e74b4ab6262419f9b7f50bfb42f94d3628a90cf5c3",
            ),
            (
                "exec",
                &exec,
                "sha256:92dcefcc2ae3856d1b66a54c4ef4aeefb183332a120ed4ac0a8c5f68d0dd8b00",
            ),
        ] {
            let canonical = provider
                .canonical_request(&keyed_call())
                .unwrap_or_else(|| panic!("{kind} must be cacheable"));
            let key = request_cache_key(&canonical, 0, provider.cache_salt(), None);
            assert_eq!(key.0, expected, "{kind}: the live key moved");
        }
    }

    /// …and the same with both salt members present, so the *composed* shape is
    /// pinned rather than only the salt-free one. A salt that started being
    /// merged, renamed, or inserted in a different order would pass every
    /// relative test in this file and fail here.
    #[test]
    fn golden_live_key_with_both_salts() {
        let exec = exec(Some("v1"));
        let mut call = keyed_call();
        call.case_salt = Some("d1".into());
        let canonical = exec.canonical_request(&call).expect("exec is cacheable");
        assert_eq!(
            request_cache_key(&canonical, 0, exec.cache_salt(), call.case_salt.as_deref()).0,
            "sha256:2338250032821b0d218a23b3d37b526883c459457feb9996b680f449cae6455e"
        );
    }

    /// The exec protocol version is inside the key, and this is where you find
    /// that out.
    ///
    /// `canonical_request` keeps the `domarinn` envelope deliberately — it is
    /// sent, and a child may answer a v2 request differently — which makes
    /// `PROTOCOL_VERSION` a key member. Without this assertion the bump would be
    /// made by someone reasoning about the wire format, with no reason to think
    /// about caches, and every warm exec store in the world would go quiet at
    /// once.
    #[test]
    fn bumping_the_exec_protocol_version_re_keys_every_exec_entry() {
        let canonical = exec(None)
            .canonical_request(&keyed_call())
            .expect("exec is cacheable");
        assert_eq!(
            canonical["stdin"]["domarinn"]["protocol"],
            serde_json::json!(1),
            "bumping PROTOCOL_VERSION re-keys every exec cache entry in every store; \
             freeze the protocol-1 shape as a legacy generation in cache_migrate.rs \
             before shipping"
        );
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

    /// Grader keys and request keys cannot collide, and it is structural rather
    /// than probabilistic: `request` is not a member of a grader key's object and
    /// `kind`/`fingerprint`/`graded` are not members of this one, so no pair of
    /// inputs can ever produce one canonical string. Fed the other key's own
    /// parts as the request — the most adversarial input available — they still
    /// differ. The same guard against the *provider* key space domarinn used to
    /// publish lives in `cache_migrate.rs`, next to the frozen function.
    #[test]
    fn a_request_key_can_never_equal_a_grader_key() {
        let fp = serde_json::json!({"type": "anthropic"});
        let graded = serde_json::json!({"output": "hi"});
        let grader_parts = serde_json::json!({
            "kind": "grader-verdict",
            "fingerprint": fp,
            "graded": graded,
            "repeat": 0,
        });
        assert_ne!(
            request_cache_key(&grader_parts, 0, None, None),
            grader_cache_key(&fp, &graded, 0)
        );
    }
}
