//! Matrix / parameter sweeps: expand one case into a cross-product of cases.
//!
//! A case with a `matrix` fans out over the cartesian product of its axes, one
//! concrete case per combination:
//!
//! ```yaml
//! - id: greet
//!   matrix:
//!     style: [terse, warm]
//!     temperature: [0, 1]
//!   vars: { name: "Ada" }
//! ```
//!
//! yields four cases — `greet[style=terse,temperature=0]`,
//! `greet[style=terse,temperature=1]`, `greet[style=warm,temperature=0]`,
//! `greet[style=warm,temperature=1]`. Each axis value is merged into `vars`
//! (the axis wins over a base var of the same name, for that key only), so the
//! prompt sees `style`/`temperature` alongside `name`.
//!
//! Ids are deterministic: axes iterate in sorted key order and the default id
//! encodes every `key=value` pair, so the same suite produces the same ids —
//! and therefore stable [`CaseKey`](crate::ids::CaseKey)s — across runs. A
//! `matrix_id` template (`"{{ style }}-{{ temperature }}"`) overrides the id
//! shape when you want something friendlier; it is rendered against the axis
//! values of each combination.
//!
//! Expansion is pure: a `!raw` axis value stays raw in the produced case's vars.

use std::collections::BTreeMap;

use serde_json::Value as Json;

use crate::config::TestCase;
use crate::resolve::ResolveError;
use crate::template::TemplateEngine;
use crate::val::Val;

/// Expand a case's `matrix` into one case per axis combination. A case with no
/// `matrix` returns a single-element vec (identity), so callers can map every
/// case through this unconditionally.
pub fn expand_matrix(tc: &TestCase) -> Result<Vec<TestCase>, ResolveError> {
    if tc.matrix.is_empty() {
        return Ok(vec![tc.clone()]);
    }

    // BTreeMap iteration is sorted by key, which fixes both the cross-product
    // order and the `key=value` order in generated ids.
    let axes: Vec<(&String, &Vec<Val>)> = tc.matrix.iter().collect();
    for (key, values) in &axes {
        if values.is_empty() {
            return Err(ResolveError::Parse {
                path: tc.id.clone().unwrap_or_else(|| "matrix".to_string()),
                message: format!("matrix axis '{key}' has no values"),
            });
        }
    }

    // Cartesian product: grow the set of assignments one axis at a time.
    let mut combos: Vec<Vec<(String, Val)>> = vec![Vec::new()];
    for (key, values) in &axes {
        let mut next = Vec::with_capacity(combos.len() * values.len());
        for combo in &combos {
            for value in values.iter() {
                let mut extended = combo.clone();
                extended.push(((*key).clone(), value.clone()));
                next.push(extended);
            }
        }
        combos = next;
    }

    let engine = tc.matrix_id.as_ref().map(|_| TemplateEngine::new());
    let base_id = tc.id.clone().unwrap_or_default();

    let mut out = Vec::with_capacity(combos.len());
    for combo in combos {
        let id = match (&tc.matrix_id, &engine) {
            (Some(template), Some(engine)) => render_matrix_id(engine, template, &combo, &base_id)?,
            _ => default_matrix_id(&base_id, &combo),
        };

        let mut case = tc.clone();
        // The produced case is concrete — it must not re-expand or carry the
        // matrix into the serialized config snapshot.
        case.matrix = BTreeMap::new();
        case.matrix_id = None;
        case.id = Some(id);
        for (key, value) in combo {
            case.vars.insert(key, value);
        }
        out.push(case);
    }
    Ok(out)
}

/// The default id: `<base>[k1=v1,k2=v2]` with values rendered compactly.
fn default_matrix_id(base_id: &str, combo: &[(String, Val)]) -> String {
    let pairs: Vec<String> = combo
        .iter()
        .map(|(k, v)| format!("{k}={}", axis_value_label(v)))
        .collect();
    format!("{base_id}[{}]", pairs.join(","))
}

/// Render a `matrix_id` template against the combination's axis values.
fn render_matrix_id(
    engine: &TemplateEngine,
    template: &str,
    combo: &[(String, Val)],
    base_id: &str,
) -> Result<String, ResolveError> {
    let mut ctx = serde_json::Map::new();
    for (key, value) in combo {
        ctx.insert(key.clone(), value.as_json().clone());
    }
    engine
        .render_str(template, &Json::Object(ctx))
        .map_err(|e| ResolveError::Parse {
            path: format!("{base_id}.matrix_id"),
            message: e.to_string(),
        })
}

/// A compact, id-safe label for an axis value: a string as-is, anything else as
/// compact JSON (so `0` stays `0`, `true` stays `true`).
fn axis_value_label(val: &Val) -> String {
    match val.as_json() {
        Json::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(yaml: &str) -> TestCase {
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(yaml).unwrap();
        let desugared = crate::val::desugar_tags(value);
        serde_yaml_ng::from_value(desugared).unwrap()
    }

    fn ids(cases: &[TestCase]) -> Vec<String> {
        cases.iter().map(|c| c.id.clone().unwrap()).collect()
    }

    #[test]
    fn two_by_two_expands_to_four_sorted_deterministic_ids() {
        let tc = case(
            r#"
id: greet
matrix:
  style: [terse, warm]
  temperature: [0, 1]
"#,
        );
        let expanded = expand_matrix(&tc).unwrap();
        assert_eq!(
            ids(&expanded),
            vec![
                "greet[style=terse,temperature=0]",
                "greet[style=terse,temperature=1]",
                "greet[style=warm,temperature=0]",
                "greet[style=warm,temperature=1]",
            ]
        );
        // Re-running yields the exact same order (determinism).
        assert_eq!(ids(&expand_matrix(&tc).unwrap()), ids(&expanded));
    }

    /// Expansion clones the whole case, so the annotation rides along — but a
    /// pin, because losing it would silently turn every cell's XFail into a
    /// gate-failing Fail.
    #[test]
    fn expect_fail_propagates_to_every_matrix_cell() {
        let tc = case(
            r#"
id: greet
expect_fail: "known bug"
matrix:
  style: [terse, warm]
"#,
        );
        let expanded = expand_matrix(&tc).unwrap();
        assert_eq!(expanded.len(), 2);
        for cell in &expanded {
            assert!(cell.expect_fail_enabled());
            assert_eq!(cell.expect_fail_reason(), Some("known bug"));
        }
    }

    #[test]
    fn axis_values_merge_into_vars() {
        let tc = case(
            r#"
id: greet
matrix:
  style: [terse]
vars: { name: "Ada" }
"#,
        );
        let expanded = expand_matrix(&tc).unwrap();
        assert_eq!(expanded.len(), 1);
        assert_eq!(
            expanded[0].vars["style"],
            Val::Tpl(Json::String("terse".into()))
        );
        assert_eq!(
            expanded[0].vars["name"],
            Val::Tpl(Json::String("Ada".into()))
        );
    }

    #[test]
    fn axis_wins_over_base_var_of_the_same_name() {
        let tc = case(
            r#"
id: c
matrix:
  tone: [formal, casual]
vars: { tone: "default" }
"#,
        );
        let expanded = expand_matrix(&tc).unwrap();
        let tones: Vec<String> = expanded
            .iter()
            .map(|c| match &c.vars["tone"] {
                Val::Tpl(Json::String(s)) => s.clone(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(tones, vec!["formal", "casual"]);
    }

    #[test]
    fn single_axis_expands_per_value() {
        let tc = case(
            r#"
id: c
matrix:
  n: [1, 2, 3]
"#,
        );
        assert_eq!(expand_matrix(&tc).unwrap().len(), 3);
    }

    #[test]
    fn empty_axis_is_an_error() {
        let tc = case(
            r#"
id: c
matrix:
  style: []
"#,
        );
        let err = expand_matrix(&tc).unwrap_err();
        assert!(matches!(err, ResolveError::Parse { .. }), "{err:?}");
        assert!(err.to_string().contains("has no values"));
        assert!(err.to_string().contains("style"));
    }

    #[test]
    fn empty_matrix_is_identity() {
        let tc = case("id: c\nvars: { x: \"1\" }\n");
        let expanded = expand_matrix(&tc).unwrap();
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].id.as_deref(), Some("c"));
    }

    #[test]
    fn matrix_id_template_overrides_the_id() {
        let tc = case(
            r#"
id: greet
matrix_id: "{{ style }}-t{{ temperature }}"
matrix:
  style: [terse, warm]
  temperature: [0, 1]
"#,
        );
        assert_eq!(
            ids(&expand_matrix(&tc).unwrap()),
            vec!["terse-t0", "terse-t1", "warm-t0", "warm-t1"]
        );
    }

    #[test]
    fn raw_axis_value_stays_raw() {
        let tc = case(
            r#"
id: c
matrix:
  payload: [!raw "{{7*7}}"]
"#,
        );
        let expanded = expand_matrix(&tc).unwrap();
        assert!(
            expanded[0].vars["payload"].is_raw(),
            "a !raw axis value must not lose its raw marker"
        );
        assert_eq!(
            expanded[0].vars["payload"],
            Val::Raw(Json::String("{{7*7}}".into()))
        );
    }
}
