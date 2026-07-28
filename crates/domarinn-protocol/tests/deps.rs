//! The dependency list is this crate's product, so it is a test.
//!
//! `domarinn-protocol` exists so someone writing a provider in Rust can speak
//! the exec protocol without inheriting the engine's tree — or the schema
//! generator and TypeScript exporter that `domarinn-types` carries. That
//! promise is only worth making if something enforces it, because the cheapest
//! way to solve any given problem here will always be "add a crate".
//!
//! If you are here because this test failed: the fix is almost never to widen
//! `ALLOWED`. Serialize the thing by hand, or put it in `domarinn-types` where
//! the dependency budget is already spent.
//!
//! Parsed by hand rather than with a TOML crate, because reaching for a
//! dev-dependency to assert "this crate has two dependencies" is the joke
//! telling itself.

/// Dev-dependencies are deliberately not covered: they do not propagate to
/// consumers, so they cost a provider author nothing.
const ALLOWED: &[&str] = &["serde", "serde_json"];

#[test]
fn the_protocol_crate_depends_only_on_serde() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("reading our own manifest");

    let mut in_deps = false;
    let mut found: Vec<String> = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Exactly `[dependencies]` — not `[dev-dependencies]`, and not a
            // `[target.'cfg(...)'.dependencies]` table, which would smuggle a
            // platform-gated dependency past a looser check.
            in_deps = line == "[dependencies]";
            continue;
        }
        if !in_deps || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            found.push(name.trim().trim_matches('"').to_string());
        }
    }

    // Both directions, so a parser that silently stopped matching lines fails
    // here instead of passing forever with an empty `found`. A deliberate
    // removal updates ALLOWED; a broken parser does not.
    for expected in ALLOWED {
        assert!(
            found.iter().any(|d| d == expected),
            "did not parse `{expected}` out of the manifest — the layout \
             changed and this test is measuring nothing. Fix the parser before \
             trusting a green run here."
        );
    }

    let unexpected: Vec<&String> = found
        .iter()
        .filter(|d| !ALLOWED.contains(&d.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "domarinn-protocol grew {unexpected:?}. Read this file's module doc: \
         the small dependency list is the crate's reason to exist, and every \
         addition is paid for by everyone who writes a provider."
    );
}
