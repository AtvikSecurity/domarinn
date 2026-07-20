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
                    role: message.role.clone(),
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
pub fn load_content(spec: &str, base_dir: &Path) -> Result<String, RenderError> {
    if let Some(rel) = spec.strip_prefix("file://") {
        let path = base_dir.join(rel);
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
            Val::Tpl(Json::String("hi {{ env.MEASURELLM_TEST_NAME }}".into())),
        );
        std::env::set_var("MEASURELLM_TEST_NAME", "sam");
        let ctx = build_context(&vars, &engine).unwrap();
        assert_eq!(ctx["payload"], Json::String("{{7*7}}".into()));
        assert_eq!(ctx["greeting"], Json::String("hi sam".into()));
        std::env::remove_var("MEASURELLM_TEST_NAME");
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
                    role: "system".into(),
                    content: "You are helpful".into(),
                },
                Message {
                    role: "user".into(),
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
