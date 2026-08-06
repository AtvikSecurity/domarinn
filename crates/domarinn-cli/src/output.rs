//! Rendering and persisting run results.
//!
//! The terminal table, the machine formats (JSON/JSONL/JUnit) and the
//! humanizers they share. The markdown summary lives in [`crate::outputmd`],
//! which borrows those humanizers; [`emit`] still dispatches to it, so
//! `--format md` is reached the same way as every other format.

use std::io::Write;
use std::path::Path;

use domarinn_core::result::{CaseResult, CaseStatus, RunResult, RunSummary};

use crate::style::Palette;

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
    Jsonl,
    Junit,
    /// A markdown run summary (pass/fail counts + Wilson pass-rate interval).
    Md,
}

/// Persist a run under `<store>/<run_id>/result.json` and update the `latest`
/// pointer.
///
/// The store location comes from [`crate::loadrun`] rather than being spelled
/// out here: writing to a different directory than every reader looks in is a
/// bug that only shows up as "no runs found" much later.
pub fn persist(result: &RunResult) -> std::io::Result<()> {
    let dir = crate::loadrun::runs_dir().join(result.run_id.as_str());
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(result).map_err(std::io::Error::other)?;
    std::fs::write(dir.join("result.json"), json)?;
    std::fs::write(crate::loadrun::latest_pointer(), result.run_id.as_str())?;
    Ok(())
}

/// Emit a run in the requested format to `out` (or stdout).
///
/// `palette` colors the human table; it is forced off for `--out` file output
/// and never reaches the machine formats (json/jsonl/junit), so those stay
/// byte-identical under every color/TTY combination. `failed_only` restricts the
/// rendered cases to failures/errors (the `view --failed` filter) across every
/// format; the table's whole-run footer is unaffected.
pub fn emit(
    format: Format,
    result: &RunResult,
    out: Option<&Path>,
    palette: Palette,
    failed_only: bool,
) -> std::io::Result<()> {
    // Writing to a file forces color off regardless of the flag: files should
    // never carry terminal escapes.
    let palette = if out.is_some() {
        Palette::disabled()
    } else {
        palette
    };
    let text = match format {
        Format::Table => render_table(result, palette, failed_only),
        Format::Json => render_json(result, failed_only)?,
        Format::Jsonl => render_jsonl(result, failed_only)?,
        Format::Junit => render_junit(result, failed_only),
        Format::Md => crate::outputmd::render_run_md(result),
    };
    match out {
        Some(path) => std::fs::write(path, text.as_bytes()),
        None => write_stdout(format, &text, palette),
    }
}

/// Write to stdout, routing the human table through [`anstream::AutoStream`] so
/// Windows consoles render (or, when disabled, strip) the ANSI we emit. The
/// choice is explicit — derived from the resolved palette — so AutoStream's own
/// TTY heuristics never second-guess the precedence we already decided. Machine
/// and markdown formats carry no escapes, so they take the plain locked path and
/// stay byte-identical to a raw write.
fn write_stdout(format: Format, text: &str, palette: Palette) -> std::io::Result<()> {
    if matches!(format, Format::Table) {
        write_colored_stdout(text, palette)
    } else {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
        Ok(())
    }
}

/// Write human (possibly colored) text to stdout through [`anstream::AutoStream`],
/// so the ANSI we emit is rendered or stripped per the resolved `palette` rather
/// than AutoStream's own TTY guess. Shared by the results table and the
/// `view --case` detail view (both carry escapes when color is enabled).
pub(crate) fn write_colored_stdout(text: &str, palette: Palette) -> std::io::Result<()> {
    let choice = if palette.enabled() {
        anstream::ColorChoice::Always
    } else {
        anstream::ColorChoice::Never
    };
    let mut stream = anstream::AutoStream::new(std::io::stdout().lock(), choice);
    stream.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        stream.write_all(b"\n")?;
    }
    stream.flush()
}

pub(crate) fn status_glyph(status: CaseStatus) -> &'static str {
    match status {
        CaseStatus::Pass => "PASS",
        CaseStatus::Fail => "FAIL",
        CaseStatus::Error => "ERR ",
        CaseStatus::Skip => "SKIP",
    }
}

/// The status glyph, colored per status. The 4-byte token is byte-unchanged
/// inside the escapes so `contains("PASS")`/`contains("FAIL")` assertions and
/// downstream scrapers still match.
pub(crate) fn colored_glyph(palette: &Palette, status: CaseStatus) -> String {
    let glyph = status_glyph(status);
    match status {
        CaseStatus::Pass => palette.pass(glyph),
        CaseStatus::Fail => palette.fail(glyph),
        CaseStatus::Error => palette.error(glyph),
        CaseStatus::Skip => palette.skip(glyph),
    }
}

/// Cases restricted to failures/errors when `failed_only`, else all cases.
fn selected_cases(result: &RunResult, failed_only: bool) -> Vec<&CaseResult> {
    result
        .cases
        .iter()
        .filter(|c| !failed_only || matches!(c.status, CaseStatus::Fail | CaseStatus::Error))
        .collect()
}

fn render_table(result: &RunResult, palette: Palette, failed_only: bool) -> String {
    let cases = selected_cases(result, failed_only);
    let mut out = String::new();
    let name_w = cases
        .iter()
        .map(|c| char_width(&display_name(c)))
        .max()
        .unwrap_or(4)
        .clamp(4, 60);

    // The header is bold as one unit; every glyph is exactly 4 bytes, so the
    // status column's `{:<6}` padding is always the glyph plus two spaces —
    // reproduce that as `glyph + "  "` so the ANSI escapes never distort the
    // column width.
    // `name_w` is the longest name, so the column must be padded WIDER than it
    // or the longest row butts straight against the score — the same reason the
    // provider column truncates to 9 and pads to 10.
    let name_col = name_w + 2;
    let header = format!("{:<6}{:<10}{:<name_col$}score", "", "provider", "test");
    out.push_str(&palette.header(&header));
    out.push('\n');
    for case in &cases {
        out.push_str(&format!(
            "{}  {:<10}{:<name_col$}{:.2}\n",
            colored_glyph(&palette, case.status),
            truncate(&case.cell.provider_id, 9),
            truncate(&display_name(case), name_w),
            case.score,
        ));
    }

    // The footer always describes the whole run, even under `--failed`.
    let s = &result.summary;
    let failed = if s.failed > 0 {
        palette.fail(&s.failed.to_string())
    } else {
        s.failed.to_string()
    };
    let errored = if s.errored > 0 {
        palette.error(&s.errored.to_string())
    } else {
        s.errored.to_string()
    };
    out.push_str(&format!(
        "\n{} total: {} passed, {} failed, {} errored in {}\n",
        result.suite.as_deref().unwrap_or("run"),
        s.passed,
        failed,
        errored,
        humanize_duration(duration_secs(result)),
    ));
    out.push_str(&stats_line(s, &result.cases));
    out.push('\n');

    if failed_only {
        out.push_str(&format!(
            "showing {} failed/errored of {} cases\n",
            cases.len(),
            result.cases.len(),
        ));
    }
    out
}

/// The wall-clock duration of a run in seconds, clamped at zero to absorb clock
/// skew between `started_at` and `finished_at`.
pub(crate) fn duration_secs(result: &RunResult) -> f64 {
    let ms = (result.finished_at - result.started_at).num_milliseconds();
    (ms as f64 / 1000.0).max(0.0)
}

/// Humanize a duration: `12.3s` under a minute, `2m 05s` at or above one.
pub(crate) fn humanize_duration(secs: f64) -> String {
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let total = secs.round() as u64;
        format!("{}m {:02}s", total / 60, total % 60)
    }
}

/// Compact a count: `500`, `12.3k`, `1.2M`.
pub(crate) fn humanize_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// pass@1 across repeated trials, or `None` when the run had no repeats.
///
/// Groups cases by their cell minus the repeat index, so each group is one
/// test's trials, and averages `pass_at_k(n, passed, 1)` over the groups.
/// `stats::pass_at_k` has been implemented and unit-tested since it shipped
/// and referenced nowhere; this is its first caller.
fn pass_at_1(cases: &[domarinn_core::result::CaseResult]) -> Option<f64> {
    use std::collections::BTreeMap;
    let mut trials: BTreeMap<(String, Option<String>, String), (u64, u64)> = BTreeMap::new();
    for c in cases {
        let key = (
            c.cell.provider_id.clone(),
            c.cell.prompt_id.clone(),
            c.cell.test_id.clone(),
        );
        let entry = trials.entry(key).or_insert((0, 0));
        entry.0 += 1;
        if c.status == domarinn_core::result::CaseStatus::Pass {
            entry.1 += 1;
        }
    }
    // No repeats means pass@1 is the pass rate, and printing it twice under two
    // names is noise rather than information.
    if trials.values().all(|(n, _)| *n <= 1) {
        return None;
    }
    let total: f64 = trials
        .values()
        .map(|(n, passed)| domarinn_core::stats::pass_at_k(*n, *passed, 1))
        .sum();
    Some(total / trials.len() as f64)
}

/// How many cases came back with nothing gradeable, across every reason.
///
/// Summed from the per-reason tally rather than tracked as its own counter so
/// the total and the breakdown can never disagree. It is *not* the same as
/// `failed + errored`: a refusal can be graded as a pass by a negated assert,
/// and a provider failure is errored without being empty.
pub(crate) fn empty_total(s: &RunSummary) -> u64 {
    s.empty_counts.values().sum()
}

/// How many *graded* cases a fallback link answered, counted from the cases
/// themselves rather than read off [`RunSummary::fallback_cases`].
///
/// The stored counter is only skip-exclusive in documents written by CLI
/// 0.10.1 and later; every earlier one counted skipped cases too. Summaries are
/// read back exactly as stored and never re-derived, so pairing that stored
/// numerator with the graded denominator this crate now renders produced
/// nonsense — a 0.10.0 run with `total: 2, skipped: 2, fallback_cases: 2` read
/// as "2 of 0 cases", and diffing against it invented a fallback-count delta.
///
/// Counting here is right for a document of any vintage: `status` and
/// `answered_by_provider_id` are per-case truth and their meaning never
/// changed. Every renderer with the cases in hand goes through this instead of
/// the summary field.
pub(crate) fn graded_fallback_cases(cases: &[CaseResult]) -> u64 {
    cases
        .iter()
        .filter(|c| c.status != CaseStatus::Skip && c.answered_by_provider_id.is_some())
        .count() as u64
}

/// The stats footer line: Wilson pass-rate interval, then pass@1 and the
/// token/cost/cache segments that are omitted when zero or absent, joined
/// by ` · `.
fn stats_line(s: &RunSummary, run_cases: &[domarinn_core::result::CaseResult]) -> String {
    let mut segments = Vec::new();
    let rate = domarinn_core::stats::wilson(s.passed, s.total, domarinn_core::stats::Z_95);
    segments.push(format!(
        "pass rate {:.1}% (95% CI {:.1}–{:.1}%)",
        rate.rate * 100.0,
        rate.lower * 100.0,
        rate.upper * 100.0,
    ));
    // Rides directly behind the rate it qualifies, ahead of the cost/cache
    // trivia: a suite where most cases came back with nothing gradeable still
    // prints a pass rate, and that number alone reads as a verdict on the model
    // rather than on a run that never got an answer to grade.
    let empty = empty_total(s);
    if empty > 0 {
        segments.push(format!("{empty} empty"));
    }
    if s.prompt_tokens > 0 || s.completion_tokens > 0 {
        segments.push(format!(
            "{} in / {} out tokens",
            humanize_count(s.prompt_tokens),
            humanize_count(s.completion_tokens),
        ));
    }
    // pass@1 over repeated trials. Only meaningful with `--repeat`, and only
    // *different* from the plain pass rate when a case's trials disagreed —
    // which is exactly the judge/model variance `--repeat` exists to measure,
    // and which the pass rate alone averages away.
    if let Some(p1) = pass_at_1(run_cases) {
        segments.push(format!("pass@1 {:.1}%", p1 * 100.0));
    }
    // A breakdown of the `in` figure above, not an addition to it: the cached
    // span is part of the prompt that was sent, billed at a different rate.
    // Reading these two segments as summands would double-count it.
    if s.cache_read_tokens > 0 || s.cache_write_tokens > 0 {
        segments.push(format!(
            "{} of that cache-read / {} cache-write tokens",
            humanize_count(s.cache_read_tokens),
            humanize_count(s.cache_write_tokens),
        ));
    }
    if let Some(cost) = s.cost_usd {
        if cost > 0.0 {
            // The saving rides on the cost segment rather than standing alone:
            // it is only meaningful next to the number it is a fraction of.
            match s.cache_savings_usd.filter(|c| *c > 0.0) {
                Some(saved) => segments.push(format!("${cost:.4} (${saved:.4} saved by cache)")),
                None => segments.push(format!("${cost:.4}")),
            }
        }
    }
    // Its own segment, never added to the line above: a suite judged by a
    // larger model than it tests spends more here than on the run itself, and
    // one merged number would hide exactly that.
    if let Some(cost) = s.grader_cost_usd.filter(|c| *c > 0.0) {
        segments.push(format!("${cost:.4} grading"));
    }
    if s.cache_hits > 0 {
        segments.push(format!("{} cache hits", s.cache_hits));
    }
    // Retries otherwise leave no trace but a longer wall-clock — the run looks
    // clean while having paid for three calls on some cases.
    if s.retried_cases > 0 {
        segments.push(format!("{} retried", s.retried_cases));
    }
    segments.join(" · ")
}

pub(crate) fn display_name(case: &CaseResult) -> String {
    case.name
        .clone()
        .unwrap_or_else(|| case.cell.test_id.clone())
}

/// The number of Unicode scalar values in `s` — a cheap column-width proxy that,
/// unlike `str::len`, matches how [`truncate`] measures and never over-pads a
/// multi-byte name.
fn char_width(s: &str) -> usize {
    s.chars().count()
}

/// Truncate to at most `max` characters, appending `…` when shortened. Operates
/// on `char`s, not bytes, so a multi-byte name can never panic on a non-char
/// boundary (the pre-v2 byte-slice implementation did).
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

/// Render the (optionally filtered) run as pretty JSON. When unfiltered this is
/// byte-identical to `serde_json::to_string_pretty(result)`; under `--failed` it
/// serializes the same document with `cases` narrowed to failures/errors, while
/// `summary` still describes the whole run.
fn render_json(result: &RunResult, failed_only: bool) -> std::io::Result<String> {
    if failed_only {
        let filtered = filtered_run(result);
        serde_json::to_string_pretty(&filtered).map_err(std::io::Error::other)
    } else {
        serde_json::to_string_pretty(result).map_err(std::io::Error::other)
    }
}

fn render_jsonl(result: &RunResult, failed_only: bool) -> std::io::Result<String> {
    let mut out = String::new();
    for case in selected_cases(result, failed_only) {
        out.push_str(&serde_json::to_string(case).map_err(std::io::Error::other)?);
        out.push('\n');
    }
    Ok(out)
}

/// A minimal JUnit XML report (one testcase per result cell). Under `--failed`
/// only failing/errored testcases are emitted; the suite-level counts still
/// describe the whole run.
fn render_junit(result: &RunResult, failed_only: bool) -> String {
    let s = &result.summary;
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(&format!(
        "<testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\">\n",
        xml_escape(result.suite.as_deref().unwrap_or("domarinn")),
        s.total,
        s.failed,
        s.errored,
    ));
    for case in selected_cases(result, failed_only) {
        let name = xml_escape(&display_name(case));
        let classname = xml_escape(&case.cell.provider_id);
        out.push_str(&format!(
            "  <testcase classname=\"{classname}\" name=\"{name}\" time=\"{:.3}\">",
            case.latency_ms as f64 / 1000.0
        ));
        match case.status {
            CaseStatus::Fail => {
                out.push_str(&format!(
                    "<failure message=\"score {:.2}\"></failure>",
                    case.score
                ));
            }
            CaseStatus::Error => {
                let msg = xml_escape(case.error.as_deref().unwrap_or("error"));
                out.push_str(&format!("<error message=\"{msg}\"></error>"));
            }
            CaseStatus::Skip => out.push_str("<skipped></skipped>"),
            CaseStatus::Pass => {}
        }
        out.push_str("</testcase>\n");
    }
    out.push_str("</testsuite>\n");
    out
}

/// Clone `result` keeping only its failing/errored cases; `summary` is left
/// intact so JSON/JSONL under `--failed` still report the whole-run totals.
fn filtered_run(result: &RunResult) -> RunResult {
    let mut filtered = result.clone();
    filtered
        .cases
        .retain(|c| matches!(c.status, CaseStatus::Fail | CaseStatus::Error));
    filtered
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A minimal run with one passing and one failing case, for renderer tests.
///
/// At module scope rather than inside `mod tests` because both this module's
/// tests and [`crate::outputmd`]'s render the same run through different
/// formats; a second copy would let the two drift and quietly stop testing
/// the same document.
#[cfg(test)]
pub(crate) fn sample_run() -> RunResult {
    use domarinn_core::ids::RunId;
    use domarinn_core::result::CellKey;

    let cell = |test: &str| CellKey {
        provider_id: "p".into(),
        prompt_id: None,
        test_id: test.into(),
        repeat: 0,
    };
    // A failing case carries the assertion that failed it — the markdown
    // summary's `why` column reads it, and a case that failed with no
    // recorded assert is not a shape the runner produces.
    let asserts = |status: CaseStatus| match status {
        CaseStatus::Fail => vec![domarinn_core::result::AssertResult {
            kind: domarinn_core::asserts::AssertName::Contains,
            status: domarinn_core::result::AssertStatus::Fail,
            score: 0.0,
            weight: 1.0,
            reason: "expected \"decline\"".into(),
            details: None,
            criteria: None,
            cached: false,
            cost_usd: None,
        }],
        _ => vec![],
    };
    let case = |test: &str, status: CaseStatus, score: f64| CaseResult {
        cache_key: None,
        answered_by_provider_id: None,
        fallback_attempts: Vec::new(),
        tool_calls: Vec::new(),
        case_key: cell(test).case_key(),
        cell: cell(test),
        name: Some(test.into()),
        tags: vec![],
        vars: Default::default(),
        status,
        score,
        output: None,
        prompt: None,
        request: None,
        stop_reason: None,
        model: None,
        raw: None,
        asserts: asserts(status),
        usage: None,
        cost_usd: None,
        latency_ms: 10,
        wall_ms: None,
        reasoning: None,
        empty_reason: None,
        cached: false,
        attempts: 1,
        prompt_digest: None,
        provider_digest: None,
        assert_digest: None,
        error: None,
        error_details: None,
        error_class: None,
    };
    let started = chrono::Utc::now();
    RunResult {
        schema_version: domarinn_core::result::RESULT_SCHEMA_VERSION,
        run_id: RunId::new("testrun"),
        project: None,
        suite: Some("s".into()),
        started_at: started,
        finished_at: started + chrono::Duration::milliseconds(12_340),
        config_digest: "d".into(),
        config_snapshot: serde_json::json!({}),
        git: None,
        ci: None,
        digests: None,
        origin: None,
        share_url: None,
        filters: Default::default(),
        cases: vec![
            case("ok", CaseStatus::Pass, 1.0),
            case("bad", CaseStatus::Fail, 0.0),
        ],
        summary: RunSummary {
            total: 2,
            passed: 1,
            failed: 1,
            ..Default::default()
        },
    }
}

/// [`sample_run`] as a run document stored by CLI ≤ 0.10.0: every case skipped
/// yet answered by a fallback, with `fallback_cases` hand-set to the
/// skip-inclusive total the old CLI wrote. Renderers must read 0 fallback cases
/// from it — nothing was graded — rather than trusting the stored 2.
///
/// Shared like `sample_run` because both [`crate::outputmd`] and
/// [`crate::cisummary`] assert against the same document; two copies could
/// drift and stop covering the same vintage.
#[cfg(test)]
pub(crate) fn old_skip_inclusive_run() -> RunResult {
    let mut run = sample_run();
    for case in &mut run.cases {
        case.status = CaseStatus::Skip;
        case.answered_by_provider_id = Some("backup".into());
    }
    run.summary = RunSummary {
        total: 2,
        skipped: 2,
        fallback_cases: 2,
        ..Default::default()
    };
    run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper counts *graded* fallback answers only: a skipped case graded
    /// nothing, so it cannot have been "answered by a fallback" in the sense
    /// the metric reports, however the stored summary counted it.
    #[test]
    fn graded_fallback_cases_counts_only_non_skipped_answered_cases() {
        let run = old_skip_inclusive_run();
        assert_eq!(
            run.summary.fallback_cases, 2,
            "the stored, old-semantics count"
        );
        assert_eq!(graded_fallback_cases(&run.cases), 0);

        let mut graded = run;
        graded.cases[0].status = CaseStatus::Pass;
        assert_eq!(graded_fallback_cases(&graded.cases), 1);
        // A graded case nobody fell back for is not counted either.
        graded.cases[0].answered_by_provider_id = None;
        assert_eq!(graded_fallback_cases(&graded.cases), 0);
    }

    #[test]
    fn xml_escape_handles_specials() {
        assert_eq!(xml_escape("a<b>&\"c"), "a&lt;b&gt;&amp;&quot;c");
    }

    #[test]
    fn truncate_shortens() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn truncate_is_char_boundary_safe_on_multibyte() {
        // The old byte-slice implementation panicked here: `&"héllo…"[..2]` cuts
        // through the 2-byte `é`. The char-based version keeps `max - 1` chars.
        assert_eq!(truncate("héllo…", 3), "hé…");
        // Never shortens when already within budget, multi-byte or not.
        assert_eq!(truncate("héllo", 5), "héllo");
        // A pure-emoji name (4-byte scalars) is safe too.
        assert_eq!(truncate("🚀🚀🚀", 2), "🚀…");
    }

    #[test]
    fn humanize_duration_sub_minute_and_over() {
        assert_eq!(humanize_duration(12.34), "12.3s");
        assert_eq!(humanize_duration(0.0), "0.0s");
        assert_eq!(humanize_duration(125.0), "2m 05s");
        assert_eq!(humanize_duration(60.0), "1m 00s");
        assert_eq!(humanize_duration(3599.0), "59m 59s");
    }

    #[test]
    fn humanize_count_scales() {
        assert_eq!(humanize_count(500), "500");
        assert_eq!(humanize_count(12_300), "12.3k");
        assert_eq!(humanize_count(4_500), "4.5k");
        assert_eq!(humanize_count(2_500_000), "2.5M");
    }

    #[test]
    fn stats_line_omits_zero_segments() {
        let s = RunSummary {
            total: 6,
            passed: 5,
            failed: 1,
            ..Default::default()
        };
        let line = stats_line(&s, &[]);
        assert!(line.starts_with("pass rate 83.3% (95% CI "));
        // No tokens/cost/cache segments when they are zero/None.
        assert!(!line.contains("tokens"));
        assert!(!line.contains('$'));
        assert!(!line.contains("cache hits"));
        assert!(!line.contains(" · "));
    }

    #[test]
    fn stats_line_includes_present_segments() {
        let s = RunSummary {
            total: 6,
            passed: 5,
            failed: 1,
            prompt_tokens: 12_300,
            completion_tokens: 4_500,
            cost_usd: Some(0.42),
            cache_hits: 3,
            ..Default::default()
        };
        let line = stats_line(&s, &[]);
        assert!(line.contains("12.3k in / 4.5k out tokens"));
        assert!(line.contains("$0.4200"));
        assert!(line.contains("3 cache hits"));
        assert!(line.contains(" · "));
    }

    /// The run-level empty tally has to reach the terminal footer: a suite where
    /// most cases came back with nothing gradeable still prints a pass rate, and
    /// without this segment that number is the only thing a reader sees.
    #[test]
    fn terminal_summary_notes_empty_cases() {
        let s = RunSummary {
            total: 6,
            passed: 4,
            failed: 2,
            empty_counts: std::collections::BTreeMap::from([("refusal".to_string(), 2)]),
            ..Default::default()
        };
        assert!(stats_line(&s, &[]).contains("2 empty"));

        // Nothing came back empty: the segment is absent rather than `0 empty`,
        // the same rule every other optional segment follows.
        let clean = RunSummary {
            total: 6,
            passed: 6,
            ..Default::default()
        };
        assert!(!stats_line(&clean, &[]).contains("empty"));
    }

    #[test]
    fn table_disabled_palette_has_no_escapes_and_keeps_glyphs() {
        let run = sample_run();
        let text = render_table(&run, Palette::disabled(), false);
        assert!(!text.contains('\x1b'));
        assert!(text.contains("PASS"));
        assert!(text.contains("FAIL"));
        assert!(text.contains("s total: 1 passed, 1 failed, 0 errored in 12.3s"));
        assert!(text.contains("pass rate 50.0%"));
    }

    #[test]
    fn table_enabled_palette_emits_escapes_but_glyph_bytes_survive() {
        let run = sample_run();
        let colored = render_table(&run, Palette::for_test(true), false);
        assert!(colored.contains('\x1b'));
        // The glyph tokens remain byte-for-byte findable.
        assert!(colored.contains("PASS"));
        assert!(colored.contains("FAIL"));
    }

    #[test]
    fn failed_only_table_filters_rows_and_notes_the_selection() {
        let run = sample_run();
        let text = render_table(&run, Palette::disabled(), true);
        assert!(text.contains("FAIL"));
        assert!(!text.contains("PASS"), "passing rows are filtered out");
        // Footer still describes the whole run.
        assert!(text.contains("1 passed, 1 failed"));
        assert!(text.contains("showing 1 failed/errored of 2 cases"));
    }

    #[test]
    fn failed_only_json_filters_cases_but_keeps_summary() {
        let run = sample_run();
        let json = render_json(&run, true).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let cases = value["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0]["status"], "fail");
        // The summary is untouched.
        assert_eq!(value["summary"]["passed"], 1);
        assert_eq!(value["summary"]["total"], 2);
    }

    #[test]
    fn unfiltered_json_is_byte_identical_to_direct_serialization() {
        let run = sample_run();
        let via_emit = render_json(&run, false).unwrap();
        let direct = serde_json::to_string_pretty(&run).unwrap();
        assert_eq!(via_emit, direct);
    }
}
