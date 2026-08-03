//! Adopting cache entries written under a previous key shape.
//!
//! # Why this exists
//!
//! Through 0.4.x a provider cache key was a SHA-256 over the provider
//! *fingerprint* plus the pieces of the request — the seven-part composite
//! [`legacy_provider_key`] still computes. 0.5.0 replaced it with one rule for
//! every cached call: a hash of the canonical outgoing request
//! ([`crate::cache_key::request_cache_key`]). Every key in every store moved,
//! and a store full of perfectly good responses became unreachable — not wrong,
//! just invisible. For an LLM-graded suite that is paid for in real money, so a
//! key change should cost one migration rather than one re-run of everything
//! anybody has ever cached.
//!
//! An entry cannot be re-keyed offline. A ≤0.4.x entry records the fingerprint
//! that produced it but not the *request*, and the key hashes both, so there is
//! nothing to recompute from and no inverting SHA-256. What can be done is to
//! migrate at the only moment both halves are in hand: a lookup. On a miss,
//! [`runner_cache`] re-derives the old key from each fingerprint this provider
//! would once have published, and the first one that hits is adopted — returned
//! as a hit and rewritten under the current key (carrying its canonical request
//! this time), so the next run finds it directly.
//!
//! [`runner_cache`]: crate::runner::runner_cache
//!
//! # The shapes this machinery knows
//!
//! Two key spaces, both retired by 0.5.0's one rule.
//!
//! **Provider responses** — [`legacy_provider_key`] over one of these
//! fingerprints:
//!
//! | Provider | Shapes | Shipped |
//! |---|---|---|
//! | `openai` | `{type, model, base_url, params}` | ≤0.4.0 |
//! | `anthropic` | `{type, model, base_url, params}` | ≤0.4.0 |
//! | `http` | `{type, url, method, body, output_expr, headers?}` | ≤0.4.0 |
//! | `exec` | `{type, command, env, cache_salt}`, then four older generations | ≤0.4.0, then 0.3.1 and earlier (see [`legacy_exec_fingerprints`]) |
//!
//! **Grader verdicts** — [`legacy_grader_verdict_key`] over a grading
//! fingerprint and the graded document, both frozen below:
//!
//! | Assert | Shapes | Shipped |
//! |---|---|---|
//! | `llm-rubric` | [`legacy_grading_fingerprint`] × [`legacy_graded_payload`] | ≤0.4.x |
//! | `exec` | [`legacy_grading_fingerprint`] × [`legacy_graded_payload`] | ≤0.4.x |
//! | `similar` | **deliberately not adopted** — see [`legacy_graded_payload`] | — |
//!
//! Both spaces are deleted on the same timeline (below), and both are probed
//! out of one [`crate::cache_adopt::MigrationProbe`] budget: a store either has ≤0.4.x entries in it
//! or it does not, and which half found the first one says nothing useful.
//!
//! The ≤0.4.0 shape of each is, by construction, whatever
//! [`crate::provider::Provider::fingerprint`] returns today: that method is
//! frozen for exactly this reason (it also feeds `digests::provider_digest`,
//! which `--against` diffing compares), so
//! [`crate::provider::Provider::legacy_fingerprints`] leads with it rather than
//! duplicating four literals that could drift apart from the pin tests guarding
//! them. The older `exec` generations have no such source and are literals here.
//! `golden_*_key` below pins the exact key each shape produces, so a refactor
//! that moves one fails loudly rather than quietly stranding a store.
//!
//! # Cost, and why it is bounded
//!
//! Probing costs one extra `get` per historical shape per miss, which against a
//! remote backend is latency nobody asked for on a cache that has nothing to
//! migrate. [`crate::cache_adopt::MigrationProbe`] therefore spends a small budget of cases and
//! stops if none of them adopt anything: an empty or already-migrated store pays
//! a few dozen lookups once, and a store worth migrating keeps probing for as
//! long as it keeps paying off.
//!
//! # Deleting this
//!
//! This module is disposable by construction: it ships in 0.5.0, stays through
//! 0.6.x, and is deleted in 0.7.0 — by which point a store still holding only
//! ≤0.4.x entries has gone two minor releases without a run. Nothing else may
//! depend on it: everything here is a frozen historical literal, and editing one
//! to match a current shape would defeat the point.

use std::path::Path;

use serde_json::Value as Json;

use crate::cache::CacheKey;
use crate::config::{Assert, AssertKind, Grader, ProviderKind};
use crate::provider::ProviderRequest;
use crate::types::Output;

/// The provider cache key as domarinn computed it through 0.4.x.
///
/// **Frozen ≤0.4.x shape — do not edit.** Every byte of this function is what a
/// store in the wild was keyed with; changing it does not migrate anything, it
/// just makes the probe miss. It was `cache_key::provider_cache_key` until 0.5.0
/// moved the live path onto [`crate::cache_key::request_cache_key`], and it is
/// preserved verbatim apart from the rename.
///
/// Seven members at most: `fingerprint`, `prompt`, `vars`, `params`, `repeat`
/// unconditionally, plus `tools` and `case_salt` only when set. That conditional
/// discipline is load-bearing history — canonical JSON emits every member that
/// is present, so a null member hashes differently from an absent one, and an
/// entry written before `tools` or `case_salt` existed keeps its key only
/// because they are inserted rather than defaulted.
pub fn legacy_provider_key(fingerprint: &Json, req: &ProviderRequest, repeat: u32) -> CacheKey {
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
    // Only when set. An empty salt is a real value and is deliberately not
    // normalized to "unset".
    if let Some(salt) = &req.case_salt {
        parts
            .as_object_mut()
            .expect("json! object literal")
            .insert("case_salt".to_string(), Json::String(salt.clone()));
    }
    CacheKey::compute(&parts)
}

/// The grader-verdict cache key as domarinn computed it through 0.4.x.
///
/// **Frozen ≤0.4.x shape — do not edit.** It was `cache_key::grader_cache_key`
/// until 0.5.0 put every grader-originated call — the judge's HTTP request, an
/// embedding, an `exec` assert's protocol exchange — on
/// [`crate::cache_key::request_cache_key`] like any other request. Preserved
/// verbatim apart from the rename, because every byte of it is what a store in
/// the wild was keyed with.
///
/// Adopting one of these is not the same manoeuvre as adopting a provider
/// entry. A ≤0.4.x verdict entry holds a *verdict* and no `raw` payload, so
/// there is nothing to re-parse; it is served as the verdict it is and re-filed
/// under the request key unchanged, and the read contract in
/// [`crate::request_cache`] keeps it servable from then on.
pub fn legacy_grader_verdict_key(fingerprint: &Json, graded: &Json, repeat: u32) -> CacheKey {
    CacheKey::compute(&serde_json::json!({
        "kind": "grader-verdict",
        "fingerprint": fingerprint,
        "graded": graded,
        "repeat": repeat,
    }))
}

/// The half of a ≤0.4.x verdict key that described the *judge*.
///
/// **Frozen ≤0.4.x shape — do not edit.** Copied from `grader.rs`'s
/// `grading_fingerprint` before it was deleted, minus the arms whose verdicts
/// are not adopted. Everything it enumerated by hand — the judge's model,
/// endpoint and merged params, the system prompt, the `grader.template` file's
/// bytes — now falls out of the judge's request body instead.
///
/// `system_prompt` and `verdict_mode`'s default are parameters/current types
/// rather than frozen literals, deliberately, and it is the same argument
/// [`legacy_provider_key`] makes for taking a live
/// [`crate::provider::Provider::fingerprint`]: if the built-in grading prompt is
/// ever edited, the *live* key moves (the prompt is in the body) and this one
/// must move with it — otherwise the probe would adopt a verdict produced by a
/// prompt the run no longer uses. Freezing the old text here would preserve a
/// key nobody should still hit.
///
/// `on_disk`-style filesystem work is done inline: a `grader.template` is read
/// and digested per call. The memo `grader.rs` kept is not reproduced, because
/// this runs on the miss path of at most [`PROBE_BUDGET`] cases rather than on
/// every assertion of every cell.
pub fn legacy_grading_fingerprint(
    assert: &Assert,
    default_grader: Option<&Grader>,
    system_prompt: &str,
    base_dir: Option<&Path>,
) -> Option<Json> {
    fn provider_identity(kind: &ProviderKind) -> Option<Json> {
        match kind {
            ProviderKind::Anthropic {
                model,
                base_url,
                params,
                ..
            } => Some(
                serde_json::json!({"type": "anthropic", "model": model, "base_url": base_url, "params": params}),
            ),
            ProviderKind::Openai {
                model,
                base_url,
                params,
                ..
            } => Some(
                serde_json::json!({"type": "openai", "model": model, "base_url": base_url, "params": params}),
            ),
            ProviderKind::Embeddings {
                model,
                base_url,
                params,
                ..
            } => Some(
                serde_json::json!({"type": "embeddings", "model": model, "base_url": base_url, "params": params}),
            ),
            _ => None,
        }
    }

    let system_digest = format!("{}", blake3::hash(system_prompt.as_bytes()).to_hex());

    match &assert.kind {
        AssertKind::LlmRubric { grader, params, .. } => {
            let g = grader.as_deref().or(default_grader)?;
            Some(serde_json::json!({
                "assert": "llm-rubric",
                "provider": provider_identity(&g.provider)?,
                "template": g.template,
                "template_digest": legacy_template_digest(g.template.as_deref(), base_dir),
                "verdict_mode": g.verdict_mode.unwrap_or_default(),
                "assert_params": params,
                "system_prompt": system_digest,
            }))
        }
        AssertKind::Exec {
            command,
            cache_salt,
            config: _,
        } => Some(serde_json::json!({
            "assert": "exec",
            "command": command,
            "cache_salt": cache_salt,
        })),
        // Everything else either had no fingerprint (a local assert never
        // reached the grader) or is not adopted — see `legacy_graded_payload`.
        _ => None,
    }
}

/// A digest of a `grader.template`'s bytes, as ≤0.4.x computed it.
///
/// **Frozen ≤0.4.x shape — do not edit.** `Json::Null` both when there is no
/// template and when the file cannot be read: the old code returned null for
/// the unreadable case and never called this for the absent one, and the
/// enclosing object stored null in both.
fn legacy_template_digest(spec: Option<&str>, base_dir: Option<&Path>) -> Json {
    let Some(spec) = spec else {
        return Json::Null;
    };
    let Some(rel) = spec.strip_prefix("file://") else {
        return Json::Null;
    };
    let path = match base_dir {
        Some(dir) => match crate::sandbox::resolve_within(dir, rel) {
            Ok(p) => p,
            Err(_) => return Json::Null,
        },
        None => std::path::PathBuf::from(rel),
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            serde_json::json!(format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex()))
        }
        Err(_) => Json::Null,
    }
}

/// Everything a ≤0.4.x verdict key needed about the *question*.
///
/// A struct because the members are used by different arms and travel together:
/// `rubric` is the rendered rubric an `llm-rubric` verdict answered, and the
/// rest are what an `exec` child was told about the cell.
pub struct LegacyGraded<'a> {
    /// The output that was graded.
    pub output: &'a Output,
    /// The **rendered** rubric — never the template. ≤0.4.x hashed the rendered
    /// text, which is why two cases of one matrix did not share a verdict.
    pub rubric: &'a str,
    pub vars: &'a Json,
    pub test_id: &'a str,
    pub test_tags: &'a [String],
    pub provider_id: &'a str,
}

/// The half of a ≤0.4.x verdict key that described what was *graded*.
///
/// **Frozen ≤0.4.x shape — do not edit.** Copied from `runner_asserts.rs`'s
/// `graded_payload` before it was deleted.
///
/// `similar` is deliberately absent, and its absence is a decision rather than
/// an omission. A ≤0.4.x `similar` entry holds one cosine value, which
/// decomposes into neither of the two embedding vectors 0.5.0 caches — there is
/// nothing to adopt it *into*. Re-embedding two short strings costs a fraction
/// of a cent once, against a migration path that would have to invent a second
/// entry shape to hold an answer no future lookup can ask for. The `similar`
/// path pays the embedder once and is warm from then on.
///
/// The one case where "costs a fraction of a cent once" is the wrong summary is
/// offline: under `--cache-only` there is no re-embedding to fall back on, so a
/// store warmed by 0.4.x hard-errors on every `similar` assertion until the
/// suite has been run once in `ReadWrite` to lay the vectors down. That is a
/// one-time upgrade step for one assert kind rather than a silent wrong answer,
/// and it is the honest behaviour for a run that promised not to make network
/// calls — but it is the thing to say in the release notes.
///
/// One wart is preserved here on purpose, because a frozen shape has to be
/// frozen wart and all: the `vars` an `exec` verdict hashed is the *render
/// context*, which carries a snapshot of the whole process environment. A
/// ≤0.4.x exec verdict is therefore only adoptable on a machine whose
/// environment still matches the one that wrote it — the in-place-upgrade case,
/// which is the same one the `mtime` program flavours above serve, and the only
/// one that was ever reachable for these entries. The live key deliberately
/// drops `env` (see `grader.rs`'s `exec_assert_canonical`), so the entry
/// adoption re-files is portable even though the key it came from was not.
pub fn legacy_graded_payload(assert: &Assert, graded: &LegacyGraded<'_>) -> Option<Json> {
    let output = graded
        .output
        .as_json()
        .unwrap_or_else(|| Json::String(graded.output.as_text().into_owned()));
    match &assert.kind {
        AssertKind::LlmRubric { .. } => Some(
            serde_json::json!({"assert": "llm-rubric", "rubric": graded.rubric, "output": output}),
        ),
        AssertKind::Exec { config, .. } => Some(serde_json::json!({
            "assert": "exec",
            "config": config,
            "output": output,
            "vars": graded.vars,
            "test": {"id": graded.test_id, "tags": graded.test_tags},
            "provider": {"id": graded.provider_id},
        })),
        _ => None,
    }
}

/// The `exec` fingerprints domarinn published *before* 0.4.0, newest first.
///
/// The ≤0.4.0 shape is not here: it is the provider's own current
/// [`crate::provider::Provider::fingerprint`], which
/// [`crate::provider::Provider::legacy_fingerprints`] puts in front of this
/// list. These four have no such source and are frozen literals.
///
/// Ordered newest-first so the most likely match is found in the fewest
/// lookups. Each entry is a frozen historical literal — see the module docs; do
/// not "fix" one to track the current fingerprint.
///
/// | Shape | Shipped in | Differs by |
/// |---|---|---|
/// | `{type, command, program: [{path, content}], env, cache_salt}` | unreleased | program keyed on file contents |
/// | `{type, command, program: [{path, len, mtime, mtime_ns}], env, cache_salt}` | 0.3.1 | program keyed on stat metadata |
/// | `{type, command, program: […], cache_salt}` | 0.3.0 | before `env` joined the key |
/// | `{type, command, cache_salt}` | 0.2.x | before `program` existed at all |
///
/// The last two are emitted **only when `env_digest` is `None`**. They have no
/// `env` member to key on, so for a provider that declares one they collide
/// across every declared value — see the comment at the return below.
///
/// The two `program` flavours are both produced from one filesystem walk. The
/// `mtime` ones only ever match on the machine that wrote them, with the files
/// untouched since — which is exactly the case that matters, a developer or a
/// CI runner upgrading in place.
pub fn legacy_exec_fingerprints(
    command: &[String],
    env_digest: Option<&str>,
    cache_salt: Option<&str>,
    base_dir: Option<&Path>,
) -> Vec<Json> {
    let (by_content, by_stat) = legacy_programs(command, base_dir);
    let mut out = Vec::with_capacity(4);
    for program in [&by_content, &by_stat] {
        out.push(serde_json::json!({
            "type": "exec",
            "command": command,
            "program": program,
            "env": env_digest,
            "cache_salt": cache_salt,
        }));
    }
    // The two shapes below predate `env` joining the key, so they have nowhere
    // to put the digest and an entry filed under either was written under an
    // environment nobody recorded. Offering them to a provider that *declares*
    // `env` is not a stale-replay risk, it is a guaranteed collision: every
    // declared value recomputes the same probe, so changing the variable that
    // selects the backend adopts the old backend's answers — and on a shared
    // tier writes new ones under keys indistinguishable from real ones. A
    // provider declaring no `env` is the case these shapes exist for, and it
    // keeps them.
    if env_digest.is_some() {
        return out;
    }
    out.push(serde_json::json!({
        "type": "exec",
        "command": command,
        "program": by_stat,
        "cache_salt": cache_salt,
    }));
    out.push(serde_json::json!({
        "type": "exec",
        "command": command,
        "cache_salt": cache_salt,
    }));
    out
}

/// The `program` array as both historical flavours computed it, in one walk.
///
/// Returns `(contents, stat)`. Both are `[]` when nothing resolves, which is
/// what the old code produced for `docker run …` — and back then that meant
/// "not cacheable", so such a provider has no legacy entries to find anyway.
fn legacy_programs(command: &[String], base_dir: Option<&Path>) -> (Json, Json) {
    // Matches the historical cap. Above it the old code fell back to stat
    // metadata for *both* flavours, which is why the fallback is shared here.
    const MAX_HASHED_BYTES: u64 = 256 * 1024 * 1024;

    let (mut contents, mut stat) = (Vec::new(), Vec::new());
    for (i, arg) in command.iter().enumerate() {
        let Some(resolved) = crate::exec::resolve_program_arg(arg, i == 0, base_dir) else {
            continue;
        };
        let Ok(meta) = std::fs::metadata(&resolved) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok());
        let by_stat = serde_json::json!({
            "path": arg,
            "len": meta.len(),
            "mtime": mtime.map(|d| d.as_secs()),
            "mtime_ns": mtime.map(|d| d.subsec_nanos()),
        });
        let hashed = (meta.len() <= MAX_HASHED_BYTES)
            .then(|| file_digest(&resolved))
            .flatten();
        contents.push(match hashed {
            Some(content) => serde_json::json!({ "path": arg, "content": content }),
            None => by_stat.clone(),
        });
        stat.push(by_stat);
    }
    (Json::Array(contents), Json::Array(stat))
}

/// blake3 of a file's contents, in the `blake3:<hex>` spelling the historical
/// `program` array used.
fn file_digest(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("blake3:{}", hasher.finalize().to_hex()))
}

#[cfg(test)]
#[path = "cache_migrate_tests.rs"]
mod tests;
