//! The PR-comment markdown for a baseline comparison: verdict, both sides
//! identified, a change table, the newly-failing cases and their output diffs.
//!
//! Split out of [`crate::diffrender`] (which keeps the terminal table and the
//! shared diff helpers this module borrows) so each file stays under the
//! source-line cap. [`render_markdown`] is the only entry point; it is what
//! `ci-summary`, `run --against` and `diff --format md` all post.

use domarinn_core::diff::{CaseDelta, Delta, RunDiff};
use domarinn_core::result::RunResult;

use crate::diffrender::{
    case_output_text, index_by_key, short, short_digest, unified_diff_lines,
    MD_DIFF_LINES_PER_CASE, MD_DIFF_LINES_TOTAL, RUN_ID_ABBREV,
};
use crate::outputmd::md_cell;

/// One side of the comparison, identified for the markdown header line: a
/// branch-merged composite names its branch and how many runs fed it (its
/// synthetic run id would identify nothing a reader can find), a real run its
/// abbreviated id, finish time and pass count.
fn md_run_label(run: &RunResult) -> String {
    let passed = format!("{}/{} passed", run.summary.passed, run.summary.total);
    match &run.composite {
        Some(c) => {
            let n = c.contributing_run_ids.len();
            let runs = if n == 1 { "run" } else { "runs" };
            format!(
                "branch `{}` (merged from {n} {runs}, {passed})",
                md_cell(&c.branch)
            )
        }
        None => format!(
            "`{}` ({}, {passed})",
            short(run.run_id.as_str(), RUN_ID_ABBREV),
            run.finished_at.format("%Y-%m-%d %H:%M"),
        ),
    }
}

/// A markdown comparison suitable for a PR comment.
///
/// Verdict first (bold words, no glyphs — the same rule as the run headline),
/// then a line identifying both sides, then a change table that omits
/// zero-count rows: what is printed is what changed.
pub fn render_markdown(base: &RunResult, head: &RunResult, diff: &RunDiff) -> String {
    let s = &diff.summary;
    let mut out = String::from("### Comparison with baseline\n\n");
    if diff.has_regression() {
        out.push_str(&format!(
            "**Regressions detected** — {} newly failing\n\n",
            s.newly_failing
        ));
    } else {
        out.push_str("**No regressions**\n\n");
    }
    out.push_str(&format!(
        "Baseline {} → this run {}\n",
        md_run_label(base),
        md_run_label(head),
    ));

    let rows = [
        ("Newly failing", s.newly_failing),
        ("Newly passing", s.newly_passing),
        ("Still failing", s.still_failing),
        ("Output changed", s.output_changed),
        ("Added", s.added),
        ("Removed", s.removed),
    ];
    if rows.iter().all(|(_, n)| *n == 0) {
        out.push_str("\nNo case-level changes.\n");
    } else {
        out.push_str("\n| Change | Cases |\n|---|---|\n");
        for (label, n) in rows {
            if n > 0 {
                out.push_str(&format!("| {label} | {n} |\n"));
            }
        }
    }
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
        out.push_str("\n#### Newly failing\n\n");
        out.push_str("| Test | Score (baseline → this run) |\n|---|---|\n");
        for c in &regressions {
            let name = c.name.clone().unwrap_or_else(|| c.case_key.to_string());
            let key = c.case_key.as_str();
            let score = match (base_by.get(key), head_by.get(key)) {
                (Some(b), Some(h)) => format!("{:.2} → {:.2}", b.score, h.score),
                _ => "—".to_string(),
            };
            // Escaped like every other cell: a `|` in a test name would split
            // the row and shred the table for everything below it.
            out.push_str(&format!("| {} | {score} |\n", md_cell(&name)));
        }

        // Per-regression output diffs, capped per case and in total so a large
        // regression can't produce a giant PR comment.
        let mut body = String::new();
        let mut total = 0usize;
        let mut cases_with_diffs = 0usize;
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
            cases_with_diffs += 1;
            let per_case_cap = MD_DIFF_LINES_PER_CASE.min(MD_DIFF_LINES_TOTAL - total);
            let shown = raw.len().min(per_case_cap);
            let hidden = raw.len() - shown;
            let name = c.name.clone().unwrap_or_else(|| c.case_key.to_string());
            body.push_str(&format!("\n**{}**\n\n```diff\n", md_cell(&name)));
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
        if cases_with_diffs > 0 {
            let cases = if cases_with_diffs == 1 {
                "case"
            } else {
                "cases"
            };
            out.push_str(&format!(
                "\n<details><summary>Output diffs ({cases_with_diffs} {cases})</summary>\n"
            ));
            out.push_str(&body);
            if total_capped {
                out.push_str(&format!(
                    "\n_Output diffs truncated at {MD_DIFF_LINES_TOTAL} total lines._\n"
                ));
            }
            out.push_str("</details>\n");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diffrender::fixtures::{case, regression_pair, run_with};
    use domarinn_core::diff::diff_runs;
    use domarinn_core::ids::RunId;
    use domarinn_core::result::CaseStatus;

    #[test]
    fn markdown_has_score_column_and_output_diffs() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        assert!(
            md.contains("**Regressions detected** — 1 newly failing"),
            "verdict line; got:\n{md}"
        );
        assert!(
            md.contains("| Test | Score (baseline → this run) |"),
            "score column; got:\n{md}"
        );
        assert!(md.contains("| t1 | 1.00 → 0.00 |"));
        assert!(md.contains("<details><summary>Output diffs (1 case)</summary>"));
        assert!(md.contains("```diff"));
        assert!(md.contains("-hello") && md.contains("+goodbye"));
        assert!(md.contains("> Config changed:"), "drift note; got:\n{md}");
        assert!(!md.contains('✅') && !md.contains('❌'), "no emoji: {md}");
    }

    /// The comment says what it compared: both sides identified by abbreviated
    /// run id, finish time and pass count — a reader of last week's comment can
    /// still find the baseline it measured against.
    #[test]
    fn markdown_identifies_both_sides_of_the_comparison() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        assert!(
            md.contains("Baseline `01234567` (") && md.contains("1/1 passed"),
            "got:\n{md}"
        );
        assert!(md.contains("→ this run `01234567` ("), "got:\n{md}");
    }

    /// A branch-merged composite baseline is labelled by its branch and the
    /// runs that fed it — its synthetic run id identifies nothing a reader can
    /// look up.
    #[test]
    fn markdown_labels_a_composite_baseline_by_branch() {
        let (mut base, head) = regression_pair();
        base.composite = Some(domarinn_core::result::CompositeBaseline {
            branch: "main".into(),
            contributing_run_ids: vec![RunId::new("aaaabbbbccccdddd")],
            truncated: None,
        });
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        assert!(
            md.contains("Baseline branch `main` (merged from 1 run, 1/1 passed)"),
            "got:\n{md}"
        );
    }

    /// Zero-count change rows are omitted — what is printed is what changed —
    /// and a comparison with no changes at all says so in prose instead of
    /// rendering an empty table.
    #[test]
    fn markdown_omits_zero_change_rows() {
        let (base, head) = regression_pair();
        let diff = diff_runs(&base, &head);
        let md = render_markdown(&base, &head, &diff);
        assert!(md.contains("| Newly failing | 1 |"), "got:\n{md}");
        assert!(!md.contains("| Newly passing |"), "got:\n{md}");
        assert!(!md.contains("| Added |"), "got:\n{md}");

        let same = run_with(
            vec![case("t1", CaseStatus::Pass, 1.0, Some("hello"))],
            "d1",
            serde_json::Value::Null,
        );
        let no_change = diff_runs(&same, &same);
        let md = render_markdown(&same, &same, &no_change);
        assert!(md.contains("**No regressions**"), "got:\n{md}");
        assert!(md.contains("No case-level changes."), "got:\n{md}");
        assert!(!md.contains("| Change |"), "got:\n{md}");
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
