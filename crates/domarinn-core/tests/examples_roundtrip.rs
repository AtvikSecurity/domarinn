//! Back-compat guard: the shipped render-health example must keep loading and
//! validating cleanly — with its `!raw` SSTI payload preserved — after the
//! loader split, `${env:VAR}` interpolation, and template-filter work.
//!
//! The example uses no new syntax in its provider config, so env interpolation
//! is a no-op over it; this proves existing suites parse unchanged.

use std::path::Path;

use domarinn_core::config::TestSource;

fn example_dir(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn file_vars_example_loads_validates_and_resolves_fixtures() {
    // Exercises the sandboxed file-var path end to end from the shipped example:
    // `!file`/`{$file}` fixtures are resolved relative to the suite dir.
    let dir = example_dir("07-file-vars");
    let (suite, raw) = domarinn_core::loader::load_file_raw(&dir).unwrap();
    assert!(domarinn_core::validate(&suite, &raw).is_empty());
    let expanded = domarinn_core::expand_tests(&suite, &dir).unwrap();
    assert_eq!(expanded.tests.len(), 4);
    // The raw SSTI fixture must remain raw (never templated) after resolution.
    let ssti = expanded
        .tests
        .iter()
        .find(|t| t.id.as_deref() == Some("adversarial/ssti-fixture"))
        .expect("ssti fixture case present");
    assert!(
        ssti.vars["user_input"].is_raw(),
        "a raw file-var fixture must stay raw"
    );
}

#[test]
fn matrix_example_loads_validates_and_sweeps() {
    let dir = example_dir("08-matrix-sweeps");
    let (suite, raw) = domarinn_core::loader::load_file_raw(&dir).unwrap();
    assert!(domarinn_core::validate(&suite, &raw).is_empty());
    let expanded = domarinn_core::expand_tests(&suite, &dir).unwrap();
    // 2x2 greet sweep + 3-value locale sweep = 7 cells.
    assert_eq!(expanded.tests.len(), 7);
    let ids: Vec<&str> = expanded
        .tests
        .iter()
        .map(|t| t.id.as_deref().unwrap())
        .collect();
    assert!(ids.contains(&"greet[style=terse,temperature=0]"));
    assert!(ids.contains(&"sweep-fr"));
}

#[test]
fn render_health_example_loads_and_validates() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/12-render-health");
    let (suite, raw) = domarinn_core::loader::load_file_raw(&dir).unwrap();
    let issues = domarinn_core::validate(&suite, &raw);
    assert!(
        issues.is_empty(),
        "unexpected validation issues: {issues:?}"
    );

    // The `!raw` SSTI payload must survive as a raw (never-rendered) value — the
    // whole point of the escape hatch, and the thing filters must never touch.
    let ssti = suite
        .tests
        .iter()
        .find_map(|t| match t {
            TestSource::Inline(tc) if tc.id.as_deref() == Some("adversarial/ssti-literal") => {
                Some(tc)
            }
            _ => None,
        })
        .expect("ssti-literal case present");
    assert!(
        ssti.vars["user_input"].is_raw(),
        "the SSTI payload must remain raw"
    );
}
