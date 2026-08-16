//! Merging runs on a branch into one synthetic baseline document.
//!
//! A branch baseline ("gate this PR against `main`") cannot be the single
//! newest run on the branch: if that run was a `--filter` or `--provider`
//! subset, every case it lacks diffs as `Added` and can never regress — the
//! gate's denominator silently shrinks. Instead the baseline is a *merge*: per
//! [`case_key`](domarinn_types::result::CaseResult::case_key), the newest run
//! on the branch that has the case wins, walking newest→oldest over a bounded
//! window.
//!
//! One function shared by the CLI (merging local store runs) and the server
//! (merging hydrated export blobs), so the two can never disagree about what a
//! branch baseline contains. The result is built in memory for comparison and
//! is never persisted or uploaded.

use domarinn_types::result::RunResult;

/// How many newest runs on a branch a composite may draw from.
///
/// Normally the newest run covers every case and the walk stops immediately;
/// the window only matters for filtered or sharded uploads. Twenty covers a
/// 10-way shard matrix twice over while bounding the server's worst case at 20
/// blob hydrations per gate. Also the horizon past which cases deleted from
/// the suite age out of the baseline (as non-gating `removed` noise until
/// then).
pub const BRANCH_LOOKBACK: usize = 20;

/// Merge `runs_newest_first` (already filtered to one branch and suite, newest
/// first) into a synthetic baseline document for `branch`.
///
/// Returns `None` when no run contributed any case — an absent baseline, not
/// an empty one.
pub fn merge_branch_runs(branch: &str, runs_newest_first: Vec<RunResult>) -> Option<RunResult> {
    use domarinn_types::ids::RunId;
    use domarinn_types::result::{CompositeBaseline, GitMeta, RESULT_SCHEMA_VERSION};
    use std::collections::HashSet;

    let mut seen: HashSet<String> = HashSet::new();
    let mut merged_cases = Vec::new();
    let mut contributors: Vec<RunId> = Vec::new();
    // The skeleton — everything but cases/summary/composite — comes from the
    // newest contributor: the run whose config the baseline most plausibly
    // represents.
    let mut skeleton: Option<RunResult> = None;
    // The length of the whole newest→oldest walk, so "was the horizon still
    // productive" below can tell a cap that cut coverage off from one that
    // changed nothing.
    let total = runs_newest_first.len();
    let mut last_contributing_index = None;

    for (index, mut run) in runs_newest_first
        .into_iter()
        .take(BRANCH_LOOKBACK)
        .enumerate()
    {
        let cases = std::mem::take(&mut run.cases);
        let mut contributed = false;
        for case in cases {
            if seen.insert(case.case_key.as_str().to_string()) {
                merged_cases.push(case);
                contributed = true;
            }
        }
        if contributed {
            contributors.push(run.run_id.clone());
            last_contributing_index = Some(index);
            if skeleton.is_none() {
                skeleton = Some(run);
            }
        }
    }

    let newest = skeleton?;
    // Truncated iff the very last run inside the window still contributed and
    // there were runs beyond it — older coverage may exist past the horizon.
    let truncated = (last_contributing_index == Some(BRANCH_LOOKBACK - 1)
        && total > BRANCH_LOOKBACK)
        .then_some(true);
    let summary = crate::runner::summarize(&merged_cases);
    Some(RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        run_id: format!("composite-{branch}-{}", newest.run_id.as_str()).into(),
        project: newest.project,
        suite: newest.suite,
        started_at: newest.started_at,
        finished_at: newest.finished_at,
        config_digest: newest.config_digest,
        config_snapshot: newest.config_snapshot,
        git: Some(GitMeta {
            branch: Some(branch.to_string()),
            // A composite spans commits; claiming one would mislead.
            commit: None,
            dirty: false,
        }),
        ci: None,
        digests: newest.digests,
        origin: None,
        share_url: None,
        composite: Some(CompositeBaseline {
            branch: branch.to_string(),
            contributing_run_ids: contributors,
            truncated,
        }),
        // Deliberately empty, not the newest run's: the merge spans runs with
        // different filters, and claiming any one set would imply the whole
        // composite was produced under it.
        filters: Default::default(),
        summary,
        cases: merged_cases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use domarinn_types::result::{CaseResult, CaseStatus, CellKey, GitMeta, RunResult, RunSummary};
    use domarinn_types::types::Output;

    fn case(key: &str, status: CaseStatus) -> CaseResult {
        CaseResult {
            cache_key: None,
            tool_calls: Vec::new(),
            cell: CellKey {
                provider_id: "p".into(),
                prompt_id: None,
                test_id: key.into(),
                repeat: 0,
            },
            case_key: key.into(),
            name: Some(key.into()),
            tags: vec![],
            vars: Default::default(),
            status,
            score: if matches!(status, CaseStatus::Pass) {
                1.0
            } else {
                0.0
            },
            output: Some(Output::Text("out".into())),
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
            error_details: None,
            model: None,
            error_class: None,
            answered_by_provider_id: None,
            fallback_attempts: Vec::new(),
            expect_fail_reason: None,
        }
    }

    fn run(id: &str, cases: Vec<CaseResult>) -> RunResult {
        RunResult {
            schema_version: 4,
            run_id: id.into(),
            project: Some("proj".into()),
            suite: Some("suite".into()),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            config_digest: format!("digest-{id}"),
            config_snapshot: serde_json::Value::Null,
            git: Some(GitMeta {
                branch: Some("main".into()),
                commit: Some(format!("commit-{id}")),
                dirty: false,
            }),
            ci: None,
            digests: None,
            origin: None,
            share_url: None,
            composite: None,
            filters: Default::default(),
            summary: RunSummary::default(),
            cases,
        }
    }

    fn keys(merged: &RunResult) -> Vec<&str> {
        let mut ks: Vec<&str> = merged.cases.iter().map(|c| c.case_key.as_str()).collect();
        ks.sort_unstable();
        ks
    }

    #[test]
    fn the_newest_run_wins_per_case_key() {
        let newest = run("r2", vec![case("shared-16-chars0", CaseStatus::Fail)]);
        let older = run("r1", vec![case("shared-16-chars0", CaseStatus::Pass)]);
        let merged = merge_branch_runs("main", vec![newest, older]).unwrap();
        assert_eq!(merged.cases.len(), 1);
        assert!(matches!(merged.cases[0].status, CaseStatus::Fail));
    }

    #[test]
    fn older_runs_fill_cases_the_newest_run_lacks() {
        let newest = run("r2", vec![case("case-aaaaaaaaaaaa", CaseStatus::Pass)]);
        let older = run(
            "r1",
            vec![
                case("case-aaaaaaaaaaaa", CaseStatus::Fail),
                case("case-bbbbbbbbbbbb", CaseStatus::Pass),
            ],
        );
        let merged = merge_branch_runs("main", vec![newest, older]).unwrap();
        assert_eq!(
            keys(&merged),
            vec!["case-aaaaaaaaaaaa", "case-bbbbbbbbbbbb"]
        );
        let a = merged
            .cases
            .iter()
            .find(|c| c.case_key.as_str() == "case-aaaaaaaaaaaa")
            .unwrap();
        assert!(
            matches!(a.status, CaseStatus::Pass),
            "the newest run's verdict must win for the shared case"
        );
    }

    #[test]
    fn the_summary_is_recomputed_over_the_merged_cases() {
        let newest = run("r2", vec![case("case-aaaaaaaaaaaa", CaseStatus::Pass)]);
        let older = run(
            "r1",
            vec![
                case("case-aaaaaaaaaaaa", CaseStatus::Fail),
                case("case-bbbbbbbbbbbb", CaseStatus::Fail),
            ],
        );
        let merged = merge_branch_runs("main", vec![newest, older]).unwrap();
        assert_eq!(merged.summary.total, 2);
        assert_eq!(merged.summary.passed, 1);
        assert_eq!(merged.summary.failed, 1);
    }

    #[test]
    fn an_empty_run_list_merges_to_none() {
        assert!(merge_branch_runs("main", vec![]).is_none());
    }

    #[test]
    fn runs_with_no_cases_at_all_merge_to_none() {
        // An absent baseline, not an empty one: a zero-case document would
        // resolve, compare, and pass every gate vacuously.
        let empty = run("r1", vec![]);
        assert!(merge_branch_runs("main", vec![empty]).is_none());
    }

    #[test]
    fn contributing_run_ids_name_only_runs_that_contributed() {
        let newest = run(
            "r2",
            vec![
                case("case-aaaaaaaaaaaa", CaseStatus::Pass),
                case("case-bbbbbbbbbbbb", CaseStatus::Pass),
            ],
        );
        // A strict subset of the newest run: nothing left to contribute.
        let older = run("r1", vec![case("case-aaaaaaaaaaaa", CaseStatus::Fail)]);
        let merged = merge_branch_runs("main", vec![newest, older]).unwrap();
        let composite = merged.composite.as_ref().expect("composite provenance");
        let ids: Vec<&str> = composite
            .contributing_run_ids
            .iter()
            .map(|r| r.as_str())
            .collect();
        assert_eq!(ids, vec!["r2"]);
        assert_eq!(composite.branch, "main");
        assert_eq!(composite.truncated, None);
    }

    #[test]
    fn the_skeleton_comes_from_the_newest_contributor() {
        let newest = run("r9", vec![case("case-aaaaaaaaaaaa", CaseStatus::Pass)]);
        let older = run("r1", vec![case("case-bbbbbbbbbbbb", CaseStatus::Pass)]);
        let merged = merge_branch_runs("main", vec![newest, older]).unwrap();
        assert_eq!(merged.project.as_deref(), Some("proj"));
        assert_eq!(merged.suite.as_deref(), Some("suite"));
        assert_eq!(merged.config_digest, "digest-r9");
        assert!(
            merged.run_id.as_str().starts_with("composite-main-"),
            "synthetic id, never a real run's: got {}",
            merged.run_id.as_str()
        );
        let git = merged.git.as_ref().expect("git metadata");
        assert_eq!(git.branch.as_deref(), Some("main"));
        // A composite spans commits; claiming one would mislead.
        assert_eq!(git.commit, None);
    }

    #[test]
    fn a_capped_walk_that_was_still_adding_cases_is_marked_truncated() {
        // One unique case per run: every run inside the window contributes, and
        // the one past it is cut off.
        let runs: Vec<RunResult> = (0..BRANCH_LOOKBACK + 1)
            .map(|i| {
                run(
                    &format!("r{i}"),
                    vec![case(&format!("case-{i:012}"), CaseStatus::Pass)],
                )
            })
            .collect();
        let merged = merge_branch_runs("main", runs).unwrap();
        assert_eq!(merged.cases.len(), BRANCH_LOOKBACK);
        let composite = merged.composite.as_ref().expect("composite provenance");
        assert_eq!(composite.truncated, Some(true));
        assert!(
            !merged
                .cases
                .iter()
                .any(|c| c.case_key.as_str() == format!("case-{:012}", BRANCH_LOOKBACK)),
            "the run past the lookback horizon must not contribute"
        );
    }
}
