//! The error-class rules exist twice, in two languages. This is the seam.
//!
//! # Why this guard exists
//!
//! `ErrorClass::is_infrastructure` decides an amber chip from a red one in the
//! run page, so `web/src/lib/errors.ts` reimplements it — the web bundle cannot
//! call into Rust, and shipping the tally over the wire per class would put a
//! presentation rule in the result schema. The TS copy carried a comment saying
//! it mirrored the Rust predicate, and nothing checked that it still did.
//!
//! Silent drift here is the bad kind: the page keeps rendering, every chip
//! keeps a colour, and the only symptom is that one class is quietly filed
//! under the wrong owner. A reviewer changing the Rust list has no reason to
//! grep the web directory.
//!
//! What is pinned is the *rule*, not the text: the set of prefixes and exact
//! names each side treats as infrastructure. Reformat either file freely.
//!
//! The `unknown` bucket is pinned for a different reason — it is a value both
//! sides *emit*, into a PR comment and a run page describing the same run, so
//! two spellings would be a visible contradiction rather than a wrong colour.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root resolves from crates/domarinn-cli")
}

fn read(relative: &str) -> String {
    let path = repo_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {relative}: {e}"))
}

/// The body of `errorClassTone`, where the mirrored rule lives.
fn tone_fn() -> String {
    let src = read("web/src/lib/errors.ts");
    let start = src
        .find("export function errorClassTone")
        .expect("web/src/lib/errors.ts must still export errorClassTone");
    let rest = &src[start..];
    let end = rest
        .find("\n}")
        .expect("errorClassTone must be a brace-delimited function");
    rest[..end].to_string()
}

/// Every class the Rust predicate calls infrastructure must be amber in the
/// web UI, and every class it does not must not be.
///
/// Asserted through the real predicate rather than a copied list, so this
/// cannot pass by agreeing with a stale transcription of it.
#[test]
fn the_web_tone_rule_matches_the_rust_infrastructure_predicate() {
    use domarinn_core::error_class::ErrorClass;

    let tone = tone_fn();

    // The prefix arms, spelled as the TS writes them.
    for prefix in ["provider_", "cache_"] {
        assert!(
            tone.contains(&format!("startsWith(\"{prefix}\")")),
            "web/src/lib/errors.ts no longer treats `{prefix}*` as infrastructure, \
             but ErrorClass::is_infrastructure still does:\n{tone}"
        );
    }

    // Every named constant, checked against the predicate itself.
    let named = [
        ErrorClass::EXEC_FAILED,
        ErrorClass::RENDER_FAILED,
        ErrorClass::GRADER_FAILED,
        ErrorClass::GRADER_UNAVAILABLE,
        ErrorClass::GRADER_MISSING,
        ErrorClass::ASSERT_FAILED,
    ];
    for class in named {
        let rust_says_infra = ErrorClass::new(class).is_infrastructure();
        let web_says_infra = tone.contains(&format!("=== \"{class}\""));
        assert_eq!(
            rust_says_infra,
            web_says_infra,
            "`{class}`: Rust is_infrastructure() = {rust_says_infra}, but \
             web/src/lib/errors.ts {} it as an exact-match amber case. One side \
             changed without the other.\n{tone}",
            if web_says_infra {
                "does list"
            } else {
                "does not list"
            }
        );
    }
}

/// Every named class is well-formed and reaches a gate bucket.
///
/// Weaker than the tone check above on purpose: which bucket a class belongs in
/// is `error_class.rs`'s own table test. What this adds is that the list here
/// must be extended when a constant is added — the compiler does not enforce
/// exhaustiveness over associated consts, so a new class would otherwise be
/// invisible to every cross-surface guard in this file.
#[test]
fn every_named_class_has_a_gate_bucket_and_a_well_formed_name() {
    use domarinn_core::error_class::{ErrorClass, GateFault};

    // Every constant this build knows, so adding one to `error_class.rs`
    // without considering its gate bucket fails here rather than silently
    // defaulting.
    let all = [
        ErrorClass::PROVIDER_REQUEST,
        ErrorClass::PROVIDER_AUTH,
        ErrorClass::PROVIDER_RATE_LIMIT,
        ErrorClass::PROVIDER_UNAVAILABLE,
        ErrorClass::PROVIDER_TIMEOUT,
        ErrorClass::PROVIDER_PROTOCOL,
        ErrorClass::EXEC_FAILED,
        ErrorClass::RENDER_FAILED,
        ErrorClass::GRADER_FAILED,
        ErrorClass::GRADER_UNAVAILABLE,
        ErrorClass::GRADER_MISSING,
        ErrorClass::ASSERT_FAILED,
        ErrorClass::CACHE_MISS,
        ErrorClass::CACHE_UNAVAILABLE,
    ];
    for class in all {
        let c = ErrorClass::new(class);
        // Not an assertion about which bucket — that is `error_class.rs`'s own
        // table test. This asserts only that the class is spelled the way the
        // constants are, so the two lists cannot drift apart by a typo.
        assert!(
            !class.is_empty() && class.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
            "`{class}` is not a snake_case class name"
        );
        // Exercised so a new class must at least be reachable from here.
        let _: GateFault = c.gate_fault();
    }
}

/// Both surfaces file an unclassifiable error under the same word.
///
/// The CLI writes it into a PR comment's `Errors` row and the web writes it
/// into the run page's breakdown. Two spellings would have the same run
/// described two ways in two places a reader compares directly.
#[test]
fn both_surfaces_bucket_an_unclassified_error_as_unknown() {
    let ts = read("web/src/lib/errors.ts");
    assert!(
        ts.contains(r#"c.error_class ?? "unknown""#),
        "web/src/lib/errors.ts no longer buckets a classless error as `unknown`"
    );

    // The CLI's half, read from source for the same reason as the TS half:
    // `UNKNOWN_CLASS` is `pub(crate)` inside the binary and cannot be imported
    // here. That the CLI *uses* it is covered by
    // `errorstats::tests::an_errored_case_without_a_class_is_bucketed_not_dropped`;
    // what this pins is that the two surfaces agree on the word.
    let rs = read("crates/domarinn-cli/src/errorstats.rs");
    assert!(
        rs.contains(r#"UNKNOWN_CLASS: &str = "unknown""#),
        "the CLI no longer buckets a classless error as `unknown`, so a PR \
         comment and the run page would describe the same run differently"
    );
}
