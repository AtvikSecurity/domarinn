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

/// Every quoted argument of `pattern("` in `src`, in order.
///
/// Textual on purpose — the web bundle cannot be executed from here — but
/// *extractive* rather than probe-based: a probe (`contains`) can only prove a
/// listed arm exists, so an arm someone *adds* is invisible to it. Extracting
/// everything and comparing sets fails in both directions.
fn quoted_args(src: &str, pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find(pattern) {
        rest = &rest[at + pattern.len()..];
        if let Some(end) = rest.find('"') {
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    out
}

/// Every class constant `error_class.rs` declares, read from its source.
///
/// Derived rather than hand-listed because a hand list checked against itself
/// is how the last version of this guard could not see a newly added class at
/// all: the compiler does not enforce exhaustiveness over associated consts,
/// so the only place the full set exists is the declaration site.
fn declared_classes() -> Vec<String> {
    let src = read("crates/domarinn-types/src/error_class.rs");
    let names = quoted_args(&src, ": &'static str = \"");
    assert!(
        names.len() >= 13,
        "expected the error-class constants in error_class.rs, found {names:?} — \
         did the declaration shape change?"
    );
    names
}

/// The web tone rule and the Rust infrastructure predicate agree — in both
/// directions, over every class the Rust side declares.
///
/// The TS arms are *extracted* (every `startsWith(...)` and `=== "..."` in the
/// function) and compared as sets against what `is_infrastructure` actually
/// returns, so this fails when either side adds, removes, or widens an arm —
/// including a new `startsWith` prefix, which a `contains` probe of the known
/// arms would never see.
#[test]
fn the_web_tone_rule_matches_the_rust_infrastructure_predicate() {
    use domarinn_core::error_class::ErrorClass;

    let tone = tone_fn();
    let prefixes = quoted_args(&tone, "startsWith(\"");
    let exact: Vec<String> = quoted_args(&tone, "=== \"");

    // Direction 1: every TS arm must be justified by the Rust predicate.
    for prefix in &prefixes {
        assert!(
            ErrorClass::new(format!("{prefix}x")).is_infrastructure(),
            "web/src/lib/errors.ts treats `{prefix}*` as infrastructure, but \
             ErrorClass::is_infrastructure does not:\n{tone}"
        );
    }
    for name in &exact {
        assert!(
            ErrorClass::new(name.clone()).is_infrastructure(),
            "web/src/lib/errors.ts lists `{name}` as an exact-match amber case, \
             but ErrorClass::is_infrastructure says it is not infrastructure:\n{tone}"
        );
    }

    // Direction 2: every class Rust calls infrastructure must be reachable by
    // some TS arm, and every class it does not must be reachable by none.
    for class in declared_classes() {
        let rust_says = ErrorClass::new(class.clone()).is_infrastructure();
        let web_says =
            prefixes.iter().any(|p| class.starts_with(p.as_str())) || exact.contains(&class);
        assert_eq!(
            rust_says,
            web_says,
            "`{class}`: Rust is_infrastructure() = {rust_says}, but the web \
             tone rule renders it {} — one side changed without the other.\n{tone}",
            if web_says { "amber" } else { "red" }
        );
    }
}

/// Every declared class is snake_case — derived from source, so a class added
/// tomorrow is covered without anyone remembering this file exists.
#[test]
fn every_declared_class_is_well_formed() {
    for class in declared_classes() {
        assert!(
            !class.is_empty() && class.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'),
            "`{class}` is not a snake_case class name"
        );
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
        ts.contains(r#"c.error_class || "unknown""#),
        "web/src/lib/errors.ts no longer buckets a classless error as `unknown`"
    );
    // `??` passes a present-but-empty class straight through to a nameless
    // chip, while the Rust side buckets it. Both spellings satisfy a naive
    // "does it mention unknown" grep, which is why this asserts the operator.
    assert!(
        !ts.contains(r#"c.error_class ?? "unknown""#),
        "`??` lets an empty-string class through as its own bucket; the CLI \
         filters it, so the two surfaces would disagree"
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
