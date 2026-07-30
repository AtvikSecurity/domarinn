//! What `grader.include_tool_calls` and a tool-calling cell do to the cache.
//!
//! The other half of `grader_request_cache.rs`, split off at the repo's
//! 1000-line source cap and sharing its fixtures through
//! `grader_request_cache/shared.rs`. That file proves the mechanism; this one
//! proves the two things the tool-calls feature adds to it:
//!
//! - **Re-keying.** Showing the judge — or an `exec` child — something it was
//!   not shown before is a different question, so it must miss a warm entry
//!   written for the old one. The request body *is* the key, so this is a
//!   consequence rather than a rule, and the tests below observe it as one:
//!   they count calls, and they compare the two request documents member by
//!   member so the miss is provably caused by the tool calls and nothing else.
//! - **Adoption refusal.** The frozen ≤0.4.x key space has no way to say
//!   "graded with tool calls in view" — a 0.4 judge was never shown them. So a
//!   seeded ≤0.4.x verdict that a flag-off run adopts happily must be left
//!   alone, unprobed, the moment the calls are in play.
//!
//! Every claim here is paired with its control in the same test: the flag-off
//! or tool-less arm runs first against the same cache, so an assertion can only
//! pass because the tool calls changed something. Without that pairing "the
//! judge was called again" is satisfied by any cache that simply never hits.

use std::path::Path;

use domarinn_core::result::CaseStatus;
use domarinn_core::runner::RunOptions;
use domarinn_core::types::Output;
use serde_json::{json, Value as Json};

#[path = "grader_request_cache/shared.rs"]
mod shared;
use shared::*;

// ── llm-rubric ───────────────────────────────────────────────────────────────

/// Turning the flag on re-asks the judge; turning nothing on replays.
///
/// The SUT here calls no tools at all, which is the sharper case: the opted-in
/// prompt carries an *empty* `TOOL CALLS` section, so the only thing that
/// changed is the framing. If that did not re-key, a suite could flip the flag
/// and keep serving verdicts reached without it — the section would be a
/// documentation change rather than a question change.
#[tokio::test]
async fn opting_into_tool_calls_re_keys_the_judge_entry() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let cache = MemCache::default();
    let off = rubric_suite_over(&server.uri(), None, SUT_DECLINES, false);
    let on = rubric_suite_over(&server.uri(), None, SUT_DECLINES, true);

    let cold = run_suite(&off, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(cold.cases[0].status, CaseStatus::Pass);
    assert_eq!(judge_calls(&server).await, 1, "a cold run pays the judge");

    let warm = run_suite(&off, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        1,
        "control: an untouched suite replays, so a later call means the key moved"
    );
    assert!(warm.cases[0].asserts[0].cached);

    let opted_in = run_suite(&on, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        2,
        "a judge shown the tool calls is being asked a different question"
    );
    assert!(!opted_in.cases[0].asserts[0].cached);

    // …and the new question is an ordinary cached one, not a permanent miss.
    let warm_again = run_suite(&on, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 2);
    assert!(warm_again.cases[0].asserts[0].cached);

    // Flipping back finds the entry the first two runs wrote: the flag-off
    // request body never moved, which is the whole promise of an opt-in.
    let back = run_suite(&off, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        2,
        "the flag-off question is still answered by the entry it always was"
    );
    assert!(back.cases[0].asserts[0].cached);
}

/// The opted-in prompt is the old prompt with a section appended, and the
/// section names the tool and its arguments but never the vendor's call id.
///
/// Asserted as `starts_with` rather than as two substring checks: it is the
/// claim that nothing *before* the section moved, which is what makes the
/// flag-off key stable for every suite that never sets it. The call id is
/// excluded because it is a fresh random token per live response — carrying it
/// would give the same decision a different key every time the model made it.
#[tokio::test]
async fn the_judge_sees_the_tool_calls_only_when_the_suite_opts_in() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let cache = MemCache::default();
    let off = rubric_suite_over(&server.uri(), None, SUT_DECLINES_AFTER_A_CALL, false);
    let on = rubric_suite_over(&server.uri(), None, SUT_DECLINES_AFTER_A_CALL, true);

    run_suite(&off, Path::new("."), &cache, &RunOptions::default()).await;
    run_suite(&on, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(judge_calls(&server).await, 2);

    let asked: Vec<String> = cache
        .all(|e| e.verdict.is_none() && e.raw.is_some())
        .into_iter()
        .map(|e| {
            e.request.expect("a judge entry records its request")["body"]["messages"][0]["content"]
                .as_str()
                .expect("the user message is a string")
                .to_string()
        })
        .collect();
    assert_eq!(asked.len(), 2, "two questions, two entries: {asked:?}");
    let (shown, hidden): (Vec<String>, Vec<String>) =
        asked.into_iter().partition(|u| u.contains("TOOL CALLS"));
    assert_eq!(shown.len(), 1, "exactly one prompt carries the section");
    assert_eq!(hidden.len(), 1);
    let (shown, hidden) = (&shown[0], &hidden[0]);

    // The flag-off prompt is unchanged by a cell that happened to call a tool.
    assert!(hidden.contains("declines the task"), "{hidden}");
    assert!(hidden.contains("I cannot help"), "{hidden}");
    assert!(!hidden.contains("get_weather"), "{hidden}");
    assert!(!hidden.contains("Oslo"), "{hidden}");

    // …and the flag-on prompt is that prompt, plus a section.
    assert!(
        shown.starts_with(hidden),
        "the section is appended, not woven in:\n{shown}"
    );
    assert!(shown.contains("get_weather"), "{shown}");
    assert!(shown.contains("Oslo"), "{shown}");
    assert!(
        !shown.contains("toolu_01ABCDEF"),
        "the vendor call id must never reach the key: {shown}"
    );
}

/// A ≤0.4.x verdict the flag-off run adopts is left untouched — and unprobed —
/// once the judge is being shown the calls.
///
/// The first run is the control and the reason the seeded key is trustworthy:
/// it proves the entry is exactly the one this suite would adopt. The second
/// run differs from it in one setting, and must pay the judge instead.
#[tokio::test]
async fn a_legacy_verdict_is_not_adopted_when_the_judge_is_shown_tool_calls() {
    let server = always_passes().await;
    set_env("DOMARINN_REQCACHE_KEY", "sk-test").await;
    let off = rubric_suite_over(&server.uri(), None, SUT_DECLINES_AFTER_A_CALL, false);
    let on = rubric_suite_over(&server.uri(), None, SUT_DECLINES_AFTER_A_CALL, true);
    let output = Output::Text("I cannot help".into());
    let legacy = legacy_rubric_key(
        &output,
        domarinn_core::load_str(&off)
            .unwrap()
            .grader
            .as_ref()
            .unwrap(),
    );
    // The frozen key space cannot see the flag, so both suites would look up the
    // *same* entry. That is precisely why refusing to adopt is the only honest
    // option: there is no key the old store could have used to say "this verdict
    // was reached without seeing the calls".
    assert_eq!(
        legacy_rubric_key(
            &output,
            domarinn_core::load_str(&on)
                .unwrap()
                .grader
                .as_ref()
                .unwrap(),
        ),
        legacy,
        "the ≤0.4.x fingerprint is blind to include_tool_calls"
    );

    let cache = MemCache::default();
    cache.seed(&legacy, verdict_entry("verdict from 0.4"));

    let adopted = run_suite(&off, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        0,
        "control: flag off, this exact entry is adopted"
    );
    assert!(adopted.cases[0].asserts[0].cached);
    assert_eq!(adopted.cases[0].asserts[0].reason, "verdict from 0.4");

    cache.forget_gets();
    let regraded = run_suite(&on, Path::new("."), &cache, &RunOptions::default()).await;
    assert_eq!(
        judge_calls(&server).await,
        1,
        "a 0.4 judge never saw the calls, so its verdict does not answer this"
    );
    assert!(!regraded.cases[0].asserts[0].cached);
    assert_eq!(regraded.cases[0].asserts[0].reason, "live verdict");
    assert_eq!(
        cache.asked_for(&legacy),
        0,
        "and the old key is not even probed"
    );
}

// ── exec asserts ─────────────────────────────────────────────────────────────

/// A cell whose model called a tool re-keys its `exec` assert, and the stdin
/// document says why.
///
/// The two SUTs print the byte-identical `output`, so the assert child is
/// grading the same text either way; the entire difference between the two
/// requests is the `tool_calls` member, which the test removes and then compares
/// the remainder for equality. That is the strong form of "tool-less cells are
/// byte-identical": not merely that the tool-less document lacks the member, but
/// that adding it is the only edit.
#[tokio::test]
async fn an_exec_assert_re_keys_when_the_cell_called_a_tool() {
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let cache = MemCache::default();
    let plain = exec_assert_suite_over(&judge, None, SUT_SAME);
    let calling = exec_assert_suite_over(&judge, None, SUT_SAME_AFTER_A_CALL);

    let cold = run_suite(&plain, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(cold.cases[0].status, CaseStatus::Pass);
    assert_eq!(calls(&counter), 1, "the child is asked once");

    let warm = run_suite(&plain, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        1,
        "control: a tool-less rerun replays, so a later spawn means the key moved"
    );
    assert!(warm.cases[0].asserts[0].cached);

    let called = run_suite(&calling, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        2,
        "a child handed the calls is being asked a different question"
    );
    assert!(!called.cases[0].asserts[0].cached);
    assert_eq!(called.cases[0].status, CaseStatus::Pass);

    let warm_again = run_suite(&calling, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(calls(&counter), 2, "…and it caches like any other");
    assert!(warm_again.cases[0].asserts[0].cached);

    // The two stored stdin documents, and the single member between them.
    let stdin: Vec<Json> = cache
        .all(is_exec_assert_entry)
        .into_iter()
        .map(|e| e.request.expect("checked by the predicate")["stdin"].clone())
        .collect();
    assert_eq!(stdin.len(), 2, "two questions, two entries: {stdin:?}");
    let (mut with, without): (Vec<Json>, Vec<Json>) = stdin
        .into_iter()
        .partition(|s| s.get("tool_calls").is_some());
    assert_eq!(with.len(), 1, "exactly one request carries the calls");
    assert_eq!(without.len(), 1);
    assert!(
        without[0].get("tool_calls").is_none(),
        "a tool-less cell writes no tool_calls member at all: {}",
        without[0]
    );

    let calls_sent = with[0]
        .as_object_mut()
        .expect("the stdin document is an object")
        .remove("tool_calls")
        .expect("checked by the partition");
    assert_eq!(
        with[0], without[0],
        "tool_calls is the only difference, so it is the only reason the key moved"
    );
    assert_eq!(calls_sent[0]["name"], json!("lookup_capital"));
    assert_eq!(calls_sent[0]["arguments"]["country"], json!("France"));
    // Unlike the judge prompt, the child *is* told the vendor id: it is the only
    // way a multi-call response stays attributable, and a child may want it. It
    // is therefore in the key — harmless for a deterministic provider, and worth
    // knowing about for one that mints a fresh id per call.
    assert_eq!(calls_sent[0]["id"], json!("toolu_01ABCDEF"));
}

/// The `exec` half of adoption refusal: the same seeded ≤0.4.x verdict, adopted
/// for the tool-less cell and refused for the tool-calling one.
///
/// [`legacy_exec_key`] does not depend on the SUT at all — it is derived from
/// the assert, the render context and the graded output, all of which are
/// identical here. So the single seeded entry really is the one both runs would
/// look up, and the second run's spawn is the refusal rather than a miss.
#[tokio::test]
async fn a_legacy_exec_verdict_is_not_adopted_for_a_tool_calling_cell() {
    // Held across derivation *and* both runs: the key covers the environment.
    let _env = ENV_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (judge, counter) = counting_judge(dir.path());
    let plain = exec_assert_suite_over(&judge, None, SUT_SAME);
    let calling = exec_assert_suite_over(&judge, None, SUT_SAME_AFTER_A_CALL);
    let legacy = legacy_exec_key(&judge, dir.path(), &Output::Text("same".into()));

    let cache = MemCache::default();
    cache.seed(&legacy, exec_verdict_entry("child said so in 0.4"));

    let adopted = run_suite(&plain, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        0,
        "control: tool-less, this exact entry is adopted"
    );
    assert!(adopted.cases[0].asserts[0].cached);
    assert_eq!(adopted.cases[0].asserts[0].reason, "child said so in 0.4");

    cache.forget_gets();
    let live = run_suite(&calling, dir.path(), &cache, &RunOptions::default()).await;
    assert_eq!(
        calls(&counter),
        1,
        "a 0.4 child was handed a request without the calls, so its verdict is stale"
    );
    assert!(!live.cases[0].asserts[0].cached);
    assert_eq!(live.cases[0].asserts[0].reason, "child says ok");
    assert_eq!(live.cases[0].status, CaseStatus::Pass);
    assert_eq!(
        cache.asked_for(&legacy),
        0,
        "and the old key is not even probed"
    );
}
