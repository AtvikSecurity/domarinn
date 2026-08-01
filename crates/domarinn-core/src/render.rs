//! Building the render context for a test and rendering prompts.
//!
//! A test's `vars` become a JSON context (raw vars pass through untouched); an
//! `env` object exposes environment variables for templates that need them
//! (opt-in by the template author). Prompt `template`/`messages` content may use
//! `file://` to load from disk relative to the suite directory.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value as Json;

use crate::config::{
    ContentBlockSpec, HistorySpec, Message, MessageContentSpec, Prompt, PromptEntry,
};
use crate::result::ToolCall;
use crate::template::{TemplateEngine, TemplateError};
use crate::types::ChatRole;
use crate::types::{ChatMessage, ContentBlock, MessageContent, RenderedPrompt};
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
    /// A turn that cannot mean anything: no content and no `tool_calls`, or a
    /// tool field on a role that cannot carry it. Enforced here as well as in
    /// `validate()` for the same reason as [`Self::DuplicateMarker`], and
    /// because a `file://` transcript is not read until run time.
    #[error("history turn: {0}")]
    BadTurn(String),
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
            msgs.push(ChatMessage::text(ChatRole::User, text));
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
///
/// Two things are deliberately **not** templated:
///
/// - a `thinking` block, because its `signature` is a vendor integrity token
///   over these exact bytes and any rewrite invalidates the replay;
/// - a tool call's `name` and `id`, for the same reason `tools[].name` is not:
///   the declared surface and a call into it must be spelled the same way, and
///   an id is a correlation token the docs already promise is never interpreted.
///
/// A call's `arguments`, by contrast, is per-case author-written data — the
/// same *kind* of thing as a `vars:` value — so it goes through the same
/// string-leaf renderer those use, which keeps `{order_id: 1042}` an integer.
fn render_message(
    message: &Message,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<ChatMessage, RenderError> {
    if let Some(problem) = crate::config_history::turn_problem(message) {
        return Err(RenderError::BadTurn(problem));
    }
    let content = match &message.content {
        Some(spec) => render_content(spec, ctx, engine, base_dir)?,
        None => MessageContent::Text(String::new()),
    };
    let tool_calls = message
        .tool_calls
        .iter()
        .map(|tc| {
            Ok(ToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                // A call with no arguments is a call with an *empty* arguments
                // object. Both vendor mappings already normalize this, but an
                // `exec` child reads the serialized turn directly, and
                // `docs/reference/protocol.md` promises it a decoded object —
                // so normalize once, here, rather than per provider.
                arguments: match engine.render_json(&tc.arguments, ctx)? {
                    Json::Null => serde_json::json!({}),
                    rendered => rendered,
                },
            })
        })
        .collect::<Result<Vec<_>, RenderError>>()?;
    Ok(ChatMessage {
        role: message.role,
        content,
        tool_calls,
        tool_call_id: message.tool_call_id.clone(),
    })
}

/// [`render_message`]'s content half: prose keeps the `file://`-then-template
/// contract; a `thinking` block is copied through untouched.
fn render_content(
    spec: &MessageContentSpec,
    ctx: &Json,
    engine: &TemplateEngine,
    base_dir: &Path,
) -> Result<MessageContent, RenderError> {
    match spec {
        MessageContentSpec::Text(s) => {
            let source = load_content(s, base_dir)?;
            Ok(MessageContent::Text(engine.render_str(&source, ctx)?))
        }
        MessageContentSpec::Blocks(blocks) => {
            let rendered = blocks
                .iter()
                .map(|b| match b {
                    ContentBlockSpec::Text { text } => {
                        let source = load_content(text, base_dir)?;
                        Ok(ContentBlock::Text {
                            text: engine.render_str(&source, ctx)?,
                        })
                    }
                    // Verbatim, signature included. See the fn doc.
                    ContentBlockSpec::Thinking {
                        thinking,
                        signature,
                    } => Ok(ContentBlock::Thinking {
                        thinking: thinking.clone(),
                        signature: signature.clone(),
                    }),
                })
                .collect::<Result<Vec<_>, RenderError>>()?;
            Ok(MessageContent::Blocks(rendered))
        }
    }
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
                PromptEntry::Turn(Message::text(ChatRole::System, "You are helpful")),
                PromptEntry::Turn(Message::text(ChatRole::User, "{{ request }}")),
            ]),
        };
        let ctx = serde_json::json!({ "request": "hi" });
        match render_prompt(&prompt, &ctx, &engine, Path::new(".")).unwrap() {
            RenderedPrompt::Messages(msgs) => {
                assert_eq!(msgs.len(), 2);
                assert_eq!(msgs[1].content.text(), "hi");
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
        ChatMessage::text(role, content)
    }

    fn cfg_turn(role: ChatRole, content: &str) -> PromptEntry {
        PromptEntry::Turn(Message::text(role, content))
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
                    .map(|m| (m.role.as_str(), m.content.text().into_owned()))
                    .collect();
                assert_eq!(
                    flat,
                    vec![
                        ("system", "You are helpful".to_string()),
                        ("user", "hi".to_string()),
                        ("assistant", "hello".to_string()),
                        ("user", "next".to_string()),
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
                assert_eq!(msgs[1].content.text(), "hi");
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
                assert_eq!(msgs[0].content.text(), "hi");
                assert_eq!(msgs[2].content.text(), "next");
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
                assert_eq!(msgs[2].content.text(), "Q: next");
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
        let spec = HistorySpec::Inline(vec![Message::text(ChatRole::Assistant, "{{ prior }}")]);
        let ctx = serde_json::json!({ "prior": "the earlier answer" });
        let turns = resolve_history(&spec, &ctx, &engine, Path::new(".")).unwrap();
        assert_eq!(turns[0].content.text(), "the earlier answer");
    }

    #[test]
    fn an_inline_history_turn_loads_file_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("turn.txt"), "from disk: {{ x }}").unwrap();
        let engine = TemplateEngine::new();
        let spec = HistorySpec::Inline(vec![Message::text(ChatRole::User, "file://turn.txt")]);
        let ctx = serde_json::json!({ "x": "ok" });
        let turns = resolve_history(&spec, &ctx, &engine, dir.path()).unwrap();
        assert_eq!(turns[0].content.text(), "from disk: ok");
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
        assert_eq!(turns[0].content.text(), "hi");
        assert_eq!(turns[1].content.text(), "hello");
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

/// Turns that carry tool calls, tool results, or content blocks.
#[cfg(test)]
mod tool_history_tests {
    use super::*;
    use crate::config::{ContentBlockSpec, Message, MessageContentSpec, ToolCallSpec};
    use crate::types::ChatRole;

    fn render_one(message: &Message, ctx: &Json) -> Result<ChatMessage, RenderError> {
        render_message(message, ctx, &TemplateEngine::new(), Path::new("."))
    }

    /// Arguments are templated leaf by leaf, so a number stays a number.
    ///
    /// The alternative — stringify, template, reparse — would turn `5` into
    /// `"5"`, and a `tool-call` assertion comparing `args` against the decoded
    /// object would then never match.
    #[test]
    fn tool_call_arguments_render_against_the_case_vars() {
        let message = Message {
            content: None,
            tool_calls: vec![ToolCallSpec {
                id: None,
                name: "lookup".into(),
                arguments: serde_json::json!({"city": "{{ city }}", "limit": 5}),
            }],
            ..Message::text(ChatRole::Assistant, "")
        };
        let ctx = serde_json::json!({ "city": "Reykjavik" });
        let out = render_one(&message, &ctx).unwrap();
        assert_eq!(out.tool_calls[0].arguments["city"], "Reykjavik");
        assert_eq!(
            out.tool_calls[0].arguments["limit"],
            serde_json::json!(5),
            "a numeric argument must not become a string"
        );
    }

    /// The one silent-failure mode this feature has: Anthropic's `signature` is
    /// an integrity token over the exact thinking bytes, so rendering a
    /// template inside them would invalidate the replay.
    #[test]
    fn a_thinking_block_is_never_templated() {
        let message = Message {
            content: Some(MessageContentSpec::Blocks(vec![
                ContentBlockSpec::Thinking {
                    thinking: "the city is {{ city }}".into(),
                    signature: Some("ErUBCk".into()),
                },
                ContentBlockSpec::Text {
                    text: "looking up {{ city }}".into(),
                },
            ])),
            ..Message::text(ChatRole::Assistant, "")
        };
        let ctx = serde_json::json!({ "city": "Reykjavik" });
        let out = render_one(&message, &ctx).unwrap();
        match &out.content {
            MessageContent::Blocks(blocks) => {
                assert_eq!(
                    blocks[0],
                    ContentBlock::Thinking {
                        thinking: "the city is {{ city }}".into(),
                        signature: Some("ErUBCk".into()),
                    },
                    "thinking must survive verbatim, signature included"
                );
                // Prose beside it still renders normally.
                assert_eq!(
                    blocks[1],
                    ContentBlock::Text {
                        text: "looking up Reykjavik".into()
                    }
                );
            }
            other => panic!("expected blocks, got {other:?}"),
        }
    }

    /// A call's `name` matches a declared `tools[].name`, which is not
    /// templated either; an `id` is a correlation token the docs promise is
    /// never interpreted.
    #[test]
    fn a_tool_call_name_and_id_are_not_templated() {
        let message = Message {
            content: None,
            tool_calls: vec![ToolCallSpec {
                id: Some("{{ city }}".into()),
                name: "{{ city }}".into(),
                arguments: Json::Null,
            }],
            ..Message::text(ChatRole::Assistant, "")
        };
        let out = render_one(&message, &serde_json::json!({ "city": "Reykjavik" })).unwrap();
        assert_eq!(out.tool_calls[0].name, "{{ city }}");
        assert_eq!(out.tool_calls[0].id.as_deref(), Some("{{ city }}"));
    }

    /// Enforced at render as well as in `validate`, because a `file://`
    /// transcript is not read until run time.
    #[test]
    fn a_turn_with_neither_content_nor_calls_is_a_render_error() {
        let message = Message {
            role: ChatRole::User,
            content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        };
        let err = render_one(&message, &serde_json::json!({})).unwrap_err();
        assert!(
            matches!(err, RenderError::BadTurn(_)),
            "expected BadTurn, got {err:?}"
        );
    }

    /// The cache tripwire at the render layer: a plain turn still renders to
    /// exactly the two keys it always did.
    #[test]
    fn a_plain_turn_still_renders_to_the_same_two_keys() {
        let out = render_one(
            &Message::text(ChatRole::User, "{{ q }}"),
            &serde_json::json!({ "q": "hi" }),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&out).unwrap(),
            serde_json::json!({"role": "user", "content": "hi"})
        );
    }

    /// `protocol.md` promises an exec child a decoded arguments *object*, and
    /// a call written without arguments must honour that rather than sending
    /// `null` — which both vendor mappings already normalize away.
    #[test]
    fn a_call_without_arguments_renders_an_empty_object() {
        let message = Message {
            content: None,
            tool_calls: vec![ToolCallSpec {
                id: None,
                name: "ping".into(),
                arguments: Json::Null,
            }],
            ..Message::text(ChatRole::Assistant, "")
        };
        let out = render_one(&message, &serde_json::json!({})).unwrap();
        assert_eq!(out.tool_calls[0].arguments, serde_json::json!({}));
    }
}
