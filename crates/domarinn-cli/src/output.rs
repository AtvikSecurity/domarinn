//! Rendering and persisting run results.

use std::io::Write;
use std::path::Path;

use domarinn_core::result::{AssertStatus, CaseResult, CaseStatus, RunResult, RunSummary};

use crate::style::Palette;

/// Failure rows the markdown summary will print before collapsing the rest into
/// a "…and N more" line. A PR comment is a poor place to read 300 failures, and
/// GitHub truncates a long one anyway — the run URL and the JUnit artifact are
/// where the full list lives.
const MD_FAILURE_ROWS: usize = 10;

#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Table,
    Json,
    Jsonl,
    Junit,
    /// A markdown run summary (pass/fail counts + Wilson pass-rate interval).
    Md,
}

/// Persist a run under `.domarinn/runs/<run_id>/result.json` and update the
/// `latest` pointer.
pub fn persist(result: &RunResult) -> std::io::Result<()> {
    let dir = Path::new(".domarinn")
        .join("runs")
        .join(result.run_id.as_str());
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(result).map_err(std::io::Error::other)?;
    std::fs::write(dir.join("result.json"), json)?;
    std::fs::write(
        Path::new(".domarinn").join("runs").join("latest"),
        result.run_id.as_str(),
    )?;
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
        Format::Md => render_run_md(result),
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
    let header = format!("{:<6}{:<10}{:<name_w$}score", "", "provider", "test");
    out.push_str(&palette.header(&header));
    out.push('\n');
    for case in &cases {
        out.push_str(&format!(
            "{}  {:<10}{:<name_w$}{:.2}\n",
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
fn duration_secs(result: &RunResult) -> f64 {
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
fn humanize_count(n: u64) -> String {
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
    if s.cache_read_tokens > 0 || s.cache_write_tokens > 0 {
        segments.push(format!(
            "{} cache-read / {} cache-write tokens",
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

/// A markdown run summary: the reusable core of [`crate::diffcmd::write_summary_md`],
/// also served by `run --format md` / `view --format md`. Never colored.
///
/// A headline metrics table, then the failing cases. Metric rows that carry no
/// information (no cache was consulted, no tokens were counted, nothing was
/// retried) are omitted rather than printed as zero, so what remains is all
/// signal — the same rule [`stats_line`] applies to the terminal footer.
pub fn render_run_md(run: &RunResult) -> String {
    let mut out = render_run_md_headline(run);
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
    let mut out = match run.suite.as_deref() {
        Some(suite) => format!("### domarinn run — {}\n\n", md_cell(suite)),
        None => "### domarinn run\n\n".to_string(),
    };

    out.push_str("| metric | value |\n|---|---|\n");
    out.push_str(&format!("| Result | {} |\n", result_line(s)));

    let rate = domarinn_core::stats::wilson(s.passed, s.total, domarinn_core::stats::Z_95);
    out.push_str(&format!(
        "| Pass rate | {:.1}% (95% CI {:.1}–{:.1}%, n={}) |\n",
        rate.rate * 100.0,
        rate.lower * 100.0,
        rate.upper * 100.0,
        rate.total
    ));

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
    out.push_str(&format!(
        "| Duration | {} |\n",
        humanize_duration(duration_secs(run))
    ));
    out
}

/// `✅ 12 passed`, or `❌` plus every nonzero failure bucket.
fn result_line(s: &RunSummary) -> String {
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
    let glyph = if s.failed > 0 || s.errored > 0 {
        "❌"
    } else {
        "✅"
    };
    format!("{glyph} {}", parts.join(", "))
}

/// The failing/errored cases as a table, capped at [`MD_FAILURE_ROWS`]. Empty
/// when the run is clean.
pub fn render_failures_md(run: &RunResult) -> String {
    let failures: Vec<&CaseResult> = run
        .cases
        .iter()
        .filter(|c| matches!(c.status, CaseStatus::Fail | CaseStatus::Error))
        .collect();
    if failures.is_empty() {
        return String::new();
    }

    let mut out =
        String::from("\n**Failing:**\n\n| test | provider | score | why |\n|---|---|---|---|\n");
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

/// Why a case failed: its first failing assertion, or the error that stopped it
/// before any assertion ran.
fn failure_reason(case: &CaseResult) -> String {
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
fn md_cell(s: &str) -> String {
    s.replace('|', r"\|")
        .replace(['\n', '\r'], " ")
        .trim()
        .to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(md.contains("| metric | value |"));
        assert!(md.contains("| Result | ❌ 3 passed, 1 failed |"));
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
        assert!(md.contains("| Result | ✅ 4 passed |"));
        assert!(!md.contains("**Failing:**"));
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

    #[test]
    fn render_run_md_tables_failing_cases_with_the_first_failing_assert() {
        let md = render_run_md(&sample_run());
        assert!(md.contains("**Failing:**"));
        assert!(md.contains("| test | provider | score | why |"));
        assert!(md.contains("| bad | p | 0.00 | contains: expected \"decline\" |"));
        // Passing cases never appear in the failure table.
        assert!(!md.contains("| ok | p |"));
    }

    #[test]
    fn render_run_md_uses_the_error_text_for_errored_cases() {
        let mut run = sample_run();
        run.cases[1].status = CaseStatus::Error;
        run.cases[1].asserts.clear();
        run.cases[1].error = Some("provider timed out".into());
        assert!(render_run_md(&run).contains("| bad | p | 0.00 | provider timed out |"));
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

    /// A minimal run with one passing and one failing case, for renderer tests.
    fn sample_run() -> RunResult {
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
