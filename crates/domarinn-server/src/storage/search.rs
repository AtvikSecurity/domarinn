//! Full-text search over runs and cases (FTS5).
//!
//! Two plain FTS5 tables (migration 5) mirror the searchable text:
//! * `runs_fts` — one row per run: project, suite, branch, commit,
//!   description, tags.
//! * `cases_fts` — one row per case: name, rendered-prompt text, output,
//!   provider error, tags.
//!
//! They are written in the same transaction as ingest (`PreparedRun::insert`),
//! deleted alongside the run (`delete_run` — FTS tables have no FK cascade),
//! and backfilled on startup for databases that predate the migration
//! ([`backfill`], driven from `storage::backfill::run`). Prompt and error text
//! exist only inside the compressed `cases.detail` blobs, which is why the
//! tables are not external-content over `cases`.

use anyhow::Context;
use rusqlite::{params, Connection, TransactionBehavior};

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::{CaseResult, CaseStatus};
use domarinn_core::types::RenderedPrompt;

use super::{decompress, ms_to_rfc3339, Storage};
use crate::dto::search::{
    CaseSearchHit, RunSearchHit, SearchResponse, SNIPPET_CLOSE, SNIPPET_OPEN,
};
use crate::runsets::{visibility_predicate, RunVisibility};

/// Snippet length passed to FTS5 `snippet()` (in tokens).
const SNIPPET_TOKENS: i64 = 12;

impl Storage {
    /// Grouped full-text hits for a user query, each group ranked by bm25.
    /// A query with no indexable tokens returns empty groups.
    pub async fn search(
        &self,
        query: String,
        limit: i64,
        vis: RunVisibility,
    ) -> anyhow::Result<SearchResponse> {
        self.runs
            .read(move |conn| {
                let Some(fts) = fts_match_query(&query) else {
                    return Ok(SearchResponse {
                        runs: Vec::new(),
                        cases: Vec::new(),
                    });
                };
                Ok(SearchResponse {
                    runs: run_hits(conn, &fts, limit, &vis)?,
                    cases: case_hits(conn, &fts, limit, &vis)?,
                })
            })
            .await
    }
}

/// Build an FTS5 `MATCH` expression from raw user input: each whitespace
/// token becomes a quoted phrase with prefix matching (`"tok"*`), joined by
/// implicit AND. Quoting neutralizes FTS5 operators (`AND`, `NEAR`, parens);
/// embedded double quotes are stripped since they cannot be escaped inside a
/// phrase. Tokens with no alphanumeric characters are dropped — quoted, they
/// tokenize to an empty phrase, which FTS5 rejects as a syntax error.
fn fts_match_query(input: &str) -> Option<String> {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .map(|t| format!("\"{t}\"*"))
        .collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(" "))
    }
}

fn run_hits(
    conn: &Connection,
    fts: &str,
    limit: i64,
    vis: &RunVisibility,
) -> anyhow::Result<Vec<RunSearchHit>> {
    // The FTS tables index every run; the join back to `runs` is where access
    // is decided, so a restricted run never surfaces as a hit or a snippet.
    let mut args: Vec<rusqlite::types::Value> = vec![
        fts.to_string().into(),
        SNIPPET_OPEN.to_string().into(),
        SNIPPET_CLOSE.to_string().into(),
        SNIPPET_TOKENS.into(),
        limit.into(),
    ];
    let visible = visibility_predicate("r", vis, &mut args);
    let sql = format!(
        "SELECT runs_fts.run_id, r.project, r.suite, r.created_at,
                snippet(runs_fts, -1, ?2, ?3, '…', ?4)
         FROM runs_fts
         JOIN runs r ON r.id = runs_fts.run_id
         WHERE runs_fts MATCH ?1 AND {visible}
         ORDER BY rank
         LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
        Ok(RunSearchHit {
            id: RunId::new(row.get::<_, String>(0)?),
            project: row.get(1)?,
            suite: row.get(2)?,
            created_at: ms_to_rfc3339(row.get(3)?),
            snippet: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn case_hits(
    conn: &Connection,
    fts: &str,
    limit: i64,
    vis: &RunVisibility,
) -> anyhow::Result<Vec<CaseSearchHit>> {
    let mut args: Vec<rusqlite::types::Value> = vec![
        fts.to_string().into(),
        SNIPPET_OPEN.to_string().into(),
        SNIPPET_CLOSE.to_string().into(),
        SNIPPET_TOKENS.into(),
        limit.into(),
    ];
    let visible = visibility_predicate("r", vis, &mut args);
    let sql = format!(
        "SELECT cases_fts.run_id, cases_fts.case_key, c.name, c.status,
                r.project, r.suite,
                snippet(cases_fts, -1, ?2, ?3, '…', ?4)
         FROM cases_fts
         JOIN cases c ON c.run_id = cases_fts.run_id AND c.case_key = cases_fts.case_key
         JOIN runs r ON r.id = cases_fts.run_id
         WHERE cases_fts MATCH ?1 AND {visible}
         ORDER BY rank
         LIMIT ?5"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |row| {
        Ok(CaseSearchHit {
            run_id: RunId::new(row.get::<_, String>(0)?),
            case_key: CaseKey::new(row.get::<_, String>(1)?),
            name: row.get(2)?,
            // The column is CHECK-constrained to the four statuses; Error
            // is an unreachable fallback, not a real decode path.
            status: row
                .get::<_, String>(3)?
                .parse()
                .unwrap_or(CaseStatus::Error),
            project: row.get(4)?,
            suite: row.get(5)?,
            snippet: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

// ---------------------------------------------------------------------------
// Index writers (shared by ingest and the startup backfill)
// ---------------------------------------------------------------------------

/// The searchable text of one run, borrowed from wherever the caller has it
/// (the prepared ingest row or the `runs` table during backfill).
pub(super) struct RunFtsRow<'a> {
    pub run_id: &'a str,
    pub project: Option<&'a str>,
    pub suite: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub commit: Option<&'a str>,
    pub description: Option<&'a str>,
    pub tags: &'a [String],
}

/// The searchable text of one case.
pub(super) struct CaseFtsRow<'a> {
    pub run_id: &'a str,
    pub case_key: &'a str,
    pub name: Option<&'a str>,
    pub prompt_text: Option<&'a str>,
    pub output_text: Option<&'a str>,
    pub error: Option<&'a str>,
    pub tags: &'a [String],
}

/// Insert the `runs_fts` row for one run inside the caller's transaction.
pub(super) fn index_run(
    tx: &rusqlite::Transaction<'_>,
    row: &RunFtsRow<'_>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO runs_fts (run_id, project, suite, branch, commit_sha, description, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.run_id,
            row.project.unwrap_or(""),
            row.suite.unwrap_or(""),
            row.branch.unwrap_or(""),
            row.commit.unwrap_or(""),
            row.description.unwrap_or(""),
            row.tags.join(" "),
        ],
    )?;
    Ok(())
}

/// Insert the `cases_fts` row for one case inside the caller's transaction.
pub(super) fn index_case(
    tx: &rusqlite::Transaction<'_>,
    row: &CaseFtsRow<'_>,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO cases_fts (run_id, case_key, name, prompt, output, error, tags)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.run_id,
            row.case_key,
            row.name.unwrap_or(""),
            row.prompt_text.unwrap_or(""),
            row.output_text.unwrap_or(""),
            row.error.unwrap_or(""),
            row.tags.join(" "),
        ],
    )?;
    Ok(())
}

/// Flatten a rendered prompt to indexable text: message contents joined with
/// newlines (roles carry no search value).
///
/// A turn's tool calls contribute their name and arguments, so searching for
/// `lookup_order` finds the case that replayed one. A turn with no calls
/// flattens to exactly the bytes it did before they were expressible — which
/// matters because the startup backfill only visits runs missing from
/// `runs_fts`, so an already-indexed run is never revisited and any change here
/// would otherwise leave old rows permanently inconsistent with new ones.
pub(super) fn flatten_prompt(prompt: &RenderedPrompt) -> String {
    match prompt {
        RenderedPrompt::Text(text) => text.clone(),
        RenderedPrompt::Messages(messages) => messages
            .iter()
            .map(|m| {
                let text = m.content.text();
                if m.tool_calls.is_empty() {
                    return text.into_owned();
                }
                let calls = m
                    .tool_calls
                    .iter()
                    .map(|c| format!("{} {}", c.name, c.arguments))
                    .collect::<Vec<_>>()
                    .join("\n");
                if text.is_empty() {
                    calls
                } else {
                    format!("{text}\n{calls}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

// ---------------------------------------------------------------------------
// Startup backfill
// ---------------------------------------------------------------------------

/// Runs indexed per chunk (each chunk decompresses every case blob of its
/// runs, so chunks are kept small like the migration-3 run backfill).
const RUN_CHUNK: i64 = 50;

/// Index runs that have no `runs_fts` row — i.e. rows ingested before the FTS
/// migration. Idempotent: freshly-ingested runs are indexed by ingest itself,
/// so the driving predicate selects nothing on a current database. A case
/// whose blob cannot be decoded is indexed from its promoted columns alone
/// (name/output/tags) rather than skipped, so the run never rescans.
pub(super) fn backfill(conn: &mut Connection) -> anyhow::Result<()> {
    loop {
        let chunk: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT id FROM runs
                 WHERE id NOT IN (SELECT run_id FROM runs_fts)
                 LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![RUN_CHUNK], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if chunk.is_empty() {
            break;
        }
        let n = chunk.len();

        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for run_id in chunk {
            backfill_one_run(&tx, &run_id)
                .with_context(|| format!("backfilling search index for run {run_id}"))?;
        }
        tx.commit()?;
        tracing::debug!(runs = n, "backfill: indexed search chunk");
    }
    Ok(())
}

fn backfill_one_run(tx: &rusqlite::Transaction<'_>, run_id: &str) -> anyhow::Result<()> {
    let (project, suite, branch, commit, description) = tx.query_row(
        "SELECT project, suite, git_branch, git_commit, description FROM runs WHERE id = ?1",
        params![run_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    let run_tags: Vec<String> = {
        let mut stmt = tx.prepare("SELECT tag FROM run_tags WHERE run_id = ?1 ORDER BY tag")?;
        let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    index_run(
        tx,
        &RunFtsRow {
            run_id,
            project: project.as_deref(),
            suite: suite.as_deref(),
            branch: branch.as_deref(),
            commit: commit.as_deref(),
            description: description.as_deref(),
            tags: &run_tags,
        },
    )?;

    struct StoredCase {
        case_key: String,
        name: Option<String>,
        output_text: Option<String>,
        detail: Option<Vec<u8>>,
    }
    let cases: Vec<StoredCase> = {
        let mut stmt =
            tx.prepare("SELECT case_key, name, output_text, detail FROM cases WHERE run_id = ?1")?;
        let rows = stmt.query_map(params![run_id], |row| {
            Ok(StoredCase {
                case_key: row.get(0)?,
                name: row.get(1)?,
                output_text: row.get(2)?,
                detail: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for case in cases {
        let decoded: Option<CaseResult> = case
            .detail
            .as_deref()
            .and_then(|blob| decompress(blob).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        if decoded.is_none() {
            tracing::warn!(
                run_id,
                case_key = case.case_key,
                "search backfill: undecodable case detail; indexing promoted columns only"
            );
        }
        let prompt_text = decoded
            .as_ref()
            .and_then(|c| c.prompt.as_ref())
            .map(flatten_prompt);
        let error = decoded.as_ref().and_then(|c| c.error.clone());
        let tags = decoded.map(|c| c.tags).unwrap_or_default();
        index_case(
            tx,
            &CaseFtsRow {
                run_id,
                case_key: &case.case_key,
                name: case.name.as_deref(),
                prompt_text: prompt_text.as_deref(),
                output_text: case.output_text.as_deref(),
                error: error.as_deref(),
                tags: &tags,
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::fts_match_query;

    #[test]
    fn tokens_become_quoted_prefix_phrases_joined_by_implicit_and() {
        assert_eq!(
            fts_match_query("empty cart").as_deref(),
            Some("\"empty\"* \"cart\"*")
        );
    }

    #[test]
    fn operators_and_quotes_are_neutralized() {
        assert_eq!(fts_match_query("AND").as_deref(), Some("\"AND\"*"));
        assert_eq!(
            fts_match_query("a\"b OR(c").as_deref(),
            Some("\"ab\"* \"OR(c\"*")
        );
    }

    #[test]
    fn queries_with_no_indexable_tokens_are_rejected() {
        assert_eq!(fts_match_query(""), None);
        assert_eq!(fts_match_query("   "), None);
        assert_eq!(fts_match_query("-- ((( \"\""), None);
    }
}
