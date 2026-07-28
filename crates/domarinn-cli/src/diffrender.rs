//! Rendering for `domarinn diff`: the comparison table, the PR-comment
//! markdown, and the shared unified-diff / score-delta / config-drift helpers.
//!
//! Split out of `diffcmd` (which keeps the command wiring) so each file stays
//! well under the source-line cap. `render_table` / `render_markdown` are the
//! only entry points; everything else is a private helper.

use std::collections::HashMap;

use domarinn_core::diff::{CaseDelta, Delta, RunDiff};
use domarinn_core::result::{CaseResult, RunResult};
use domarinn_core::stats::{wilson, Z_95};
use similar::{ChangeTag, TextDiff};

use crate::style::Palette;

/// Diff-cost guard: each side of an output/config diff is clamped to at most this
/// many bytes *before* `similar` runs, so a pathological multi-megabyte output
/// can never blow up the O(n·m) diff.
const DIFF_MAX_BYTES: usize = 64 * 1024;
/// Diff-cost guard companion to [`DIFF_MAX_BYTES`]: …and to at most this many
/// lines.
const DIFF_MAX_LINES: usize = 400;
/// Context lines kept around each change in a unified hunk.
const DIFF_CONTEXT_RADIUS: usize = 3;
/// Table render cap: diff lines shown per case before a `--full` hint is emitted
/// (suppressed by `--full`).
const TABLE_DIFF_LINES_PER_CASE: usize = 60;
/// Markdown render cap: diff lines per case inside the `<details>` block.
const MD_DIFF_LINES_PER_CASE: usize = 30;
/// Markdown render cap: diff lines across *all* cases (keeps a PR comment small).
const MD_DIFF_LINES_TOTAL: usize = 300;
/// Leading characters of a `blake3:…` digest shown in a drift note
/// (`"blake3:"` = 7 chars, plus 8 hex nibbles).
const DIGEST_ABBREV: usize = 7 + 8;
/// Leading characters of a run id shown in the comparison header.
const RUN_ID_ABBREV: usize = 8;

/// Which cases receive an inline unified output diff.
#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
#[clap(rename_all = "lowercase")]
pub enum DiffScope {
    /// Only newly-failing cases (the default).
    Regressions,
    /// Every case whose output changed.
    Changed,
    /// Every joined case with output on both sides.
    All,
    /// No inline diffs.
    None,
}

/// Index a run's cases by their stable `case_key`, for joining base↔head.
fn index_by_key(run: &RunResult) -> HashMap<&str, &CaseResult> {
    run.cases.iter().map(|c| (c.case_key.as_str(), c)).collect()
}

/// The text view of a case's output, matching the diff engine's own notion of
/// "output" (`Output::as_text`, compact JSON) so what we render agrees with the
/// `output_changed` flag computed in core.
fn case_output_text(case: &CaseResult) -> String {
    case.output
        .as_ref()
        .map(|o| o.as_text().into_owned())
        .unwrap_or_default()
}

/// First `n` characters of `s` (char-safe; shorter strings pass through).
fn short(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A `blake3:…`-style digest abbreviated for a drift note.
fn short_digest(d: &str) -> String {
    if d.chars().count() > DIGEST_ABBREV {
        format!("{}…", short(d, DIGEST_ABBREV))
    } else {
        d.to_string()
    }
}

/// Pretty-print a JSON value (falls back to the compact form if that ever fails).
fn pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Clamp `text` to at most [`DIFF_MAX_BYTES`] / [`DIFF_MAX_LINES`] before diffing.
/// Returns the input unchanged (borrowed) when it is already within both caps, so
/// the common small-output case allocates nothing and keeps its exact shape.
fn clamp_for_diff(text: &str) -> std::borrow::Cow<'_, str> {
    let within_bytes = text.len() <= DIFF_MAX_BYTES;
    let slice: &str = if within_bytes {
        text
    } else {
        let mut end = DIFF_MAX_BYTES;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    };
    // Short-circuit the line count: we only care whether it exceeds the cap.
    let within_lines = slice.lines().take(DIFF_MAX_LINES + 1).count() <= DIFF_MAX_LINES;
    if within_bytes && within_lines {
        std::borrow::Cow::Borrowed(text)
    } else {
        std::borrow::Cow::Owned(
            slice
                .lines()
                .take(DIFF_MAX_LINES)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// The raw unified-diff lines (uncolored, un-indented) of two texts after the
/// pre-diff clamp: `@@ … @@` hunk headers and `-`/`+`/` `-prefixed change lines.
/// Empty when the two texts are identical.
fn unified_diff_lines(base: &str, head: &str) -> Vec<String> {
    let base = clamp_for_diff(base);
    let head = clamp_for_diff(head);
    let diff = TextDiff::from_lines(base.as_ref(), head.as_ref());
    let mut unified = diff.unified_diff();
    unified.context_radius(DIFF_CONTEXT_RADIUS);
    let mut lines = Vec::new();
    for hunk in unified.iter_hunks() {
        lines.push(hunk.header().to_string());
        for change in hunk.iter_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
                ChangeTag::Equal => ' ',
            };
            let value = change.value();
            let value = value.strip_suffix('\n').unwrap_or(value);
            lines.push(format!("{sign}{value}"));
        }
    }
    lines
}

/// Style one raw unified-diff line and indent it by four: `-` red, `+` green,
/// hunk headers dim. The line's bytes are unchanged inside any escapes.
fn style_diff_line(line: &str, palette: &Palette) -> String {
    let styled = if line.starts_with("@@") {
        palette.dim(line)
    } else if line.starts_with('-') {
        palette.removed(line)
    } else if line.starts_with('+') {
        palette.added(line)
    } else {
        line.to_string()
    };
    format!("    {styled}")
}

/// The styled, indented, capped diff block for one case (or config section).
/// Empty when there is no difference. Under `!full`, at most
/// [`TABLE_DIFF_LINES_PER_CASE`] lines are shown, followed by a `--full` hint.
fn styled_diff_block(base: &str, head: &str, palette: &Palette, full: bool) -> Vec<String> {
    let raw = unified_diff_lines(base, head);
    if raw.is_empty() {
        return Vec::new();
    }
    let cap = if full {
        usize::MAX
    } else {
        TABLE_DIFF_LINES_PER_CASE
    };
    let mut out: Vec<String> = raw
        .iter()
        .take(cap)
        .map(|l| style_diff_line(l, palette))
        .collect();
    if raw.len() > cap {
        let hidden = raw.len() - cap;
        out.push(format!(
            "    {}",
            palette.dim(&format!("… +{hidden} more diff lines (--full to show)"))
        ));
    }
    out
}

/// Whether a case is selected for an inline diff under `scope`. A case must be
/// *joined* (present on both sides) to have anything to diff.
fn diff_selected(delta: Delta, output_changed: bool, joined: bool, scope: DiffScope) -> bool {
    if !joined {
        return false;
    }
    match scope {
        DiffScope::None => false,
        DiffScope::Regressions => delta == Delta::NewlyFailing,
        DiffScope::Changed => output_changed,
        DiffScope::All => true,
    }
}

/// The two-line comparison header: run ids / times / pass counts, then the
/// Wilson pass-rate intervals for each side.
fn diff_header(base: &RunResult, head: &RunResult, palette: &Palette) -> String {
    let line1 = format!(
        "base {} ({}, {}/{} passed) → head {} ({}, {}/{} passed)",
        short(base.run_id.as_str(), RUN_ID_ABBREV),
        base.finished_at.format("%Y-%m-%d %H:%M"),
        base.summary.passed,
        base.summary.total,
        short(head.run_id.as_str(), RUN_ID_ABBREV),
        head.finished_at.format("%Y-%m-%d %H:%M"),
        head.summary.passed,
        head.summary.total,
    );
    let br = wilson(base.summary.passed, base.summary.total, Z_95);
    let hr = wilson(head.summary.passed, head.summary.total, Z_95);
    let line2 = format!(
        "pass rate {:.1}% (95% CI {:.1}–{:.1}) → {:.1}% (95% CI {:.1}–{:.1})",
        br.rate * 100.0,
        br.lower * 100.0,
        br.upper * 100.0,
        hr.rate * 100.0,
        hr.lower * 100.0,
        hr.upper * 100.0,
    );
    format!("{}\n{line2}\n", palette.header(&line1))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_table(
    base: &RunResult,
    head: &RunResult,
    diff: &RunDiff,
    scope: DiffScope,
    full: bool,
    config_diff: bool,
    palette: &Palette,
) -> String {
    let base_by = index_by_key(base);
    let head_by = index_by_key(head);
    let s = &diff.summary;
    let mut out = String::new();

    out.push_str(&diff_header(base, head, palette));
    out.push('\n');
    out.push_str("comparison vs baseline:\n");

    for case in &diff.cases {
        let key = case.case_key.as_str();
        let base_c = base_by.get(key).copied();
        let head_c = head_by.get(key).copied();
        let joined = base_c.is_some() && head_c.is_some();

        // The four transition markers are always shown (unchanged behavior);
        // still-failing / unchanged cases surface only when they carry a diff.
        let transition = match case.delta {
            Delta::NewlyFailing => Some("REGRESS"),
            Delta::NewlyPassing => Some("FIXED  "),
            Delta::Added => Some("NEW    "),
            Delta::Removed => Some("REMOVED"),
            _ => None,
        };

        let diff_lines = if diff_selected(case.delta, case.output_changed, joined, scope) {
            let bt = base_c.map(case_output_text).unwrap_or_default();
            let ht = head_c.map(case_output_text).unwrap_or_default();
            styled_diff_block(&bt, &ht, palette, full)
        } else {
            Vec::new()
        };

        let label = match transition {
            Some(m) => m,
            None => {
                if diff_lines.is_empty() {
                    continue;
                }
                match case.delta {
                    Delta::StillFailing => "FAIL   ",
                    _ => "SAME   ",
                }
            }
        };

        let name = case
            .name
            .clone()
            .unwrap_or_else(|| case.case_key.to_string());
        let mut line = format!("  {label}  {name}");
        // Score delta only when both sides exist (skips NEW / REMOVED).
        if let (Some(b), Some(h)) = (base_c, head_c) {
            line.push_str(&format!("  score {:.2} → {:.2}", b.score, h.score));
        }
        out.push_str(&line);
        out.push('\n');
        for dl in diff_lines {
            out.push_str(&dl);
            out.push('\n');
        }
    }

    // Config / prompt drift, between the cases and the summary.
    if base.config_digest != head.config_digest {
        out.push('\n');
        out.push_str(&format!(
            "config changed: {} → {}\n",
            short_digest(&base.config_digest),
            short_digest(&head.config_digest),
        ));
        let (bs, hs) = if config_diff {
            (pretty(&base.config_snapshot), pretty(&head.config_snapshot))
        } else {
            (
                pretty(&base.config_snapshot["prompts"]),
                pretty(&head.config_snapshot["prompts"]),
            )
        };
        for dl in styled_diff_block(&bs, &hs, palette, full) {
            out.push_str(&dl);
            out.push('\n');
        }
    }

    out.push_str(&format!(
        "\n{} newly failing, {} newly passing, {} still failing, {} output-changed, {} added, {} removed\n",
        s.newly_failing, s.newly_passing, s.still_failing, s.output_changed, s.added, s.removed
    ));
    out.push_str(&format!(
        "McNemar: {} regressions vs {} fixes, statistic {:.2} {}\n",
        diff.mcnemar.regressions,
        diff.mcnemar.fixes,
        diff.mcnemar.statistic,
        if diff.mcnemar.significant {
            "(significant at 95%)"
        } else {
            "(not significant at 95%)"
        }
    ));
    out
}

/// A markdown comparison suitable for a PR comment.
pub fn render_markdown(base: &RunResult, head: &RunResult, diff: &RunDiff) -> String {
    let s = &diff.summary;
    let mut out = String::new();
    let verdict = if diff.has_regression() {
        "❌ Regressions detected"
    } else {
        "✅ No regressions"
    };
    out.push_str(&format!("### domarinn comparison — {verdict}\n\n"));
    out.push_str("| metric | count |\n|---|---|\n");
    out.push_str(&format!("| Newly failing | {} |\n", s.newly_failing));
    out.push_str(&format!("| Newly passing | {} |\n", s.newly_passing));
    out.push_str(&format!("| Still failing | {} |\n", s.still_failing));
    out.push_str(&format!("| Output changed | {} |\n", s.output_changed));
    out.push_str(&format!("| Added | {} |\n", s.added));
    out.push_str(&format!("| Removed | {} |\n", s.removed));
    if diff.mcnemar.significant {
        out.push_str(&format!(
            "\n> McNemar test: change is statistically significant (statistic {:.2}).\n",
            diff.mcnemar.statistic
        ));
    }
    if base.config_digest != head.config_digest {
        out.push_str(&format!(
            "\n> Config changed: `{}` → `{}`.\n",
            short_digest(&base.config_digest),
            short_digest(&head.config_digest),
        ));
    }

    let base_by = index_by_key(base);
    let head_by = index_by_key(head);
    let regressions: Vec<&CaseDelta> = diff
        .cases
        .iter()
        .filter(|c| c.delta == Delta::NewlyFailing)
        .collect();

    if !regressions.is_empty() {
        out.push_str("\n**Newly failing:**\n\n");
        out.push_str("| test | score |\n|---|---|\n");
        for c in &regressions {
            let name = c.name.clone().unwrap_or_else(|| c.case_key.to_string());
            let key = c.case_key.as_str();
            let score = match (base_by.get(key), head_by.get(key)) {
                (Some(b), Some(h)) => format!("{:.2} → {:.2}", b.score, h.score),
                _ => "—".to_string(),
            };
            out.push_str(&format!("| {name} | {score} |\n"));
        }

        // Per-regression output diffs, capped per case and in total so a large
        // regression can't produce a giant PR comment.
        let mut body = String::new();
        let mut total = 0usize;
        let mut any = false;
        let mut total_capped = false;
        for c in &regressions {
            if total >= MD_DIFF_LINES_TOTAL {
                total_capped = true;
                break;
            }
            let key = c.case_key.as_str();
            let (Some(b), Some(h)) = (base_by.get(key), head_by.get(key)) else {
                continue;
            };
            let raw = unified_diff_lines(&case_output_text(b), &case_output_text(h));
            if raw.is_empty() {
                continue;
            }
            any = true;
            let per_case_cap = MD_DIFF_LINES_PER_CASE.min(MD_DIFF_LINES_TOTAL - total);
            let shown = raw.len().min(per_case_cap);
            let hidden = raw.len() - shown;
            let name = c.name.clone().unwrap_or_else(|| c.case_key.to_string());
            body.push_str(&format!("\n_{name}_\n\n```diff\n"));
            for l in raw.iter().take(shown) {
                body.push_str(l);
                body.push('\n');
            }
            if hidden > 0 {
                body.push_str(&format!("… +{hidden} more diff lines\n"));
            }
            body.push_str("```\n");
            total += shown;
        }
        if any {
            out.push_str("\n<details><summary>Output diffs</summary>\n");
            out.push_str(&body);
            if total_capped {
                out.push_str(&format!(
                    "\n_output diffs truncated at {MD_DIFF_LINES_TOTAL} total lines_\n"
                ));
            }
            out.push_str("</details>\n");
        }
    }
    out
}

#[cfg(test)]
mod diff_tests {
    use super::*;
    use domarinn_core::diff::diff_runs;
    use domarinn_core::ids::RunId;
    use domarinn_core::result::{CaseStatus, CellKey, RunSummary, RESULT_SCHEMA_VERSION};
    use domarinn_core::types::Output;

    fn case(test: &str, status: CaseStatus, score: f64, output: Option<&str>) -> CaseResult {
        let cell = CellKey {
            provider_id: "p".into(),
            prompt_id: None,
            test_id: test.into(),
            repeat: 0,
        };
        CaseResult {
            case_key: cell.case_key(),
            cell,
            name: Some(test.into()),
            tags: vec![],
            vars: Default::default(),
            status,
            score,
            output: output.map(|o| Output::Text(o.into())),
            prompt: None,
            request: None,
            stop_reason: None,
            raw: None,
            asserts: vec![],
            usage: None,
            cost_usd: None,
            latency_ms: 0,
            wall_ms: None,
            reasoning: None,
            empty_reason: None,
            cached: false,
            attempts: 1,
            prompt_digest: None,
            provider_digest: None,
            assert_digest: None,
            error: None,
        }
    }

    fn run_with(cases: Vec<CaseResult>, digest: &str, snapshot: serde_json::Value) -> RunResult {
        let passed = cases
            .iter()
            .filter(|c| matches!(c.status, CaseStatus::Pass))
            .count() as u64;
        let total = cases.len() as u64;
        let now = chrono::Utc::now();
        RunResult {
            schema_version: RESULT_SCHEMA_VERSION,
            run_id: RunId::new("0123456789abcdef"),
            project: None,
            suite: Some("s".into()),
            started_at: now,
            finished_at: now,
            config_digest: digest.into(),
            config_snapshot: snapshot,
            git: None,
            ci: None,
            digests: None,
            origin: None,
            share_url: None,
            filters: Default::default(),
            summary: RunSummary {
                total,
                passed,
                failed: total - passed,
                ..Default::default()
            },
            cases,
        }
    }

    // --- DiffScope selection ------------------------------------------------

    #[test]
    fn diff_selected_regressions_only_newly_failing() {
        for (delta, want) in [
            (Delta::NewlyFailing, true),
            (Delta::NewlyPassing, false),
            (Delta::StillFailing, false),
            (Delta::Unchanged, false),
        ] {
            assert_eq!(
                diff_selected(delta, true, true, DiffScope::Regressions),
                want,
                "{delta:?}"
            );
        }
    }

    #[test]
    fn diff_selected_changed_follows_output_changed() {
        assert!(diff_selected(
            Delta::StillFailing,
            true,
            true,
            DiffScope::Changed
        ));
        assert!(!diff_selected(
            Delta::StillFailing,
            false,
            true,
            DiffScope::Changed
        ));
        // Even an unchanged-status case shows when its output changed.
        assert!(diff_selected(
            Delta::Unchanged,
            true,
            true,
            DiffScope::Changed
        ));
    }

    #[test]
    fn diff_selected_all_needs_joined_none_never() {
        assert!(diff_selected(Delta::Unchanged, false, true, DiffScope::All));
        // Not joined (Added/Removed) → nothing to diff regardless of scope.
        assert!(!diff_selected(Delta::Added, true, false, DiffScope::All));
        assert!(!diff_selected(
            Delta::NewlyFailing,
            true,
            true,
            DiffScope::None
        ));
    }

    // --- Clamp caps (byte + line) ------------------------------------------

    #[test]
    fn clamp_passes_small_input_through_borrowed() {
        let text = "a\nb\nc";
        assert!(matches!(
            clamp_for_diff(text),
            std::borrow::Cow::Borrowed(_)
        ));
    }

    #[test]
    fn clamp_truncates_by_line_count() {
        let many = (0..DIFF_MAX_LINES + 50)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let clamped = clamp_for_diff(&many);
        assert!(matches!(clamped, std::borrow::Cow::Owned(_)));
        assert_eq!(clamped.lines().count(), DIFF_MAX_LINES);
    }

    #[test]
    fn clamp_truncates_by_byte_budget() {
        // One long line well over the byte cap, no newlines.
        let big = "x".repeat(DIFF_MAX_BYTES * 2);
        let clamped = clamp_for_diff(&big);
        assert!(matches!(clamped, std::borrow::Cow::Owned(_)));
        assert!(clamped.len() <= DIFF_MAX_BYTES);
    }

    // --- Unified diff + render cap -----------------------------------------

    #[test]
    fn unified_diff_lines_marks_removed_and_added() {
        let lines = unified_diff_lines("hello", "goodbye");
        assert!(lines.iter().any(|l| l == "-hello"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "+goodbye"), "{lines:?}");
        assert!(lines.iter().any(|l| l.starts_with("@@")));
    }

    #[test]
    fn unified_diff_lines_empty_when_identical() {
        assert!(unified_diff_lines("same\ntext", "same\ntext").is_empty());
    }

    #[test]
    fn styled_diff_block_caps_at_sixty_unless_full() {
        let base = (0..80)
            .map(|i| format!("a{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let head = (0..80)
            .map(|i| format!("b{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = styled_diff_block(&base, &head, &Palette::disabled(), false);
        assert_eq!(
            capped.len(),
            TABLE_DIFF_LINES_PER_CASE + 1,
            "60 + hint line"
        );
        assert!(capped
            .last()
            .unwrap()
            .contains("more diff lines (--full to show)"));

        let full = styled_diff_block(&base, &head, &Palette::disabled(), true);
        assert!(full.len() > TABLE_DIFF_LINES_PER_CASE);
        assert!(!full.iter().any(|l| l.contains("--full to show")));
    }

    #[test]
    fn styled_diff_block_colors_under_enabled_palette() {
        let block = styled_diff_block("hello", "goodbye", &Palette::for_test(true), false);
        let joined = block.join("\n");
        assert!(joined.contains('\x1b'), "enabled palette emits escapes");
        // The line bytes survive inside the escapes.
        assert!(joined.contains("-hello") && joined.contains("+goodbye"));
    }

    // --- Table: header, score line, config drift ----------------------------

    fn regression_pair() -> (RunResult, RunResult) {
        let base = run_with(
            vec![case("t1", CaseStatus::Pass, 1.0, Some("hello"))],
            "blake3:aaaa1111bbbb2222",
            serde_json::json!({"prompts": [{"id": "g", "template": "Hi"}]}),
        );
        let head = run_with(
            vec![case("t1", CaseStatus::Fail, 0.0, Some("goodbye"))],
            "blake3:cccc3333dddd4444",
            serde_json::json!({"prompts": [{"id": "g", "template": "Hello"}]}),
        );
        (base, head)
    }

    #[test]
    fn table_shows_header_score_delta_and_diff() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let text = render_table(
            &base,
            &head,
            &diff,
            DiffScope::Regressions,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(
            text.contains("base 01234567 ("),
            "run id abbreviated; got:\n{text}"
        );
        assert!(text.contains("1/1 passed"));
        assert!(text.contains("pass rate 100.0% (95% CI"));
        assert!(
            text.contains("REGRESS  t1  score 1.00 → 0.00"),
            "got:\n{text}"
        );
        assert!(text.contains("-hello"));
        assert!(text.contains("+goodbye"));
        assert!(text.contains("1 newly failing"));
        assert!(!text.contains('\x1b'), "disabled palette has no escapes");
    }

    #[test]
    fn table_config_drift_present_iff_digests_differ() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let drift = render_table(
            &base,
            &head,
            &diff,
            DiffScope::Regressions,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(
            drift.contains("config changed: blake3:aaaa1111"),
            "got:\n{drift}"
        );
        // The prompts section diff surfaces the template change.
        assert!(drift.contains("\"template\""));

        // Identical digests → silence.
        let mut head_same = head.clone();
        head_same.config_digest = base.config_digest.clone();
        head_same.config_snapshot = base.config_snapshot.clone();
        let diff2 = diff_runs(&base, &head_same);
        let quiet = render_table(
            &base,
            &head_same,
            &diff2,
            DiffScope::Regressions,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(!quiet.contains("config changed:"), "got:\n{quiet}");
    }

    #[test]
    fn table_config_diff_flag_diffs_whole_snapshot() {
        // Snapshots differ only outside "prompts": default is quiet on prompts,
        // but --config-diff surfaces the change.
        let base = run_with(
            vec![case("t1", CaseStatus::Pass, 1.0, Some("x"))],
            "blake3:aaaa",
            serde_json::json!({"providers": [{"id": "a"}]}),
        );
        let head = run_with(
            vec![case("t1", CaseStatus::Pass, 1.0, Some("x"))],
            "blake3:bbbb",
            serde_json::json!({"providers": [{"id": "b"}]}),
        );
        let diff = diff_runs(&base, &head);
        let default = render_table(
            &base,
            &head,
            &diff,
            DiffScope::All,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(default.contains("config changed:"));
        assert!(
            !default.contains("\"providers\""),
            "prompts-only by default"
        );
        let full = render_table(
            &base,
            &head,
            &diff,
            DiffScope::All,
            false,
            true,
            &Palette::disabled(),
        );
        assert!(
            full.contains("\"providers\""),
            "whole snapshot with --config-diff"
        );
    }

    #[test]
    fn table_diffs_none_suppresses_hunks() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let text = render_table(
            &base,
            &head,
            &diff,
            DiffScope::None,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(text.contains("REGRESS  t1"), "marker still shown");
        assert!(!text.contains("-hello"), "no hunks under --diffs none");
        assert!(!text.contains("+goodbye"));
    }

    #[test]
    fn table_mcnemar_states_significance_explicitly() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let text = render_table(
            &base,
            &head,
            &diff,
            DiffScope::Regressions,
            false,
            false,
            &Palette::disabled(),
        );
        assert!(text.contains("(not significant at 95%)"), "got:\n{text}");
    }

    // --- Markdown -----------------------------------------------------------

    #[test]
    fn markdown_has_score_column_and_output_diffs() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        assert!(md.contains("| test | score |"), "score column; got:\n{md}");
        assert!(md.contains("| t1 | 1.00 → 0.00 |"));
        assert!(md.contains("<details><summary>Output diffs</summary>"));
        assert!(md.contains("```diff"));
        assert!(md.contains("-hello") && md.contains("+goodbye"));
        assert!(md.contains("> Config changed:"), "drift note; got:\n{md}");
    }

    #[test]
    fn markdown_caps_diff_lines_per_case() {
        let base = run_with(
            vec![case(
                "t1",
                CaseStatus::Pass,
                1.0,
                Some(
                    &(0..50)
                        .map(|i| format!("a{i}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            )],
            "d1",
            serde_json::Value::Null,
        );
        let head = run_with(
            vec![case(
                "t1",
                CaseStatus::Fail,
                0.0,
                Some(
                    &(0..50)
                        .map(|i| format!("b{i}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
            )],
            "d1",
            serde_json::Value::Null,
        );
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        // 100+ raw diff lines, capped at 30 per case with an inline note.
        assert!(
            md.contains("more diff lines"),
            "per-case cap note; got:\n{md}"
        );
        let fence_lines = md
            .lines()
            .skip_while(|l| !l.contains("```diff"))
            .skip(1)
            .take_while(|l| !l.starts_with("```"))
            .filter(|l| !l.contains("more diff lines"))
            .count();
        assert!(
            fence_lines <= MD_DIFF_LINES_PER_CASE,
            "at most {MD_DIFF_LINES_PER_CASE} diff lines in the fence, got {fence_lines}"
        );
    }
}
