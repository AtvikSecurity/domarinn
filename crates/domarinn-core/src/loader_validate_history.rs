//! Transcript-shape checks: the history mistakes that are near-certain provider
//! 400s, reported at author time instead of run time.
//!
//! Why say anything at all: at run time these surface as an **errored cell**,
//! which reads like an outage (credentials, rate limit, provider down) rather
//! than a typo in turn 3. And an errored cell is never cached, so a malformed
//! history in a large suite re-pays on every run until somebody notices.
//!
//! # Three warnings and one error
//!
//! A history opening on `assistant` or on `tool`, and a blank `content`, are
//! **warnings**. Role ordering is deliberately not validated — it is the
//! provider's contract, as `docs/reference/domarinn-yaml.md` says — and an
//! Anthropic assistant *prefill* is a legitimate instance of the first shape.
//! So domarinn says what it sees and gets out of the way. None has a
//! render-time counterpart,
//! unlike [`crate::render::RenderError::DuplicateMarker`]: that one is
//! duplicated because it is an *error* and an embedder skipping `validate` must
//! not get silent wrong behaviour, whereas these are advice and the run is
//! meant to proceed.
//!
//! [`check_turn_shape`] is the exception, and an **error**: a turn with no
//! content and no `tool_calls` cannot mean anything to any provider. That one
//! *is* enforced twice — here for the turns `validate` can see, and in
//! [`crate::render`] for the ones it cannot.
//!
//! # What this can see
//!
//! `validate` runs on freshly loaded config, so:
//!
//! - a `history: file://…` transcript is **not** read (it is loaded in
//!   [`crate::render::resolve_history`] at run time);
//! - `defaults.history` has **not** been merged into cases yet;
//! - `tests:` sources are **unexpanded**, so only inline cases are visible —
//!   a CSV `__history` column or a generator's output is not.
//!
//! That is the documented concession: warning on the statically visible cases
//! catches most of it, and pretending otherwise would mean rendering.

use crate::config::{Message, Prompt, PromptEntry, Suite, TestSource};
use crate::loader_validate::Issue;
use crate::types::ChatRole;

/// Append every transcript-shape warning this suite earns.
pub(crate) fn check(suite: &Suite, issues: &mut Vec<Issue>) {
    let leads = history_can_land_first(suite);

    if let Some(crate::config::HistorySpec::Inline(turns)) =
        suite.defaults.as_ref().and_then(|d| d.history.as_ref())
    {
        check_history(turns, "defaults.history", leads, issues);
    }

    for (i, source) in suite.tests.iter().enumerate() {
        // Glob and generator sources do not exist yet — the same skip
        // `check_unknown_flatten_keys` makes for non-inline sources.
        let TestSource::Inline(test) = source else {
            continue;
        };
        if let Some(crate::config::HistorySpec::Inline(turns)) = &test.history {
            check_history(turns, &format!("tests[{i}].history"), leads, issues);
        }
    }

    // A blank turn in a `messages:` prompt is the same provider 400 with a
    // wider blast radius — it breaks every case, not one. `messages: []` is
    // already an error next door; `[{role: user, content: ""}]` is the same
    // typo one keystroke away and was silent.
    for (i, prompt) in suite.prompts.iter().enumerate() {
        for (j, entry) in prompt.messages.iter().flatten().enumerate() {
            if let PromptEntry::Turn(turn) = entry {
                let path = format!("prompts[{i}].messages[{j}]");
                check_turn_shape(turn, &path, issues);
                if is_blank(turn) {
                    issues.push(blank_content(&path));
                }
            }
        }
    }
}

/// A turn that cannot mean anything at all — no content and no `tool_calls`, or
/// a tool field on a role that cannot carry it.
///
/// An **error**, not a warning, and the only one in this module: unlike the two
/// shapes above, there is no provider for which this is legitimate. It is the
/// same rule [`crate::render`] enforces via
/// [`crate::config_history::turn_problem`] — reported here for the turns
/// `validate` can see, so the author learns at author time rather than from an
/// errored cell.
fn check_turn_shape(turn: &Message, path: &str, issues: &mut Vec<Issue>) {
    if let Some(problem) = crate::config_history::turn_problem(turn) {
        issues.push(Issue::new(path, problem));
    }
}

/// The checks over one resolved list of turns.
fn check_history(turns: &[Message], path: &str, leads: bool, issues: &mut Vec<Issue>) {
    if leads {
        if let Some((i, turn)) = first_non_system(turns) {
            match turn.role {
                ChatRole::Assistant => issues.push(Issue::warning(
                    format!("{path}[{i}]"),
                    "first turn is `assistant`, and this suite splices history at the \
                     front of the transcript, where providers require the opening turn \
                     to be `user`. An Anthropic assistant *prefill* is the legitimate \
                     exception; otherwise this is a 400 at run time — an errored cell, \
                     which is never cached, so it is re-paid on every run",
                )),
                // Stricter in substance than the assistant case — a result with
                // nothing to answer has no legitimate reading at all — but kept
                // a warning for consistency with the documented stance that
                // role ordering is the provider's contract.
                ChatRole::Tool => issues.push(Issue::warning(
                    format!("{path}[{i}]"),
                    "first turn is `tool`, so it answers a call no earlier turn made. \
                     This suite splices history at the front of the transcript, where \
                     there is nothing before it; both providers reject a tool result \
                     with no matching call",
                )),
                ChatRole::System | ChatRole::User => {}
            }
        }
    }
    for (i, turn) in turns.iter().enumerate() {
        check_turn_shape(turn, &format!("{path}[{i}]"), issues);
        if is_blank(turn) {
            issues.push(blank_content(&format!("{path}[{i}]")));
        }
    }
}

fn blank_content(path: &str) -> Issue {
    Issue::warning(
        path,
        "`content` is empty or whitespace-only; providers reject an empty text \
         block with a 400. Delete the turn, or give it text",
    )
}

/// A turn with nothing in it that a provider could receive.
///
/// Only *statically visible* emptiness: `"{{ note }}"` is not blank here and may
/// still render blank, but predicting that would mean rendering. A turn that
/// carries `tool_calls` is not blank even with no prose — the call is the point.
fn is_blank(turn: &Message) -> bool {
    turn.tool_calls.is_empty() && turn.content.as_ref().is_some_and(|c| c.is_blank())
}

/// The first turn a provider will actually judge, skipping any leading `system`
/// preamble — every provider treats those as preamble, and `anthropic` hoists
/// them out of the message array entirely.
fn first_non_system(turns: &[Message]) -> Option<(usize, &Message)> {
    turns
        .iter()
        .enumerate()
        .find(|(_, m)| m.role != ChatRole::System)
}

/// Does this suite ever splice a case's history at the **front** of the
/// composed transcript?
///
/// A history that opens on `assistant` is only broken when it lands first. A
/// prompt shaped `[system, user, history]` makes exactly that history correct,
/// and warning about it would be a false positive on a legal suite.
///
/// The question is per-**suite**, not per-prompt, because a case is not bound to
/// a prompt: a run is the providers × prompts × tests matrix, so every case
/// meets every prompt. If any prompt lands history at the front, the case hits
/// it.
fn history_can_land_first(suite: &Suite) -> bool {
    // No `prompts:` block at all: the case's history *is* the transcript.
    if suite.prompts.is_empty() {
        return true;
    }
    suite.prompts.iter().any(prompt_puts_history_first)
}

/// One prompt's splice position, mirroring [`crate::render::render_prompt_with_history`]
/// shape for shape.
fn prompt_puts_history_first(prompt: &Prompt) -> bool {
    let Some(entries) = &prompt.messages else {
        // A `template:` prompt renders `history + [user: template]`, so history
        // is always the opening turn.
        //
        // A prompt with neither `template` nor `messages` also lands here. That
        // is already an error, so the suite cannot run; one extra warning
        // alongside it costs nothing.
        return true;
    };
    // `position` takes the FIRST marker. A second marker is already an error
    // here and a render error at run time, so the suite that would make this
    // ambiguous never runs.
    match entries
        .iter()
        .position(|e| matches!(e, PromptEntry::Marker(_)))
    {
        // An explicit marker: history lands where it says, which is the front
        // iff everything before it is a `system` turn.
        Some(at) => entries[..at].iter().all(is_system_turn),
        // No marker: render splices after the leading run of `system` turns,
        // which is the front by definition.
        None => true,
    }
}

fn is_system_turn(entry: &PromptEntry) -> bool {
    matches!(entry, PromptEntry::Turn(m) if m.role == ChatRole::System)
}

#[cfg(test)]
mod tests {
    use crate::loader::load_str_raw;
    use crate::loader_validate::{validate, Severity};

    const PROVIDER: &str = "providers:\n  - {id: p, type: openai, model: gpt-x}\n";

    fn check_yaml(yaml: &str) -> crate::loader_validate::Validation {
        let (suite, raw) = load_str_raw(yaml).unwrap();
        validate(&suite, &raw)
    }

    /// The load-bearing half: a warning must not make the suite unrunnable.
    #[test]
    fn assistant_first_history_warns_without_making_the_suite_invalid() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: assistant, content: \"hi\"}}\n"
        ));
        let hit = report
            .warnings()
            .find(|i| i.path == "tests[0].history[0]")
            .unwrap_or_else(|| panic!("expected a warning, got {:?}", report.issues()));
        assert_eq!(hit.severity, Severity::Warning);
        assert!(hit.message.contains("assistant"));
        assert!(!report.has_errors(), "a warning must not fail the suite");
    }

    /// Example 41's shape: `[system, history, user]` still lands history first.
    #[test]
    fn assistant_first_warns_under_a_marker_that_follows_only_system_turns() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}prompts:\n  - id: s\n    messages:\n      \
             - {{role: system, content: \"be terse\"}}\n      - history\n      \
             - {{role: user, content: \"q\"}}\ntests:\n  - id: t\n    history:\n      \
             - {{role: assistant, content: \"hi\"}}\n"
        ));
        assert!(report.warnings().any(|i| i.path == "tests[0].history[0]"));
    }

    /// **The core nuance.** With a marker after a `user` turn, the history is
    /// not the transcript's opening, so an `assistant` first turn is correct.
    #[test]
    fn assistant_first_is_not_flagged_when_a_marker_puts_history_after_a_user_turn() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}prompts:\n  - id: s\n    messages:\n      \
             - {{role: system, content: \"be terse\"}}\n      \
             - {{role: user, content: \"q\"}}\n      - history\ntests:\n  - id: t\n    \
             history:\n      - {{role: assistant, content: \"hi\"}}\n"
        ));
        assert!(
            report.is_clean(),
            "history after a user turn may open on assistant: {:?}",
            report.issues()
        );
    }

    /// The matrix-semantics test: a case meets *every* prompt, so one
    /// front-splicing prompt is enough to warn.
    #[test]
    fn a_mixed_prompt_set_warns_because_a_case_meets_every_prompt() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}prompts:\n  - id: after\n    messages:\n      \
             - {{role: user, content: \"q\"}}\n      - history\n  - id: before\n    \
             messages:\n      - history\n      - {{role: user, content: \"q\"}}\n\
             tests:\n  - id: t\n    history:\n      - {{role: assistant, content: \"hi\"}}\n"
        ));
        assert!(report.warnings().any(|i| i.path == "tests[0].history[0]"));
    }

    /// A broken default multiplies across every case that does not override.
    #[test]
    fn an_assistant_first_default_history_warns() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}defaults:\n  history:\n    \
             - {{role: assistant, content: \"hi\"}}\ntests:\n  - id: t\n"
        ));
        assert!(report.warnings().any(|i| i.path == "defaults.history[0]"));
    }

    /// Leading `system` turns are preamble, so the first *judged* turn is the
    /// one that matters — index 1 here, not 0.
    #[test]
    fn leading_system_turns_are_skipped_when_finding_the_first_turn() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: system, content: \"be terse\"}}\n      \
             - {{role: assistant, content: \"hi\"}}\n      \
             - {{role: user, content: \"q\"}}\n"
        ));
        assert!(report.warnings().any(|i| i.path == "tests[0].history[1]"));
    }

    /// A result answering nothing has no legitimate reading — stricter in
    /// substance than the assistant case, but kept a warning for consistency
    /// with the documented "ordering is the provider's contract" stance.
    #[test]
    fn a_tool_first_history_warns() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: tool, content: \"{{}}\"}}\n"
        ));
        let hit = report
            .warnings()
            .find(|i| i.path == "tests[0].history[0]")
            .unwrap_or_else(|| panic!("expected a warning, got {:?}", report.issues()));
        assert!(hit.message.contains("answers a call no earlier turn made"));
        assert!(!report.has_errors());
    }

    #[test]
    fn a_user_first_history_is_never_flagged() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: user, content: \"hi\"}}\n      \
             - {{role: assistant, content: \"hello\"}}\n"
        ));
        assert!(report.is_clean(), "{:?}", report.issues());
    }

    #[test]
    fn a_blank_history_turn_warns_at_its_index() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: user, content: \"hi\"}}\n      \
             - {{role: assistant, content: \"   \"}}\n"
        ));
        let hit = report
            .warnings()
            .find(|i| i.path == "tests[0].history[1]")
            .unwrap_or_else(|| panic!("expected a blank-content warning: {:?}", report.issues()));
        assert!(hit.message.contains("empty"));
    }

    /// The accepted scope extension: the same typo in a prompt turn breaks
    /// every case rather than one.
    #[test]
    fn a_blank_prompt_turn_warns() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}prompts:\n  - id: s\n    messages:\n      \
             - {{role: system, content: \"be terse\"}}\n      \
             - {{role: user, content: \"\"}}\n"
        ));
        assert!(report
            .warnings()
            .any(|i| i.path == "prompts[0].messages[1]"));
    }

    /// A turn whose whole point is a tool call has no prose and is not blank.
    #[test]
    fn a_tool_call_turn_is_not_blank() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: user, content: \"q\"}}\n      \
             - {{role: assistant, tool_calls: [{{name: lookup}}]}}\n"
        ));
        assert!(report.is_clean(), "{:?}", report.issues());
    }

    /// The documented gap: a `file://` transcript is not read at validate time.
    #[test]
    fn a_file_history_is_not_inspected() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history: file://convo.yaml\n"
        ));
        assert!(report.is_clean(), "{:?}", report.issues());
    }

    /// Emptiness that only appears after rendering is not guessed.
    #[test]
    fn templated_content_is_not_guessed() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: user, content: \"{{{{ note }}}}\"}}\n"
        ));
        assert!(report.is_clean(), "{:?}", report.issues());
    }

    /// Glob sources are unexpanded, so a CSV `__history` column is invisible.
    #[test]
    fn glob_test_sources_are_not_inspected() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - file://cases.csv\n"
        ));
        assert!(report.is_clean(), "{:?}", report.issues());
    }

    /// A turn that cannot mean anything is an **error**, not advice: no
    /// provider accepts it, and render already refuses it.
    #[test]
    fn an_incoherent_turn_is_an_error_not_a_warning() {
        let report = check_yaml(&format!(
            "version: 1\n{PROVIDER}tests:\n  - id: t\n    history:\n      \
             - {{role: user, content: \"hi\"}}\n      - {{role: user}}\n"
        ));
        let hit = report
            .errors()
            .find(|i| i.path == "tests[0].history[1]")
            .unwrap_or_else(|| panic!("expected an error, got {:?}", report.issues()));
        assert_eq!(hit.severity, Severity::Error);
        assert!(report.has_errors());
    }

    /// Structural problems did not soften into advice.
    #[test]
    fn existing_structural_problems_stay_errors() {
        let report = check_yaml("version: 1\nproviders: []\n");
        assert!(report.has_errors());
        assert!(report
            .errors()
            .any(|i| i.severity == Severity::Error && i.path == "providers"));
    }
}
