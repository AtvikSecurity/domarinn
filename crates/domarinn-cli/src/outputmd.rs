//! The markdown run summary: the headline metrics table and the failing-case
//! table.
//!
//! Split out of [`crate::output`], which keeps the terminal/JSON/JSONL/JUnit
//! formats and the shared humanizers this module borrows. The seam is the
//! audience rather than the mechanism: these two tables are what a PR comment
//! and `ci-summary` show, and they are the only renderers with callers besides
//! [`crate::output::emit`] — `cisummary` composes them a piece at a time, which
//! is why the headline and the failure table are separately public.

use domarinn_core::result::{AssertStatus, CaseResult, CaseStatus, RunResult, RunSummary};

use crate::output::{
    display_name, duration_secs, empty_total, graded_fallback_cases, humanize_count,
    humanize_duration, truncate,
};

/// Failure rows the markdown summary will print before collapsing the rest into
/// a "…and N more" line. A PR comment is a poor place to read 300 failures, and
/// GitHub truncates a long one anyway — the run URL and the JUnit artifact are
/// where the full list lives.
const MD_FAILURE_ROWS: usize = 10;

/// A markdown run summary: the reusable core of [`crate::diffcmd::write_summary_md`],
/// also served by `run --format md` / `view --format md`. Never colored.
///
/// A headline metrics table, then the failing cases. Metric rows that carry no
/// information (no cache was consulted, no tokens were counted, nothing was
/// retried) are omitted rather than printed as zero, so what remains is all
/// signal — the same rule `output::stats_line` applies to the terminal footer.
pub fn render_run_md(run: &RunResult) -> String {
    let mut out = render_run_md_headline(run);
    out.push_str(&render_errors_md(run));
    out.push_str(&render_failures_md(run));
    out
}

/// [`render_run_md`] without the failure table.
///
/// For callers that follow the headline with a baseline comparison: that
/// comparison tables the newly-failing cases itself, so including both prints
/// most of the same rows twice.
pub fn render_run_md_headline(run: &RunResult) -> String {
    let s = &run.summary;
    // Re-derived from the cases rather than read off `s.fallback_cases`, which
    // documents written before 0.10.1 filled in skip-inclusively. See
    // [`graded_fallback_cases`].
    let fallback = graded_fallback_cases(&run.cases);
    let mut out = match run.suite.as_deref() {
        Some(suite) => format!("### domarinn run — {}\n\n", md_cell(suite)),
        None => "### domarinn run\n\n".to_string(),
    };

    // The verdict leads as its own bold line — the one thing every reader
    // wants first — and the table below carries only the supporting metrics.
    out.push_str(&format!("{}\n\n", result_line(s, fallback)));
    out.push_str("| Metric | Value |\n|---|---|\n");

    let rate = domarinn_core::stats::wilson(s.passed, s.total, domarinn_core::stats::Z_95);
    out.push_str(&format!(
        "| Pass rate | {:.1}% (95% CI {:.1}–{:.1}%, n={}) |\n",
        rate.rate * 100.0,
        rate.lower * 100.0,
        rate.upper * 100.0,
        rate.total
    ));

    // Placed against the pass rate for the same reason as the terminal
    // segment, and broken down by reason because "empty" is a family: a
    // refusal, a truncation and a tool-only reply each call for a different
    // fix, and one merged total would send a reader looking for the wrong one.
    // Reasons print in `BTreeMap` order, so the row is stable across runs
    // rather than reordering with whatever the providers happened to return.
    let empty = empty_total(s);
    if empty > 0 {
        let by_reason: Vec<String> = s
            .empty_counts
            .iter()
            .map(|(reason, count)| format!("{} × {count}", md_cell(reason)))
            .collect();
        out.push_str(&format!("| Empty | {empty} ({}) |\n", by_reason.join(", ")));
    }

    // Beside Empty, and broken down for the same reason: "errored" is a family
    // too. A judge that returned malformed JSON, a provider that timed out and
    // a suite naming a grader that does not exist are three different people's
    // problems, and the bare count sends a reader to none of them. This row is
    // what a PR comment previously lacked entirely — a run whose baseline
    // resolved showed "2 errored" and no other trace of what broke.
    if s.errored > 0 {
        let by_class = crate::errorstats::error_class_summary(&run.cases);
        if by_class.is_empty() {
            out.push_str(&format!("| Errors | {} |\n", s.errored));
        } else {
            out.push_str(&format!(
                "| Errors | {} ({}) |\n",
                s.errored,
                md_cell(&by_class)
            ));
        }
    }

    // `cache_misses` counts every case not served from cache, not lookups the
    // cache actually performed — under `--no-cache` it equals the case count.
    // So the honest denominator is "cases", and the row is phrased that way
    // rather than as a hit *rate*, which would imply a lookup that never
    // happened. The sum still gates the row so runs stored before these fields
    // existed print nothing instead of a false 0%.
    let counted = s.cache_hits + s.cache_misses;
    if counted > 0 {
        out.push_str(&format!(
            "| Cache | {}/{} cases from cache ({:.1}%) |\n",
            s.cache_hits,
            counted,
            s.cache_hits as f64 / counted as f64 * 100.0,
        ));
    }
    if s.prompt_tokens > 0 || s.completion_tokens > 0 {
        out.push_str(&format!(
            "| Tokens | {} in / {} out |\n",
            humanize_count(s.prompt_tokens),
            humanize_count(s.completion_tokens),
        ));
    }
    if s.cache_read_tokens > 0 || s.cache_write_tokens > 0 {
        out.push_str(&format!(
            "| Cache tokens | {} read / {} written |\n",
            humanize_count(s.cache_read_tokens),
            humanize_count(s.cache_write_tokens),
        ));
    }
    if let Some(cost) = s.cost_usd.filter(|c| *c > 0.0) {
        out.push_str(&format!("| Cost | ${cost:.4} |\n"));
    }
    if let Some(saved) = s.cache_savings_usd.filter(|c| *c > 0.0) {
        out.push_str(&format!("| Saved by cache | ${saved:.4} |\n"));
    }
    if let Some(cost) = s.grader_cost_usd.filter(|c| *c > 0.0) {
        out.push_str(&format!("| Grading cost | ${cost:.4} |\n"));
    }
    if s.retried_cases > 0 {
        out.push_str(&format!("| Retries | {} cases |\n", s.retried_cases));
    }
    // Denominated in *graded* cases, matching what the numerator counts: a
    // skipped case graded nothing, so including it would shrink the fraction and
    // read as "the fallback mattered less than it did". Both sides of the
    // fraction are therefore skip-exclusive — which is exactly why the numerator
    // cannot come from `s.fallback_cases` on an old document.
    if fallback > 0 {
        out.push_str(&format!(
            "| Answered by fallback | {} of {} cases |\n",
            fallback,
            s.total.saturating_sub(s.skipped),
        ));
    }
    out.push_str(&format!(
        "| Duration | {} |\n",
        humanize_duration(duration_secs(run))
    ));
    out
}

/// `**Pass** — 12 passed`, or `**Fail**` plus every nonzero failure bucket.
///
/// Words, not glyphs: this line is quoted in PR comments, job summaries and
/// notification emails, where an emoji verdict reads as noise and renders
/// inconsistently. Bold is the whole emphasis budget.
///
/// `fallback` is the *graded* fallback count from
/// [`crate::output::graded_fallback_cases`] — passed in rather than read off
/// `s` so this line and the metrics row can never disagree, and so neither
/// repeats the pre-0.10.1 skip-inclusive number.
fn result_line(s: &RunSummary, fallback: u64) -> String {
    let mut parts = vec![format!("{} passed", s.passed)];
    if s.failed > 0 {
        parts.push(format!("{} failed", s.failed));
    }
    if s.errored > 0 {
        parts.push(format!("{} errored", s.errored));
    }
    if s.skipped > 0 {
        parts.push(format!("{} skipped", s.skipped));
    }
    if s.xfailed > 0 {
        parts.push(format!("{} xfailed", s.xfailed));
    }
    if s.xpassed > 0 {
        parts.push(format!("{} xpassed", s.xpassed));
    }
    // An xpass is a gate failure (strict expect_fail); an xfail is not.
    let verdict = if s.failed > 0 || s.errored > 0 || s.xpassed > 0 {
        "Fail"
    } else {
        "Pass"
    };
    let mut line = format!("**{verdict}** — {}", parts.join(", "));
    // Qualifies the verdict rather than joining the bucket list: these cases are
    // already counted as passed/failed above, and a green result somebody else's
    // model produced is a different claim from a green result the configured one
    // produced.
    if fallback > 0 {
        line.push_str(&format!(" ({fallback} via fallback)"));
    }
    line
}

/// The failing cases as a table, capped at [`MD_FAILURE_ROWS`]. Empty when the
/// run is clean.
///
/// Errored cases are **not** here — [`render_errors_md`] tables them instead,
/// with the class column that says whose problem each one is. They used to
/// share this table, where an error's only column was 80 characters of prose
/// and a `0.00` score that read as a judgement the grader never made.
pub fn render_failures_md(run: &RunResult) -> String {
    let failures: Vec<&CaseResult> = run
        .cases
        .iter()
        .filter(|c| crate::output::is_gate_failing(c.status) && c.status != CaseStatus::Error)
        .collect();
    if failures.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n#### Failing cases\n\n| Test | Provider | Score | Reason |\n|---|---|---|---|\n",
    );
    for case in failures.iter().take(MD_FAILURE_ROWS) {
        out.push_str(&format!(
            "| {} | {} | {:.2} | {} |\n",
            md_cell(&display_name(case)),
            md_cell(&case.cell.provider_id),
            case.score,
            md_cell(&truncate(&failure_reason(case), 80)),
        ));
    }
    if let Some(hidden) = failures
        .len()
        .checked_sub(MD_FAILURE_ROWS)
        .filter(|n| *n > 0)
    {
        out.push_str(&format!("\n_…and {hidden} more._\n"));
    }
    out
}

/// The errored cases as a table, capped at [`MD_FAILURE_ROWS`]. Empty when
/// nothing errored.
///
/// Its own section rather than rows in the failure table, because an error is
/// not a judgement: there is no meaningful score, and the column that matters
/// is *which kind* of failure it was. `ci-summary` renders this whether or not
/// a baseline resolved — the bug that prompted it was a PR comment that showed
/// a clean "no regressions" diff and, because a baseline *had* resolved,
/// suppressed every trace of the two cases whose grader had fallen over.
pub fn render_errors_md(run: &RunResult) -> String {
    let errored: Vec<&CaseResult> = run
        .cases
        .iter()
        .filter(|c| c.status == CaseStatus::Error)
        .collect();
    if errored.is_empty() {
        return String::new();
    }

    let mut out = String::from(
        "\n#### Errored cases\n\n_These graded nothing — their scores are not evidence \
         either way._\n\n| Test | Provider | Class | Error |\n|---|---|---|---|\n",
    );
    for case in errored.iter().take(MD_FAILURE_ROWS) {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            md_cell(&display_name(case)),
            md_cell(&case.cell.provider_id),
            md_cell(
                case.error_class
                    .as_ref()
                    .map(|k| k.as_str())
                    .unwrap_or(crate::errorstats::UNKNOWN_CLASS)
            ),
            md_cell(&truncate(&error_reason(case), 160)),
        ));
    }
    if let Some(hidden) = errored
        .len()
        .checked_sub(MD_FAILURE_ROWS)
        .filter(|n| *n > 0)
    {
        out.push_str(&format!("\n_…and {hidden} more._\n"));
    }
    out
}

/// What stopped an errored case.
///
/// Prefers the case-level `error` — the verbatim prose the runner recorded —
/// and falls back to the first errored assertion, which is where a deferred
/// grader failure lands.
fn error_reason(case: &CaseResult) -> String {
    if let Some(e) = case.error.as_ref().filter(|e| !e.is_empty()) {
        return e.clone();
    }
    if let Some(a) = case
        .asserts
        .iter()
        .find(|a| a.status == AssertStatus::Error)
    {
        if a.reason.is_empty() {
            return a.kind.as_str().to_string();
        }
        return format!("{}: {}", a.kind.as_str(), a.reason);
    }
    "—".to_string()
}

/// Why a case failed: its first failing assertion, or the error that stopped it
/// before any assertion ran.
fn failure_reason(case: &CaseResult) -> String {
    if case.status == CaseStatus::XPass {
        return match &case.expect_fail_reason {
            Some(reason) => format!("expected to fail, but passed ({reason})"),
            None => "expected to fail, but passed".to_string(),
        };
    }
    if let Some(a) = case
        .asserts
        .iter()
        .find(|a| matches!(a.status, AssertStatus::Fail | AssertStatus::Error))
    {
        if a.reason.is_empty() {
            return a.kind.as_str().to_string();
        }
        return format!("{}: {}", a.kind.as_str(), a.reason);
    }
    case.error.clone().unwrap_or_else(|| "—".to_string())
}

/// Make `s` safe to drop into a markdown table cell.
///
/// An unescaped `|` in an assertion reason (or a test name) splits the row into
/// extra columns and shreds the table for every row below it; a newline ends the
/// row outright. Both are entirely reachable — `llm-rubric` reasons quote model
/// output verbatim.
pub(crate) fn md_cell(s: &str) -> String {
    s.replace('|', r"\|")
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{old_skip_inclusive_run, sample_run};

    /// The ci-summary body is [`render_run_md_headline`] plus links, so the row
    /// added there is what a PR comment shows — asserted through `cisummary`
    /// rather than the renderer to prove the composition really carries it.
    /// The markdown verdict includes the expected-failure buckets when
    /// nonzero, goes red on an xpass, and explains each xpass row.
    #[test]
    fn result_line_and_failures_cover_expected_failures() {
        let run = sample_run();
        let md = render_run_md(&run);
        assert!(md.contains("1 xfailed"), "{md}");
        assert!(md.contains("1 xpassed"), "{md}");
        assert!(
            md.contains("**Fail**"),
            "an xpass is a failed verdict: {md}"
        );

        let failures = render_failures_md(&run);
        assert!(failures.contains("fixed"), "the xpass row: {failures}");
        assert!(
            failures.contains("expected to fail, but passed"),
            "{failures}"
        );
        assert!(
            !failures.contains("known-bad"),
            "xfail rows are expected, not failures: {failures}"
        );

        // A run whose only red is xfail (no xpass, no fail) is green.
        let mut calm = sample_run();
        calm.cases
            .retain(|c| matches!(c.status, CaseStatus::Pass | CaseStatus::XFail));
        calm.summary.failed = 0;
        calm.summary.xpassed = 0;
        calm.summary.total = 2;
        let md = render_run_md(&calm);
        assert!(md.contains("**Pass**"), "{md}");
    }

    #[test]
    fn ci_summary_notes_empty_cases() {
        let mut run = sample_run();
        run.summary.empty_counts = std::collections::BTreeMap::from([("refusal".to_string(), 2)]);
        let md = crate::cisummary::render(&run, None);
        assert!(md.contains("| Empty | 2 (refusal × 2) |"), "got:\n{md}");
        // Exactly once: the headline row is the only place it is stated.
        assert_eq!(md.matches("| Empty |").count(), 1, "got:\n{md}");

        // A run with nothing empty omits the row entirely.
        let clean = sample_run();
        assert!(!crate::cisummary::render(&clean, None).contains("| Empty |"));
    }

    /// Every reason gets its own term, in `BTreeMap` order — collapsing them
    /// into a bare total would hide that "empty" covers refusals, truncations
    /// and tool-only replies, which call for different fixes.
    #[test]
    fn render_run_md_lists_every_empty_reason_in_order() {
        let mut run = sample_run();
        run.summary.empty_counts = std::collections::BTreeMap::from([
            ("truncated".to_string(), 1),
            ("refusal".to_string(), 3),
        ]);
        assert!(render_run_md(&run).contains("| Empty | 4 (refusal × 3, truncated × 1) |"));
    }

    #[test]
    fn render_run_md_has_header_and_wilson_interval() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 4,
            passed: 3,
            failed: 1,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(md.starts_with("### domarinn run — s\n"));
        assert!(md.contains("| Metric | Value |"));
        assert!(md.contains("**Fail** — 3 passed, 1 failed"));
        assert!(md.contains("| Pass rate | 75.0% (95% CI "));
        assert!(md.contains(", n=4) |"));
        // Wall time comes from started_at..finished_at, not the case latencies.
        assert!(md.contains("| Duration | 12.3s |"));
    }

    #[test]
    fn render_run_md_marks_a_clean_run_with_a_check() {
        let mut run = sample_run();
        run.cases.retain(|c| c.status == CaseStatus::Pass);
        run.summary = RunSummary {
            total: 4,
            passed: 4,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(md.contains("**Pass** — 4 passed"));
        assert!(!md.contains("Failing cases"));
    }

    #[test]
    fn render_run_md_omits_metric_rows_that_have_no_data() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 2,
            passed: 1,
            failed: 1,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(!md.contains("| Cache |"));
        assert!(!md.contains("| Tokens |"));
        assert!(!md.contains("| Cost |"));
        assert!(!md.contains("| Retries |"));
    }

    #[test]
    fn render_run_md_reports_the_share_of_cases_served_from_cache() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 12,
            passed: 12,
            cache_hits: 8,
            cache_misses: 4,
            prompt_tokens: 4_100,
            completion_tokens: 892,
            cost_usd: Some(0.0231),
            retried_cases: 2,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(md.contains("| Cache | 8/12 cases from cache (66.7%) |"));
        assert!(md.contains("| Tokens | 4.1k in / 892 out |"));
        assert!(md.contains("| Cost | $0.0231 |"));
        assert!(md.contains("| Retries | 2 cases |"));
    }

    /// A fully-cached run must still render the row — `0 misses` is exactly the
    /// result a CI reader wants confirmed, and omitting it reads as "no cache".
    #[test]
    fn render_run_md_reports_a_fully_cached_run() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 5,
            passed: 5,
            cache_hits: 5,
            cache_misses: 0,
            ..Default::default()
        };
        assert!(render_run_md(&run).contains("| Cache | 5/5 cases from cache (100.0%) |"));
    }

    /// A run stored before the cache counters existed deserializes both as 0.
    /// Printing `0/0` there would invent a statistic the run never recorded.
    #[test]
    fn render_run_md_omits_the_cache_row_when_nothing_was_counted() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 2,
            passed: 2,
            ..Default::default()
        };
        assert!(!render_run_md(&run).contains("| Cache |"));
    }

    /// A run some other provider answered for says so in both places a reader
    /// looks: the verdict line and its own metrics row. The denominator is the
    /// *graded* count, not `total` — a skipped case graded nothing.
    #[test]
    fn render_run_md_reports_cases_answered_by_a_fallback() {
        let mut run = sample_run();
        // The count comes from the cases, not the summary field.
        for case in &mut run.cases {
            case.answered_by_provider_id = Some("backup".into());
        }
        run.summary = RunSummary {
            total: 6,
            passed: 5,
            skipped: 1,
            fallback_cases: 2,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(
            md.contains("**Pass** — 5 passed, 1 skipped (4 via fallback)"),
            "got:\n{md}"
        );
        assert!(
            md.contains("| Answered by fallback | 4 of 5 cases |"),
            "got:\n{md}"
        );
    }

    /// A run where nothing fell back emits neither the row nor the segment, so
    /// every summary written before the feature existed renders byte-identically.
    #[test]
    fn render_run_md_omits_the_fallback_row_when_nothing_fell_back() {
        let mut run = sample_run();
        run.summary = RunSummary {
            total: 2,
            passed: 1,
            failed: 1,
            ..Default::default()
        };
        let md = render_run_md(&run);
        assert!(!md.contains("Answered by fallback"), "got:\n{md}");
        assert!(!md.contains("via fallback"), "got:\n{md}");
    }

    /// A run document stored by CLI ≤ 0.10.0, where `fallback_cases` counted
    /// skipped cases too. Summaries are read back as stored, so pairing that
    /// numerator with the skip-exclusive denominator this renderer uses printed
    /// "2 of 0 cases". Re-deriving from the cases reports the truth — nothing
    /// was graded, so nothing fell back — and the row and segment both vanish.
    #[test]
    fn render_run_md_ignores_a_pre_0_10_1_skip_inclusive_fallback_count() {
        let run = old_skip_inclusive_run();
        let md = render_run_md(&run);
        assert!(!md.contains("Answered by fallback"), "got:\n{md}");
        assert!(!md.contains("via fallback"), "got:\n{md}");
        assert!(md.contains("**Pass** — 0 passed, 4 skipped"), "got:\n{md}");
    }

    #[test]
    fn render_run_md_tables_failing_cases_with_the_first_failing_assert() {
        let md = render_run_md(&sample_run());
        assert!(md.contains("#### Failing cases"));
        assert!(md.contains("| Test | Provider | Score | Reason |"));
        assert!(md.contains("| bad | p | 0.00 | contains: expected \"decline\" |"));
        // Passing cases never appear in the failure table.
        assert!(!md.contains("| ok | p |"));
    }

    #[test]
    fn render_run_md_uses_the_error_text_for_errored_cases() {
        let mut run = errored_run();
        run.cases[1].error_class = Some(domarinn_core::error_class::ErrorClass::new(
            "provider_timeout",
        ));
        let md = render_run_md(&run);
        assert!(
            md.contains("| bad | p | provider_timeout | provider timed out |"),
            "{md}"
        );
    }

    /// A run with one errored case, `summary.errored` kept consistent with it.
    fn errored_run() -> domarinn_core::result::RunResult {
        let mut run = sample_run();
        run.cases[1].status = CaseStatus::Error;
        run.cases[1].asserts.clear();
        run.cases[1].score = 0.0;
        run.cases[1].error = Some("provider timed out".into());
        run.summary.failed = run.summary.failed.saturating_sub(1);
        run.summary.errored = 1;
        run
    }

    /// An error is not a judgement, so it does not belong in the table of
    /// judgements. It used to appear there with a `0.00` score the grader never
    /// assigned, next to cases that genuinely scored zero.
    #[test]
    fn errored_cases_are_tabled_apart_from_failing_ones() {
        let md = render_run_md(&errored_run());
        assert!(md.contains("#### Errored cases"), "{md}");
        let errors_at = md.find("#### Errored cases").unwrap();
        let failing_at = md.find("#### Failing cases");
        // The errored case must not also be listed as a failure.
        if let Some(failing_at) = failing_at {
            assert!(
                !md[failing_at..].contains("| bad |"),
                "an errored case leaked into the failing table: {md}"
            );
            assert!(errors_at < failing_at, "errors should lead: {md}");
        }
    }

    /// The row the PR comment was missing: which *kind* of error, not just how
    /// many.
    #[test]
    fn render_run_md_breaks_errors_down_by_class() {
        let mut run = errored_run();
        run.cases[1].error_class =
            Some(domarinn_core::error_class::ErrorClass::new("grader_failed"));
        assert!(
            render_run_md(&run).contains("| Errors | 1 (grader_failed × 1) |"),
            "{}",
            render_run_md(&run)
        );
    }

    /// A run stored before `error_class` existed still gets a row and a table
    /// cell — bucketed, never blank.
    #[test]
    fn an_errored_case_without_a_class_renders_as_unknown() {
        let md = render_run_md(&errored_run());
        assert!(md.contains("| Errors | 1 (unknown × 1) |"), "{md}");
        assert!(
            md.contains("| bad | p | unknown | provider timed out |"),
            "{md}"
        );
    }

    /// The "no data, no row" rule the metric table applies to every other row.
    #[test]
    fn render_run_md_omits_the_errors_row_when_nothing_errored() {
        let md = render_run_md(&sample_run());
        assert!(!md.contains("| Errors |"), "{md}");
        assert!(!md.contains("#### Errored cases"), "{md}");
    }

    /// A pipe inside an assertion reason would otherwise split the row into
    /// extra columns and shred the whole table.
    #[test]
    fn render_run_md_escapes_pipes_and_newlines_in_failure_cells() {
        let mut run = sample_run();
        run.cases[1].asserts[0].reason = "expected a|b\ngot c".into();
        run.cases[1].name = Some("pipe|name".into());
        let md = render_run_md(&run);
        let row = md
            .lines()
            .find(|l| l.contains("pipe"))
            .expect("failure row present");
        assert!(row.contains(r"pipe\|name"));
        assert!(row.contains(r"expected a\|b got c"));
        // Four columns means five pipes; any unescaped one would add more.
        assert_eq!(row.matches("| ").count(), 4);
    }

    #[test]
    fn render_run_md_caps_the_failing_table_and_says_how_many_are_hidden() {
        let mut run = sample_run();
        let template = run.cases[1].clone();
        run.cases = (0..MD_FAILURE_ROWS + 3)
            .map(|i| {
                let mut c = template.clone();
                c.name = Some(format!("fail-{i}"));
                c
            })
            .collect();
        let md = render_run_md(&run);
        assert_eq!(md.matches("| fail-").count(), MD_FAILURE_ROWS);
        assert!(md.contains("_…and 3 more._"));
    }
}
