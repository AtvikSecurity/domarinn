//! `view --case <SEL>`: full per-case detail.
//!
//! Selection is tiered ([`select`]) so one selector resolves the most specific
//! thing it can — an exact case key beats a key prefix beats a test id beats a
//! name substring — and a single test id deliberately fans out to every provider
//! that ran it (a cross-provider view). [`render_case_detail`] then prints
//! everything a case carries with no truncation: this command exists to show the
//! whole record, unlike the summarizing table.

use std::collections::HashSet;

use domarinn_core::result::{AssertResult, AssertStatus, CaseResult, CaseStatus, RunResult};
use domarinn_core::types::{Output, RenderedPrompt};

use crate::output::{colored_glyph, display_name, status_glyph};
use crate::style::Palette;

/// The column the bracketed case key is aligned to in a detail header, so a
/// batch of cases (e.g. one test id across providers) lines its keys up.
const KEY_COL: usize = 46;

/// Cases matching `selector`, using tiered precedence — the first tier that
/// yields any match wins, and every case in that tier is returned:
///
/// 1. exact `case_key`
/// 2. `case_key` prefix (only when the selector is at least 4 chars)
/// 3. exact `cell.test_id` (fans out across providers/repeats)
/// 4. case-insensitive substring of `name`
///
/// An empty selector matches nothing (it would otherwise substring-match every
/// named case).
pub fn select<'a>(run: &'a RunResult, selector: &str) -> Vec<&'a CaseResult> {
    if selector.is_empty() {
        return Vec::new();
    }

    // Tier 1: exact case key.
    let exact: Vec<&CaseResult> = run
        .cases
        .iter()
        .filter(|c| c.case_key.as_str() == selector)
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    // Tier 2: case-key prefix, but only for selectors long enough to be a
    // deliberate abbreviation rather than an accidental short string.
    if selector.chars().count() >= 4 {
        let prefixed: Vec<&CaseResult> = run
            .cases
            .iter()
            .filter(|c| c.case_key.as_str().starts_with(selector))
            .collect();
        if !prefixed.is_empty() {
            return prefixed;
        }
    }

    // Tier 3: exact test id.
    let by_test: Vec<&CaseResult> = run
        .cases
        .iter()
        .filter(|c| c.cell.test_id == selector)
        .collect();
    if !by_test.is_empty() {
        return by_test;
    }

    // Tier 4: case-insensitive substring of the display name.
    let needle = selector.to_lowercase();
    run.cases
        .iter()
        .filter(|c| {
            c.name
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains(&needle))
        })
        .collect()
}

/// The union of every selector's matches, deduplicated by case key and returned
/// in the run's own case order. Empty when nothing matched (the caller then
/// offers [`suggestions`]).
pub fn select_union<'a>(run: &'a RunResult, selectors: &[String]) -> Vec<&'a CaseResult> {
    let mut keys: HashSet<&str> = HashSet::new();
    for sel in selectors {
        for case in select(run, sel) {
            keys.insert(case.case_key.as_str());
        }
    }
    // Iterating `run.cases` (not the match order) gives run-order + free dedup.
    run.cases
        .iter()
        .filter(|c| keys.contains(c.case_key.as_str()))
        .collect()
}

/// Up to five candidate cases closest to the failed selectors, most-relevant
/// first, as `"<name>  [<case_key>]"` lines. Scoring is dependency-free: exact,
/// prefix, and substring hits on the key/test-id/name, then shared-prefix
/// length as a tiebreak.
pub fn suggestions(run: &RunResult, selectors: &[String]) -> Vec<String> {
    let mut scored: Vec<(u32, String)> = run
        .cases
        .iter()
        .map(|c| {
            let best = selectors
                .iter()
                .map(|s| case_score(c, s))
                .max()
                .unwrap_or(0);
            (best, format!("{}  [{}]", display_name(c), c.case_key))
        })
        .collect();
    // Stable sort by descending score keeps run order among equal-scoring cases.
    scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
    scored.into_iter().take(5).map(|(_, label)| label).collect()
}

/// The best match score of any of a case's identifiers against `selector`.
fn case_score(case: &CaseResult, selector: &str) -> u32 {
    [
        Some(case.case_key.as_str()),
        Some(case.cell.test_id.as_str()),
        case.name.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(|cand| score(cand, selector))
    .max()
    .unwrap_or(0)
}

/// A crude relevance score of `candidate` against `selector` (case-insensitive):
/// exact 100, prefix 80, substring 60, reverse-substring 40, else the shared
/// leading-char count.
fn score(candidate: &str, selector: &str) -> u32 {
    if selector.is_empty() {
        return 0;
    }
    let c = candidate.to_lowercase();
    let s = selector.to_lowercase();
    if c == s {
        100
    } else if c.starts_with(&s) {
        80
    } else if c.contains(&s) {
        60
    } else if s.contains(&c) {
        40
    } else {
        c.chars().zip(s.chars()).take_while(|(a, b)| a == b).count() as u32
    }
}

/// Render one case's full detail. Status glyphs are colored via `palette` and
/// section labels dimmed; the raw section prints only when `show_raw`, and then
/// notes `not recorded` if the run predates stored raw metadata.
pub fn render_case_detail(case: &CaseResult, palette: &Palette, show_raw: bool) -> String {
    let mut out = String::new();

    // Header: colored glyph, name, right-aligned bracketed case key.
    let name = display_name(case);
    let plain_left = format!("{} {name}", status_glyph(case.status));
    let pad = KEY_COL.saturating_sub(plain_left.chars().count()).max(2);
    out.push_str(&format!(
        "{} {name}{}[{}]\n",
        colored_glyph(palette, case.status),
        " ".repeat(pad),
        case.case_key,
    ));

    // Identity line: the cell coordinates plus attempt/cache flags.
    let mut ident = format!("provider {}", case.cell.provider_id);
    if let Some(prompt_id) = &case.cell.prompt_id {
        ident.push_str(&format!(" · prompt {prompt_id}"));
    }
    ident.push_str(&format!(" · test {}", case.cell.test_id));
    // The model the provider *reported*, which is the point: when a suite pins
    // a floating alias, this is the only place the actual snapshot shows up.
    if let Some(model) = &case.model {
        ident.push_str(&format!(" · model {model}"));
    }
    ident.push_str(&format!(" · repeat {}", case.cell.repeat));
    ident.push_str(&format!(" · {}", pluralize(case.attempts, "attempt")));
    if case.cached {
        ident.push_str(" · cached");
    }
    out.push_str(&format!("  {ident}\n"));

    // Metrics line: score, latency, and the optional token/cost/stop segments.
    let mut metrics = format!("score {:.2} · {} ms", case.score, case.latency_ms);
    if let Some(usage) = &case.usage {
        metrics.push_str(&format!(
            " · {} in / {} out tokens",
            usage.input_tokens, usage.output_tokens
        ));
    }
    if let Some(cost) = case.cost_usd {
        metrics.push_str(&format!(" · ${cost:.4}"));
    }
    if let Some(stop) = &case.stop_reason {
        metrics.push_str(&format!(" · stop: {stop}"));
    }
    out.push_str(&format!("  {metrics}\n"));

    if !case.tags.is_empty() {
        out.push_str(&format!("  tags: {}\n", case.tags.join(", ")));
    }

    if let Some(prompt) = &case.prompt {
        out.push_str(&format!("  {}\n", palette.dim("prompt")));
        out.push_str(&render_prompt(prompt));
    }

    if !case.asserts.is_empty() {
        out.push_str(&format!("  {}\n", palette.dim("asserts")));
        out.push_str(&render_asserts(&case.asserts, palette));
    }

    if let Some(output) = &case.output {
        out.push_str(&format!("  {}\n", palette.dim("output")));
        out.push_str(&indent(&output_text(output), 4));
        out.push('\n');
    }

    if let Some(error) = &case.error {
        out.push_str(&render_error(error, palette));
    }

    // Not gated on `show_raw`: an errored case has no output and no raw
    // payload, so this is the only structured thing it carries. Hiding it
    // behind a flag would leave the reader with prose and nothing else.
    if let Some(details) = &case.error_details {
        out.push_str(&format!("  {}\n", palette.dim("error details")));
        out.push_str(&indent(&pretty_json(details), 4));
        out.push('\n');
    }

    if show_raw {
        match &case.raw {
            Some(raw) => {
                out.push_str(&format!("  {}\n", palette.dim("raw")));
                out.push_str(&indent(&pretty_json(raw), 4));
                out.push('\n');
            }
            None => out.push_str("  raw: not recorded\n"),
        }
    }

    out
}

/// The prompt section body: text prompts print verbatim (indented); message
/// prompts print one `[role] content` block each, continuation lines aligned.
fn render_prompt(prompt: &RenderedPrompt) -> String {
    let mut out = String::new();
    match prompt {
        RenderedPrompt::Text(text) => {
            out.push_str(&indent(text, 4));
            out.push('\n');
        }
        RenderedPrompt::Messages(messages) => {
            for msg in messages {
                let mut lines = msg.content.lines();
                let first = lines.next().unwrap_or("");
                out.push_str(&format!("    [{}] {first}\n", msg.role.as_str()));
                for line in lines {
                    out.push_str(&format!("    {line}\n"));
                }
            }
        }
    }
    out
}

/// The asserts section body: one glyph/kind/score/weight row per assertion, its
/// reason appended when present, and any `details` pretty-printed beneath.
fn render_asserts(asserts: &[AssertResult], palette: &Palette) -> String {
    let kind_w = asserts
        .iter()
        .map(|a| a.kind.as_str().len())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for a in asserts {
        let mut row = format!(
            "    {} {:<kind_w$}  {:.2} ×{}",
            colored_assert_glyph(palette, a.status),
            a.kind.as_str(),
            a.score,
            fmt_num(a.weight),
        );
        if !a.reason.is_empty() {
            row.push_str(&format!("  {}", a.reason));
        }
        out.push_str(&row);
        out.push('\n');
        if let Some(details) = &a.details {
            // Aligned past the 4-space indent + 4-char glyph + 1 space = 9.
            out.push_str("         details:\n");
            out.push_str(&indent(&pretty_json(details), 11));
            out.push('\n');
        }
    }
    out
}

/// The error line(s): `error: <msg>` with any continuation lines aligned under
/// the message.
fn render_error(error: &str, palette: &Palette) -> String {
    let mut out = String::new();
    let mut lines = error.lines();
    let first = lines.next().unwrap_or("");
    out.push_str(&format!("  {} {first}\n", palette.dim("error:")));
    for line in lines {
        out.push_str(&format!("         {line}\n"));
    }
    out
}

/// The status glyph for an assertion (4-byte token, matching the case glyphs).
fn assert_glyph(status: AssertStatus) -> &'static str {
    match status {
        AssertStatus::Pass => "PASS",
        AssertStatus::Fail => "FAIL",
        AssertStatus::Error => "ERR ",
        AssertStatus::Skipped => "SKIP",
    }
}

/// The assert glyph colored per status; the token bytes survive inside the
/// escapes so `contains("PASS")` still matches.
fn colored_assert_glyph(palette: &Palette, status: AssertStatus) -> String {
    let glyph = assert_glyph(status);
    match status {
        AssertStatus::Pass => palette.pass(glyph),
        AssertStatus::Fail => palette.fail(glyph),
        AssertStatus::Error => palette.error(glyph),
        AssertStatus::Skipped => palette.skip(glyph),
    }
}

/// Text output verbatim; JSON output pretty-printed (this view never truncates).
fn output_text(output: &Output) -> String {
    match output {
        Output::Text(s) => s.clone(),
        Output::Json(v) => pretty_json(v),
    }
}

fn pretty_json(value: &serde_json::Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
}

/// Prefix every line of `text` with `spaces` spaces.
fn indent(text: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    text.lines()
        .map(|line| format!("{pad}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `"1 attempt"` / `"3 attempts"` — a naive singular/plural for count nouns.
fn pluralize(n: u32, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// A weight/number rendered with a trailing `.0` for whole values (so `×1.0`,
/// not `×1`) while preserving the natural precision of fractional ones.
fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{n:.1}")
    } else {
        format!("{n}")
    }
}

/// Whether a case is a failure or infrastructure error (the `--failed` filter).
pub fn is_failed(case: &CaseResult) -> bool {
    matches!(case.status, CaseStatus::Fail | CaseStatus::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use domarinn_core::asserts::AssertName;
    use domarinn_core::ids::RunId;
    use domarinn_core::result::{CellKey, RunSummary, RESULT_SCHEMA_VERSION};
    use domarinn_core::types::{ChatMessage, ChatRole, TokenUsage};

    fn cell(provider: &str, test: &str, repeat: u32) -> CellKey {
        CellKey {
            provider_id: provider.into(),
            prompt_id: None,
            test_id: test.into(),
            repeat,
        }
    }

    /// A bare case with the given cell; callers mutate the fields they test.
    fn case(cell: CellKey, status: CaseStatus) -> CaseResult {
        CaseResult {
            case_key: cell.case_key(),
            cell,
            name: None,
            tags: vec![],
            vars: Default::default(),
            status,
            score: 0.0,
            output: None,
            prompt: None,
            request: None,
            stop_reason: None,
            model: None,
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
            error_class: None,
        }
    }

    fn run_of(cases: Vec<CaseResult>) -> RunResult {
        let now = Utc::now();
        RunResult {
            schema_version: RESULT_SCHEMA_VERSION,
            run_id: RunId::new("r"),
            project: None,
            suite: Some("s".into()),
            started_at: now,
            finished_at: now,
            config_digest: "d".into(),
            config_snapshot: serde_json::json!({}),
            git: None,
            ci: None,
            digests: None,
            origin: None,
            share_url: None,
            filters: Default::default(),
            cases,
            summary: RunSummary::default(),
        }
    }

    /// Tier 1 (exact key) wins even when the same selector would also match a
    /// prefix, a test id, and a name substring on other cases.
    #[test]
    fn exact_key_beats_lower_tiers() {
        // Craft a run where `target`'s full key is also a prefix/test-id/name of
        // siblings would be contrived; instead assert the exact-key case is the
        // sole match and lower-tier siblings are excluded.
        let target = {
            let mut c = case(cell("p", "alpha", 0), CaseStatus::Pass);
            c.name = Some("alpha".into());
            c
        };
        let key = target.case_key.as_str().to_string();
        // A sibling whose test id equals the *key* string — tier 3 would match it,
        // but tier 1 short-circuits first.
        let mut sibling = case(cell("q", &key, 0), CaseStatus::Pass);
        sibling.name = Some(key.clone());
        let run = run_of(vec![target.clone(), sibling]);

        let matched = select(&run, &key);
        assert_eq!(matched.len(), 1, "only the exact-key case matches");
        assert_eq!(matched[0].case_key.as_str(), key);
    }

    /// A key prefix (tier 2) resolves before an unrelated test-id / name would,
    /// and matches every case sharing that key prefix.
    #[test]
    fn prefix_matches_before_test_id_and_needs_four_chars() {
        let a = case(cell("p", "t", 0), CaseStatus::Pass);
        let prefix: String = a.case_key.as_str().chars().take(6).collect();
        let run = run_of(vec![a.clone()]);

        // >= 4 chars → prefix match.
        let matched = select(&run, &prefix);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].case_key, a.case_key);

        // A 3-char slice must NOT tier-2 match, even though it is a real prefix.
        let short: String = a.case_key.as_str().chars().take(3).collect();
        assert!(
            select(&run, &short).is_empty(),
            "prefix shorter than 4 chars does not match"
        );
    }

    /// Tier 2 (key prefix) wins over tier 3 (test id): a selector that both
    /// prefixes one case's key and exactly equals another case's test id resolves
    /// to the prefix match only.
    #[test]
    fn prefix_beats_test_id_across_cases() {
        let a = case(cell("p", "ta", 0), CaseStatus::Pass);
        let prefix: String = a.case_key.as_str().chars().take(6).collect();
        // Case b's *test id* is exactly that key prefix — a tier-3 candidate.
        let b = case(cell("q", &prefix, 0), CaseStatus::Fail);
        let run = run_of(vec![a.clone(), b]);

        let matched = select(&run, &prefix);
        assert_eq!(
            matched.len(),
            1,
            "only the prefix (tier 2) match, not tier 3"
        );
        assert_eq!(matched[0].case_key, a.case_key);
    }

    /// Tier 3 (exact test id) wins over tier 4 (name substring): a selector equal
    /// to one case's test id and merely a substring of another's name resolves to
    /// the test-id match only.
    #[test]
    fn test_id_beats_name_substring() {
        let by_id = case(cell("p", "greet", 0), CaseStatus::Pass);
        let mut by_name = case(cell("q", "other", 0), CaseStatus::Fail);
        by_name.name = Some("a greet-ish name".into());
        let run = run_of(vec![by_id.clone(), by_name]);

        let matched = select(&run, "greet");
        assert_eq!(matched.len(), 1, "only the exact-test-id (tier 3) match");
        assert_eq!(matched[0].cell.test_id, "greet");
    }

    /// A short selector that is not a key/test-id still resolves via tier 4
    /// (name substring), confirming tier 2 was correctly skipped rather than
    /// short-circuiting the whole search.
    #[test]
    fn short_selector_falls_through_to_name_substring() {
        let mut c = case(cell("p", "the-greeting-test", 0), CaseStatus::Pass);
        c.name = Some("Greet".into());
        let run = run_of(vec![c]);
        // "ree" is 3 chars: too short for tier 2, not an exact test id, but a
        // case-insensitive substring of the name "Greet".
        let matched = select(&run, "ree");
        assert_eq!(matched.len(), 1);
    }

    /// A test id shared by several providers (tier 3) returns all of them — the
    /// cross-provider view.
    #[test]
    fn test_id_fans_out_across_providers() {
        let run = run_of(vec![
            case(cell("p1", "greet", 0), CaseStatus::Pass),
            case(cell("p2", "greet", 0), CaseStatus::Fail),
            case(cell("p3", "other", 0), CaseStatus::Pass),
        ]);
        let matched = select(&run, "greet");
        assert_eq!(matched.len(), 2);
        assert!(matched.iter().all(|c| c.cell.test_id == "greet"));
    }

    #[test]
    fn name_substring_is_case_insensitive() {
        let mut c = case(cell("p", "t", 0), CaseStatus::Pass);
        c.name = Some("Polite Apology".into());
        let run = run_of(vec![c]);
        assert_eq!(select(&run, "apology").len(), 1);
        assert_eq!(select(&run, "POLITE").len(), 1);
        assert!(select(&run, "rude").is_empty());
    }

    #[test]
    fn empty_selector_matches_nothing() {
        let mut c = case(cell("p", "t", 0), CaseStatus::Pass);
        c.name = Some("anything".into());
        let run = run_of(vec![c]);
        assert!(select(&run, "").is_empty());
    }

    /// The union dedups a case reachable via two selectors and keeps run order.
    #[test]
    fn union_dedups_and_preserves_run_order() {
        let run = run_of(vec![
            case(cell("p1", "greet", 0), CaseStatus::Pass),
            case(cell("p2", "farewell", 0), CaseStatus::Fail),
            case(cell("p3", "greet", 0), CaseStatus::Pass),
        ]);
        // "greet" matches cases 0 and 2; its own key matches case 0 again.
        let key0 = run.cases[0].case_key.as_str().to_string();
        let selectors = vec!["greet".to_string(), key0];
        let union = select_union(&run, &selectors);
        assert_eq!(union.len(), 2, "case 0 counted once despite two selectors");
        // Run order: case 0 before case 2 (case 1 is farewell, unmatched).
        assert_eq!(union[0].cell.provider_id, "p1");
        assert_eq!(union[1].cell.provider_id, "p3");
    }

    #[test]
    fn suggestions_rank_closest_and_cap_at_five() {
        let cases: Vec<CaseResult> = (0..8)
            .map(|i| {
                let mut c = case(cell("p", &format!("test{i}"), 0), CaseStatus::Pass);
                c.name = Some(format!("name{i}"));
                c
            })
            .collect();
        // Give one case an exact test-id hit so it ranks first.
        let run = run_of(cases);
        let sugg = suggestions(&run, &["test3".to_string()]);
        assert_eq!(sugg.len(), 5, "capped at five candidates");
        // The exact test-id hit ranks first; its label carries the display name.
        assert!(sugg[0].contains("name3"), "the exact hit ranks first");
    }

    /// A synthetic v2 case renders every section: identity, metrics, prompt
    /// roles, assert kind/reason/details, output, error, and (under --raw) raw.
    #[test]
    fn detail_renders_all_v2_sections() {
        let mut c = case(cell("p", "greet", 0), CaseStatus::Fail);
        c.name = Some("greet".into());
        c.score = 0.33;
        c.latency_ms = 812;
        c.attempts = 2;
        c.cached = true;
        c.tags = vec!["smoke".into(), "polite".into()];
        c.usage = Some(TokenUsage {
            input_tokens: 123,
            output_tokens: 45,
            ..Default::default()
        });
        c.cost_usd = Some(0.0012);
        c.stop_reason = Some("end_turn".into());
        c.prompt = Some(RenderedPrompt::Messages(vec![
            ChatMessage {
                role: ChatRole::System,
                content: "You are helpful.".into(),
            },
            ChatMessage {
                role: ChatRole::User,
                content: "Say hi.".into(),
            },
        ]));
        c.output = Some(Output::Text("hello there".into()));
        c.error = Some("boom".into());
        c.raw = Some(serde_json::json!({"model": "x", "seq": 1}));
        c.asserts = vec![
            AssertResult {
                kind: AssertName::Contains,
                status: AssertStatus::Pass,
                score: 1.0,
                weight: 1.0,
                reason: String::new(),
                details: None,
                criteria: None,
                cached: false,
                cost_usd: None,
            },
            AssertResult {
                kind: AssertName::LlmRubric,
                status: AssertStatus::Fail,
                score: 0.2,
                weight: 2.0,
                reason: "response missed the apology".into(),
                details: Some(serde_json::json!({"pass": false})),
                criteria: None,
                cached: false,
                cost_usd: None,
            },
        ];

        let text = render_case_detail(&c, &Palette::disabled(), true);
        // Header + identity + metrics.
        assert!(text.contains("FAIL greet"));
        assert!(text.contains(&format!("[{}]", c.case_key)));
        assert!(text.contains("provider p · test greet · repeat 0 · 2 attempts · cached"));
        assert!(text
            .contains("score 0.33 · 812 ms · 123 in / 45 out tokens · $0.0012 · stop: end_turn"));
        assert!(text.contains("tags: smoke, polite"));
        // Prompt roles.
        assert!(text.contains("[system] You are helpful."));
        assert!(text.contains("[user] Say hi."));
        // Asserts: kind, score, weight, reason, details.
        assert!(text.contains("PASS contains"));
        assert!(text.contains("FAIL llm-rubric"));
        assert!(text.contains("×2.0"));
        assert!(text.contains("response missed the apology"));
        assert!(text.contains("details:"));
        assert!(text.contains("\"pass\": false"));
        // Output + error + raw.
        assert!(text.contains("hello there"));
        assert!(text.contains("error: boom"));
        assert!(text.contains("raw"));
        assert!(text.contains("\"model\": \"x\""));
        // No color escapes under a disabled palette.
        assert!(!text.contains('\x1b'));
    }

    /// A v1 case (no prompt/stop/raw) prints none of the v2 sections, and even
    /// with --raw only the `not recorded` note.
    #[test]
    fn detail_v1_case_omits_v2_sections() {
        let mut c = case(cell("p", "t", 0), CaseStatus::Pass);
        c.output = Some(Output::Text("out".into()));
        let text = render_case_detail(&c, &Palette::disabled(), true);
        assert!(!text.contains("prompt"), "no prompt section");
        assert!(!text.contains("stop:"), "no stop segment");
        assert!(!text.contains("tags:"), "no tags line when empty");
        assert!(
            text.contains("raw: not recorded"),
            "--raw on a v1 case notes it was not recorded"
        );
    }

    /// Without --raw, a case that *does* carry raw metadata still omits it.
    #[test]
    fn detail_hides_raw_without_flag() {
        let mut c = case(cell("p", "t", 0), CaseStatus::Pass);
        c.raw = Some(serde_json::json!({"k": "v"}));
        let text = render_case_detail(&c, &Palette::disabled(), false);
        assert!(!text.contains("raw"), "raw omitted without --raw");
    }

    #[test]
    fn detail_colors_glyph_but_keeps_token_bytes() {
        let c = case(cell("p", "t", 0), CaseStatus::Fail);
        let text = render_case_detail(&c, &Palette::for_test(true), false);
        assert!(text.contains('\x1b'), "enabled palette emits escapes");
        assert!(text.contains("FAIL"), "glyph token survives the escapes");
    }

    #[test]
    fn text_prompt_renders_plain_without_role_labels() {
        let mut c = case(cell("p", "t", 0), CaseStatus::Pass);
        c.prompt = Some(RenderedPrompt::Text("just a string".into()));
        let text = render_case_detail(&c, &Palette::disabled(), false);
        assert!(
            text.contains("    just a string"),
            "rendered plain + indented"
        );
        // Text prompts carry no `[role]` labels (the only brackets are the
        // header's `[case_key]`).
        assert!(!text.contains("[system]"));
        assert!(!text.contains("[user]"));
        assert!(!text.contains("[assistant]"));
    }

    #[test]
    fn fmt_num_keeps_a_decimal_for_whole_weights() {
        assert_eq!(fmt_num(1.0), "1.0");
        assert_eq!(fmt_num(2.0), "2.0");
        assert_eq!(fmt_num(0.5), "0.5");
        assert_eq!(fmt_num(0.25), "0.25");
    }

    #[test]
    fn pluralize_singular_and_plural() {
        assert_eq!(pluralize(1, "attempt"), "1 attempt");
        assert_eq!(pluralize(2, "attempt"), "2 attempts");
        assert_eq!(pluralize(0, "attempt"), "0 attempts");
    }
}
