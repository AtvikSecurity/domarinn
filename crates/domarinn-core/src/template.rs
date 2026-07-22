//! Thin wrapper over [`minijinja`] — the real Jinja engine.
//!
//! Two deliberate choices:
//! * `UndefinedBehavior::Strict` — a typo'd `{{ vras.x }}` is a loud error, not
//!   a silently-empty string.
//! * [`Val::Raw`] values bypass rendering entirely, so literal template syntax
//!   in test inputs is never interpolated.

use minijinja::{Environment, UndefinedBehavior};
use serde_json::Value as Json;

use crate::val::Val;

/// Errors from template rendering or expression evaluation.
#[derive(Debug, thiserror::Error)]
#[error("template error: {0}")]
pub struct TemplateError(#[from] pub minijinja::Error);

/// A configured template engine. Cheap to clone the reference; construct once
/// per run.
pub struct TemplateEngine {
    env: Environment<'static>,
}

impl Default for TemplateEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateEngine {
    pub fn new() -> Self {
        let mut env = Environment::new();
        env.set_undefined_behavior(UndefinedBehavior::Strict);
        crate::template_fns::register(&mut env);
        Self { env }
    }

    /// Render a single template string against a JSON context.
    pub fn render_str(&self, template: &str, ctx: &Json) -> Result<String, TemplateError> {
        Ok(self.env.render_str(template, ctx)?)
    }

    /// Render a [`Val`]: [`Val::Raw`] passes through untouched; [`Val::Tpl`] has
    /// every string leaf rendered (recursively for arrays/objects).
    pub fn render_val(&self, val: &Val, ctx: &Json) -> Result<Json, TemplateError> {
        match val {
            Val::Raw(v) => Ok(v.clone()),
            Val::Tpl(v) => self.render_json(v, ctx),
        }
    }

    fn render_json(&self, value: &Json, ctx: &Json) -> Result<Json, TemplateError> {
        match value {
            Json::String(s) => Ok(Json::String(self.render_str(s, ctx)?)),
            Json::Array(items) => Ok(Json::Array(
                items
                    .iter()
                    .map(|v| self.render_json(v, ctx))
                    .collect::<Result<_, _>>()?,
            )),
            Json::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (k, v) in map {
                    out.insert(k.clone(), self.render_json(v, ctx)?);
                }
                Ok(Json::Object(out))
            }
            other => Ok(other.clone()),
        }
    }

    /// Evaluate a minijinja expression to a boolean (truthy). Powers the `jinja`
    /// assertion type and `output_expr`.
    pub fn eval_bool(&self, expr: &str, ctx: &Json) -> Result<bool, TemplateError> {
        let compiled = self.env.compile_expression(expr)?;
        let result = compiled.eval(ctx)?;
        Ok(result.is_true())
    }

    /// Evaluate a minijinja expression to a JSON value. Powers `output_expr` on
    /// HTTP providers.
    pub fn eval_value(&self, expr: &str, ctx: &Json) -> Result<Json, TemplateError> {
        let compiled = self.env.compile_expression(expr)?;
        let result = compiled.eval(ctx)?;
        serde_json::to_value(result).map_err(|e| {
            TemplateError(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                e.to_string(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_a_variable() {
        let eng = TemplateEngine::new();
        let out = eng
            .render_str("hello {{ name }}", &json!({"name": "world"}))
            .unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn strict_undefined_is_an_error() {
        let eng = TemplateEngine::new();
        assert!(eng.render_str("{{ missing }}", &json!({})).is_err());
    }

    #[test]
    fn raw_block_is_preserved() {
        let eng = TemplateEngine::new();
        let out = eng
            .render_str("{% raw %}{{7*7}}{% endraw %}", &json!({}))
            .unwrap();
        assert_eq!(out, "{{7*7}}");
    }

    #[test]
    fn raw_val_never_interpolates_ssti() {
        let eng = TemplateEngine::new();
        let val = Val::Raw(json!("{{7*7}}"));
        let out = eng.render_val(&val, &json!({})).unwrap();
        assert_eq!(out, json!("{{7*7}}"), "raw value must not become 49");
    }

    #[test]
    fn tpl_val_renders_nested_strings() {
        let eng = TemplateEngine::new();
        let val = Val::Tpl(json!({"greeting": "hi {{ name }}", "n": 3}));
        let out = eng.render_val(&val, &json!({"name": "sam"})).unwrap();
        assert_eq!(out, json!({"greeting": "hi sam", "n": 3}));
    }

    #[test]
    fn eval_bool_expression() {
        let eng = TemplateEngine::new();
        assert!(eng
            .eval_bool("output | length < 10", &json!({"output": "short"}))
            .unwrap());
        assert!(!eng
            .eval_bool("output | length < 3", &json!({"output": "longer"}))
            .unwrap());
    }

    #[test]
    fn custom_filter_is_available_in_prompts() {
        // The registered `sha256` filter is usable from an ordinary render.
        let eng = TemplateEngine::new();
        let out = eng
            .render_str("{{ doc | sha256 }}", &json!({"doc": "abc"}))
            .unwrap();
        assert_eq!(
            out,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn custom_filter_is_available_in_jinja_asserts() {
        // `regex_match` powers a `jinja` assertion.
        let eng = TemplateEngine::new();
        assert!(eng
            .eval_bool(
                "output | regex_match('^[0-9]+$')",
                &json!({"output": "12345"})
            )
            .unwrap());
        assert!(!eng
            .eval_bool(
                "output | regex_match('^[0-9]+$')",
                &json!({"output": "12a45"})
            )
            .unwrap());
    }

    #[test]
    fn raw_val_bypasses_filters() {
        // A raw value that *looks* like a filter pipeline must pass through
        // verbatim — the SSTI guard covers filters, not just `{{7*7}}`.
        let eng = TemplateEngine::new();
        let val = Val::Raw(json!("{{ 'x' | sha256 }}"));
        let out = eng.render_val(&val, &json!({})).unwrap();
        assert_eq!(out, json!("{{ 'x' | sha256 }}"));
    }
}
