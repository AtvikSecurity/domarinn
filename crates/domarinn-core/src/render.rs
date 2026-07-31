//! Building the render context for a test and rendering prompts.
//!
//! A test's `vars` become a JSON context (raw vars pass through untouched); an
//! `env` object exposes environment variables for templates that need them
//! (opt-in by the template author). Prompt `template`/`messages` content may use
//! `file://` to load from disk relative to the suite directory.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::config::{HistorySpec, Message, Prompt, PromptEntry};
use crate::template::{TemplateEngine, TemplateError};
use crate::types::ChatRole;
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
    /// A `history: file://…` transcript that loaded but did not parse as a
    /// list of `{role, content}` turns.
    #[error("history transcript {path}: {message}")]
    BadTranscript { path: String, message: String },
    /// Enforced here as well as in the loader's `validate()` diagnostics,
    /// because only the CLI runs `validate()` — an embedder calling
    /// `runner::run` directly must not get silent first-marker-wins.
    #[error("prompt '{0}' has more than one `history` marker; a prompt may have at most one")]
    DuplicateMarker(String),
    /// A `messages:` prompt whose final transcript came out empty — no turns
    /// of its own and no case history to splice. Sent as-is, an empty
    /// `messages` array draws a provider 400 that never names the cause.
    #[error(
        "prompt '{0}' rendered to an empty transcript: it has no turns and \
         the case supplied no history"
    )]
    EmptyTranscript(String),
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
/// persistence into a cache entry, so an environment value read by *those
/// templates* never enters a key or a stored entry. The consequence is
/// deliberate and documented: two runs differing only in a `{{ env.X }}` value
/// share one key (see `http_provider::warn_on_runtime_env`). `${env:X}`
/// interpolation ([`crate::interp`]) resolves at load time, before the provider
/// is built, and remains the supported way to make an environment value part of
/// the key.
///
/// It withholds that one hop and no more. A case var that reads the environment
/// — `vars: {token: "{{ env.SUT_TOKEN }}"}` — is resolved long before a provider
/// renders anything, so its value reaches the request in the clear. That is by
/// design: vars are case data, have always been cache-key members, and are
/// published in `CaseResult.vars`. A credential therefore belongs in a
/// provider's own templates as `{{ env.X }}`, never routed through a case var.
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
/// `base_dir`. The no-history path: see [`render_prompt_with_history`].
pub fn render_prompt(
    prompt: &Prompt,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<RenderedPrompt, RenderError> {
    render_prompt_with_history(prompt, ctx, engine, base_dir, &[])
}

/// Render a prompt and splice a case's (already rendered) history turns in.
///
/// Splice position, in order of precedence:
/// - a `history` marker entry in a `messages:` prompt names it explicitly;
/// - otherwise after the leading run of `system` turns, so the dominant
///   `[system, user-template]` prompt shape stays a well-formed transcript;
/// - a `template:` prompt becomes the transcript's newest `user` turn — but
///   with no history it stays [`RenderedPrompt::Text`], byte-identical to
///   before this feature existed (requests, cache keys, http `{{ prompt }}`).
///
/// An empty history makes any marker simply disappear.
pub fn render_prompt_with_history(
    prompt: &Prompt,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
    history: &[ChatMessage],
) -> Result<RenderedPrompt, RenderError> {
    match (&prompt.template, &prompt.messages) {
        (Some(template), None) => {
            let source = load_content(template, base_dir)?;
            let text = engine.render_str(&source, ctx)?;
            if history.is_empty() {
                return Ok(RenderedPrompt::Text(text));
            }
            let mut msgs = history.to_vec();
            msgs.push(ChatMessage {
                role: ChatRole::User,
                content: text,
            });
            Ok(RenderedPrompt::Messages(msgs))
        }
        (None, Some(entries)) => {
            let mut rendered = Vec::with_capacity(entries.len() + history.len());
            let mut marker_at = None;
            for entry in entries {
                match entry {
                    PromptEntry::Turn(message) => {
                        rendered.push(render_message(message, ctx, engine, base_dir)?);
                    }
                    // Position indexes the rendered turns; the marker itself
                    // never appears in the output.
                    PromptEntry::Marker(_) => {
                        if marker_at.is_some() {
                            return Err(RenderError::DuplicateMarker(prompt.id.clone()));
                        }
                        marker_at = Some(rendered.len());
                    }
                }
            }
            let at = marker_at.unwrap_or_else(|| {
                rendered
                    .iter()
                    .take_while(|m| m.role == ChatRole::System)
                    .count()
            });
            rendered.splice(at..at, history.iter().cloned());
            if rendered.is_empty() {
                return Err(RenderError::EmptyTranscript(prompt.id.clone()));
            }
            Ok(RenderedPrompt::Messages(rendered))
        }
        _ => Err(RenderError::BadPrompt(prompt.id.clone())),
    }
}

/// Render one config turn: its `content` may be `file://path`, then is
/// rendered as a template against `ctx`. The single per-turn contract, shared
/// by `messages:` prompt turns and history turns.
fn render_message(
    message: &Message,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<ChatMessage, RenderError> {
    let source = load_content(&message.content, base_dir)?;
    Ok(ChatMessage {
        role: message.role,
        content: engine.render_str(&source, ctx)?,
    })
}

/// [`render_message`] over a list.
pub fn render_messages(
    messages: &[Message],
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<Vec<ChatMessage>, RenderError> {
    messages
        .iter()
        .map(|message| render_message(message, ctx, engine, base_dir))
        .collect()
}

/// Resolve a case's `history` to rendered turns: inline turns render directly;
/// a `file://` transcript loads (sandboxed, like all `file://` content), parses
/// as a YAML/JSON list of `{role, content}`, then renders each turn.
pub fn resolve_history(
    spec: &HistorySpec,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<Vec<ChatMessage>, RenderError> {
    match spec {
        HistorySpec::Inline(turns) => render_messages(turns, ctx, engine, base_dir),
        HistorySpec::File(path) => {
            let text = load_content(path, base_dir)?;
            let turns: Vec<Message> =
                serde_yaml_ng::from_str(&text).map_err(|e| RenderError::BadTranscript {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
            render_messages(&turns, ctx, engine, base_dir)
        }
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

    /// Every value names its own variable, and a variable that exists is
    /// present — the definedness a template sees, including `env['NAME']`
    /// lookups, must match [`env_object`]'s.
    ///
    /// Asserted over a single snapshot plus one variable this test sets, rather
    /// than by comparing two snapshots: sibling tests in this binary mutate the
    /// process environment in parallel, so two consecutive reads of it are not
    /// guaranteed to agree.
    #[test]
    fn every_placeholder_env_value_names_its_own_variable() {
        std::env::set_var("DOMARINN_PLACEHOLDER_PROBE", "a real value");
        let placeholders = env_placeholder_object();
        let map = placeholders.as_object().unwrap();
        assert_eq!(
            map.get("DOMARINN_PLACEHOLDER_PROBE"),
            Some(&Json::String("${env:DOMARINN_PLACEHOLDER_PROBE}".into())),
            "a variable that is set must be present, and withheld"
        );
        for (name, value) in map {
            assert_eq!(value, &Json::String(format!("${{env:{name}}}")));
        }
        std::env::remove_var("DOMARINN_PLACEHOLDER_PROBE");
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
                PromptEntry::Turn(Message {
                    role: ChatRole::System,
                    content: "You are helpful".into(),
                }),
                PromptEntry::Turn(Message {
                    role: ChatRole::User,
                    content: "{{ request }}".into(),
                }),
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

#[cfg(test)]
mod history_tests {
    use super::*;
    use crate::config::{HistoryMarker, HistorySpec, Message, Prompt, PromptEntry};
    use crate::types::ChatRole;

    fn turn(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            role,
            content: content.into(),
        }
    }

    fn cfg_turn(role: ChatRole, content: &str) -> PromptEntry {
        PromptEntry::Turn(Message {
            role,
            content: content.into(),
        })
    }

    fn marker_prompt() -> Prompt {
        Prompt {
            id: "support".into(),
            template: None,
            messages: Some(vec![
                cfg_turn(ChatRole::System, "You are helpful"),
                PromptEntry::Marker(HistoryMarker::History),
                cfg_turn(ChatRole::User, "{{ q }}"),
            ]),
        }
    }

    fn short_history() -> Vec<ChatMessage> {
        vec![
            turn(ChatRole::User, "hi"),
            turn(ChatRole::Assistant, "hello"),
        ]
    }

    #[test]
    fn history_splices_at_the_marker() {
        let engine = TemplateEngine::new();
        let ctx = serde_json::json!({ "q": "next" });
        let rendered = render_prompt_with_history(
            &marker_prompt(),
            &ctx,
            &engine,
            Path::new("."),
            &short_history(),
        )
        .unwrap();
        match rendered {
            RenderedPrompt::Messages(msgs) => {
                let flat: Vec<_> = msgs
                    .iter()
                    .map(|m| (m.role.as_str(), m.content.as_str()))
                    .collect();
                assert_eq!(
                    flat,
                    vec![
                        ("system", "You are helpful"),
                        ("user", "hi"),
                        ("assistant", "hello"),
                        ("user", "next"),
                    ]
                );
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    /// Without a marker the history lands after the leading run of `system`
    /// turns: the dominant prompt shape is `[system, user-template]`, and a
    /// well-formed transcript keeps the system message first.
    #[test]
    fn without_a_marker_history_lands_after_the_leading_system_turns() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: None,
            messages: Some(vec![
                cfg_turn(ChatRole::System, "sys"),
                cfg_turn(ChatRole::User, "{{ q }}"),
            ]),
        };
        let ctx = serde_json::json!({ "q": "next" });
        let rendered =
            render_prompt_with_history(&prompt, &ctx, &engine, Path::new("."), &short_history())
                .unwrap();
        match rendered {
            RenderedPrompt::Messages(msgs) => {
                let roles: Vec<_> = msgs.iter().map(|m| m.role.as_str()).collect();
                assert_eq!(roles, vec!["system", "user", "assistant", "user"]);
                assert_eq!(msgs[1].content, "hi");
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    #[test]
    fn without_system_turns_history_is_prepended() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: None,
            messages: Some(vec![cfg_turn(ChatRole::User, "{{ q }}")]),
        };
        let ctx = serde_json::json!({ "q": "next" });
        let rendered =
            render_prompt_with_history(&prompt, &ctx, &engine, Path::new("."), &short_history())
                .unwrap();
        match rendered {
            RenderedPrompt::Messages(msgs) => {
                assert_eq!(msgs[0].content, "hi");
                assert_eq!(msgs[2].content, "next");
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    /// A `template:` prompt with history becomes the transcript's newest user
    /// turn — the only meaning a single text template can have mid-conversation.
    #[test]
    fn a_text_prompt_with_history_becomes_the_newest_user_turn() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: Some("Q: {{ q }}".into()),
            messages: None,
        };
        let ctx = serde_json::json!({ "q": "next" });
        let rendered =
            render_prompt_with_history(&prompt, &ctx, &engine, Path::new("."), &short_history())
                .unwrap();
        match rendered {
            RenderedPrompt::Messages(msgs) => {
                assert_eq!(msgs.len(), 3);
                assert_eq!(msgs[2].role, ChatRole::User);
                assert_eq!(msgs[2].content, "Q: next");
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    /// No history: a text prompt stays `Text` (byte-identical requests, cache
    /// keys, and http `{{ prompt }}` for every existing suite).
    #[test]
    fn a_text_prompt_without_history_stays_text() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "p".into(),
            template: Some("Q: {{ q }}".into()),
            messages: None,
        };
        let ctx = serde_json::json!({ "q": "next" });
        let rendered =
            render_prompt_with_history(&prompt, &ctx, &engine, Path::new("."), &[]).unwrap();
        assert_eq!(rendered, RenderedPrompt::Text("Q: next".into()));
    }

    #[test]
    fn an_empty_history_makes_the_marker_disappear() {
        let engine = TemplateEngine::new();
        let ctx = serde_json::json!({ "q": "next" });
        let rendered =
            render_prompt_with_history(&marker_prompt(), &ctx, &engine, Path::new("."), &[])
                .unwrap();
        match rendered {
            RenderedPrompt::Messages(msgs) => {
                let roles: Vec<_> = msgs.iter().map(|m| m.role.as_str()).collect();
                assert_eq!(roles, vec!["system", "user"]);
            }
            other => panic!("expected messages, got {other:?}"),
        }
    }

    /// `validate()` also flags this, but only the CLI runs `validate()` — the
    /// embedder entry point (`runner::run`) does not, so the render path must
    /// hold the line itself rather than silently splicing at the first marker.
    #[test]
    fn a_second_marker_is_a_render_error() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "doubled".into(),
            template: None,
            messages: Some(vec![
                PromptEntry::Marker(HistoryMarker::History),
                cfg_turn(ChatRole::User, "hi"),
                PromptEntry::Marker(HistoryMarker::History),
            ]),
        };
        let err = render_prompt_with_history(
            &prompt,
            &serde_json::json!({}),
            &engine,
            Path::new("."),
            &short_history(),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("marker") && err.contains("doubled"),
            "error must name the problem and the prompt: {err}"
        );
    }

    /// A marker-only prompt is legal — it renders the case's history as the
    /// whole transcript — but a case that then supplies no history would send
    /// an empty `messages` array, which OpenAI- and Anthropic-shaped APIs
    /// reject with a 400 that never names the real cause. Fail at render, with
    /// the cause.
    #[test]
    fn an_empty_final_transcript_is_a_render_error() {
        let engine = TemplateEngine::new();
        let prompt = Prompt {
            id: "marker-only".into(),
            template: None,
            messages: Some(vec![PromptEntry::Marker(HistoryMarker::History)]),
        };
        // With history: fine — the history is the transcript.
        let ok = render_prompt_with_history(
            &prompt,
            &serde_json::json!({}),
            &engine,
            Path::new("."),
            &short_history(),
        )
        .unwrap();
        assert!(matches!(ok, RenderedPrompt::Messages(m) if m.len() == 2));
        // Without: an error that names the empty transcript, not a provider 400.
        let err = render_prompt_with_history(
            &prompt,
            &serde_json::json!({}),
            &engine,
            Path::new("."),
            &[],
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("marker-only") && err.contains("no turns"),
            "error must name the prompt and the cause: {err}"
        );
    }

    #[test]
    fn inline_history_turns_render_vars() {
        let engine = TemplateEngine::new();
        let spec = HistorySpec::Inline(vec![Message {
            role: ChatRole::Assistant,
            content: "{{ prior }}".into(),
        }]);
        let ctx = serde_json::json!({ "prior": "the earlier answer" });
        let turns = resolve_history(&spec, &ctx, &engine, Path::new(".")).unwrap();
        assert_eq!(turns[0].content, "the earlier answer");
    }

    #[test]
    fn an_inline_history_turn_loads_file_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("turn.txt"), "from disk: {{ x }}").unwrap();
        let engine = TemplateEngine::new();
        let spec = HistorySpec::Inline(vec![Message {
            role: ChatRole::User,
            content: "file://turn.txt".into(),
        }]);
        let ctx = serde_json::json!({ "x": "ok" });
        let turns = resolve_history(&spec, &ctx, &engine, dir.path()).unwrap();
        assert_eq!(turns[0].content, "from disk: ok");
    }

    #[test]
    fn a_file_history_loads_a_whole_transcript() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("convo.yaml"),
            "- role: user\n  content: hi\n- role: assistant\n  content: \"{{ prior }}\"\n",
        )
        .unwrap();
        let engine = TemplateEngine::new();
        let spec = HistorySpec::File("file://convo.yaml".into());
        let ctx = serde_json::json!({ "prior": "hello" });
        let turns = resolve_history(&spec, &ctx, &engine, dir.path()).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].content, "hi");
        assert_eq!(turns[1].content, "hello");
        assert_eq!(turns[1].role, ChatRole::Assistant);
    }

    /// Same security property as prompt `file://` content: a transcript path
    /// must not read outside the suite directory.
    #[test]
    fn a_file_history_outside_the_suite_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        std::fs::write(
            parent.path().join("convo.yaml"),
            "- role: user\n  content: x\n",
        )
        .unwrap();
        let base = parent.path().join("suite");
        std::fs::create_dir(&base).unwrap();

        let engine = TemplateEngine::new();
        let spec = HistorySpec::File("file://../convo.yaml".into());
        let err = resolve_history(&spec, &serde_json::json!({}), &engine, &base).unwrap_err();
        assert!(
            matches!(err, RenderError::Sandbox(_)),
            "traversal must be a sandbox error: {err:?}"
        );
    }

    #[test]
    fn a_malformed_transcript_names_the_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("convo.yaml"), "not: a\nlist: here\n").unwrap();
        let engine = TemplateEngine::new();
        let spec = HistorySpec::File("file://convo.yaml".into());
        let err = resolve_history(&spec, &serde_json::json!({}), &engine, dir.path())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("convo.yaml"),
            "error must name the file: {err}"
        );
    }
}
