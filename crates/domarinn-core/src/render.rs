//! Building the render context for a test and rendering prompts.
//!
//! A test's `vars` become a JSON context (raw vars pass through untouched); an
//! `env` object exposes environment variables for templates that need them
//! (opt-in by the template author). Prompt `template`/`messages` content may use
//! `file://` to load from disk relative to the suite directory.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::config::Prompt;
use crate::template::{TemplateEngine, TemplateError};
use crate::types::{ChatMessage, RenderedPrompt};
use crate::val::Val;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error("loading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// A `file://` prompt/message reference that would read outside the suite
    /// directory. Closes a `file://../../etc/passwd` sandbox-escape hole.
    #[error(transparent)]
    Sandbox(#[from] crate::sandbox::SandboxError),
    #[error("prompt '{0}' must set exactly one of 'template' or 'messages'")]
    BadPrompt(String),
}

/// Render a test's vars to a plain map.
///
/// Each var is rendered (unless it is [`Val::Raw`]) against a base context that
/// exposes `env`. The result is the rendered vars only — it does NOT include the
/// environment, so it is safe to use as the provider request identity (cache key)
/// and to hand to exec providers without leaking the whole environment.
pub fn render_vars(
    vars: &BTreeMap<String, Val>,
    engine: &TemplateEngine,
) -> Result<serde_json::Map<String, Json>, RenderError> {
    let base = serde_json::json!({ "env": env_object() });
    let mut out = serde_json::Map::new();
    for (key, val) in vars {
        out.insert(key.clone(), engine.render_val(val, &base)?);
    }
    Ok(out)
}

/// Build a template context from rendered vars plus an `env` object. Used for
/// rendering prompts and evaluating `jinja` assertions, where `{{ env.X }}` is
/// allowed. `env` is added here and never enters the request identity.
pub fn context_with_env(vars: &serde_json::Map<String, Json>) -> Json {
    let mut ctx = vars.clone();
    ctx.insert("env".to_string(), env_object());
    Json::Object(ctx)
}

/// The rendered vars plus `env`, as one context. Convenience wrapper.
pub fn build_context(
    vars: &BTreeMap<String, Val>,
    engine: &TemplateEngine,
) -> Result<Json, RenderError> {
    Ok(context_with_env(&render_vars(vars, engine)?))
}

/// A snapshot of the process environment as a JSON object of strings.
pub fn env_object() -> Json {
    let map: serde_json::Map<String, Json> = std::env::vars()
        .map(|(k, v)| (k, Json::String(v)))
        .collect();
    Json::Object(map)
}

/// The environment with every value replaced by the literal `${env:NAME}`.
///
/// The keys are [`env_object`]'s, so a template renders against exactly the same
/// definedness — `{{ env.X }}`, `env['X']`, and `env` iteration all see what
/// they would see on the real call — and only the values are withheld.
///
/// Used to render an `http` provider's url/headers/body for cache keying and for
/// persistence into a cache entry, so a call-time credential never enters a key
/// or a stored entry. The consequence is deliberate and documented: two runs
/// differing only in a `{{ env.X }}` value share one key (see
/// `http_provider::warn_on_runtime_env`). `${env:X}` interpolation
/// ([`crate::interp`]) resolves at load time, before the provider is built, and
/// remains the supported way to make an environment value part of the key.
pub fn env_placeholder_object() -> Json {
    let map: serde_json::Map<String, Json> = std::env::vars()
        .map(|(k, _)| {
            let placeholder = format!("${{env:{k}}}");
            (k, Json::String(placeholder))
        })
        .collect();
    Json::Object(map)
}

/// Render a prompt against a context, loading `file://` content relative to
/// `base_dir`.
pub fn render_prompt(
    prompt: &Prompt,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<RenderedPrompt, RenderError> {
    match (&prompt.template, &prompt.messages) {
        (Some(template), None) => {
            let source = load_content(template, base_dir)?;
            Ok(RenderedPrompt::Text(engine.render_str(&source, ctx)?))
        }
        (None, Some(messages)) => {
            let mut rendered = Vec::with_capacity(messages.len());
            for message in messages {
                let source = load_content(&message.content, base_dir)?;
                rendered.push(ChatMessage {
                    role: message.role,
                    content: engine.render_str(&source, ctx)?,
                });
            }
            Ok(RenderedPrompt::Messages(rendered))
        }
        _ => Err(RenderError::BadPrompt(prompt.id.clone())),
    }
}

/// If `spec` is `file://<path>`, read the file relative to `base_dir`; otherwise
/// return `spec` unchanged.
///
/// The path is resolved through [`crate::sandbox`], so a `file://../../etc/passwd`
/// (or a symlink pointing outside the suite) is refused rather than read.
pub fn load_content(spec: &str, base_dir: &Path) -> Result<String, RenderError> {
    if let Some(rel) = spec.strip_prefix("file://") {
        let path = crate::sandbox::resolve_within(base_dir, rel)?;
        std::fs::read_to_string(&path).map_err(|source| RenderError::Io {
            path: path.display().to_string(),
            source,
        })
    } else {
        Ok(spec.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Message;
    use crate::types::ChatRole;

    #[test]
    fn raw_var_passes_through_context_unrendered() {
        let engine = TemplateEngine::new();
        let mut vars = BTreeMap::new();
        vars.insert(
            "payload".to_string(),
            Val::Raw(Json::String("{{7*7}}".into())),
        );
        vars.insert(
            "greeting".to_string(),
            Val::Tpl(Json::String("hi {{ env.DOMARINN_TEST_NAME }}".into())),
        );
        std::env::set_var("DOMARINN_TEST_NAME", "sam");
        let ctx = build_context(&vars, &engine).unwrap();
        assert_eq!(ctx["payload"], Json::String("{{7*7}}".into()));
        assert_eq!(ctx["greeting"], Json::String("hi sam".into()));
        std::env::remove_var("DOMARINN_TEST_NAME");
    }

    /// Same keys as [`env_object`] — the definedness semantics a template sees
    /// must not change, including `env['NAME']` lookups — with every value
    /// replaced by its placeholder.
    #[test]
    fn every_placeholder_env_value_names_its_own_variable() {
        let placeholders = env_placeholder_object();
        let real = env_object();
        assert_eq!(
            placeholders.as_object().unwrap().keys().collect::<Vec<_>>(),
            real.as_object().unwrap().keys().collect::<Vec<_>>()
        );
        assert!(
            !placeholders.as_object().unwrap().is_empty(),
            "no env at all"
        );
        for (name, value) in placeholders.as_object().unwrap() {
            assert_eq!(value, &Json::String(format!("${{env:{name}}}")));
        }
    }

    #[test]
    fn renders_a_text_prompt() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: Some("Summarize: {{ doc }}".into()),
            messages: None,
        };
        let ctx = serde_json::json!({ "doc": "hello" });
        let rendered = render_prompt(&prompt, &ctx, &engine, Path::new(".")).unwrap();
        assert_eq!(rendered, RenderedPrompt::Text("Summarize: hello".into()));
    }

    #[test]
    fn renders_message_prompts() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: None,
            messages: Some(vec![
                Message {
                    role: ChatRole::System,
                    content: "You are helpful".into(),
                },
                Message {
                    role: ChatRole::User,
                    content: "{{ request }}".into(),
                },
            ]),
        };
        let ctx = serde_json::json!({ "request": "hi" });
        match render_prompt(&prompt, &ctx, &engine, Path::new(".")).unwrap() {
            RenderedPrompt::Messages(msgs) => {
                assert_eq!(msgs.len(), 2);
                assert_eq!(msgs[1].content, "hi");
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    #[test]
    fn loads_file_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("sys.j2"), "System: {{ x }}").unwrap();
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: Some("file://sys.j2".into()),
            messages: None,
        };
        let ctx = serde_json::json!({ "x": "ok" });
        let rendered = render_prompt(&prompt, &ctx, &engine, dir.path()).unwrap();
        assert_eq!(rendered, RenderedPrompt::Text("System: ok".into()));
    }

    #[test]
    fn file_traversal_out_of_the_suite_is_refused() {
        // Security regression: a `file://../secret` reference must NOT read the
        // file outside the suite directory. Before the sandbox fix this escaped.
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(parent.path().join("secret.txt"), "TOP SECRET").unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();

        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: Some("file://../secret.txt".into()),
            messages: None,
        };
        let err = render_prompt(&prompt, &serde_json::json!({}), &engine, &base).unwrap_err();
        assert!(
            matches!(err, RenderError::Sandbox(_)),
            "traversal must be a sandbox error, not a successful read: {err:?}"
        );
        assert!(err.to_string().contains("refuses to read outside"));
    }

    #[test]
    fn both_template_and_messages_is_an_error() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "bad".into(),
            template: Some("x".into()),
            messages: Some(vec![]),
        };
        assert!(render_prompt(&prompt, &serde_json::json!({}), &engine, Path::new(".")).is_err());
    }
}
