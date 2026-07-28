//! The crate boundary, enforced.
//!
//! `domarinn-types` exists so the wire contract is separable from the machinery
//! that produces it — which is only true while it stays free of the engine's
//! dependency tree. That property erodes silently: one `reqwest` added for a
//! convenience helper and the server is back to compiling an HTTP client to
//! read a struct definition.

use std::collections::BTreeSet;

/// Crates that would pull the engine (or a runtime, or an HTTP stack) in behind
/// the wire types. Not an exhaustive denylist — a tripwire on the ones a
/// well-meaning change would actually reach for.
const FORBIDDEN: &[&str] = &[
    "domarinn-core",
    "domarinn-cache",
    "domarinn-server",
    "tokio",
    "reqwest",
    "minijinja",
    "async-trait",
    "rusqlite",
    "axum",
    "futures",
];

fn declared_dependencies() -> BTreeSet<String> {
    let manifest = include_str!("../Cargo.toml");
    let mut deps = BTreeSet::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line == "[dependencies]";
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            deps.insert(name.trim().to_string());
        }
    }
    deps
}

#[test]
fn the_wire_types_do_not_depend_on_the_engine() {
    let deps = declared_dependencies();
    assert!(
        !deps.is_empty(),
        "parsed no dependencies — the manifest format changed and this guard is now vacuous"
    );
    for forbidden in FORBIDDEN {
        assert!(
            !deps.contains(*forbidden),
            "domarinn-types must stay free of `{forbidden}`: the point of the crate is that a \
             consumer can depend on the wire contract without building the engine. If this is \
             genuinely needed, the type probably belongs in domarinn-core instead."
        );
    }
}
