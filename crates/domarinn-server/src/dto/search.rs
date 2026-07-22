//! DTOs for `GET /search` (full-text search over runs and cases).

use serde::Serialize;
use ts_rs::TS;

use domarinn_core::ids::{CaseKey, RunId};
use domarinn_core::result::CaseStatus;

/// Snippet match markers. FTS5's `snippet()` wraps each matched token between
/// these two private-use-area characters; the web UI splits on them to render
/// highlights. PUA characters are used because any printable delimiter (`<b>`,
/// `[`, `«`…) can legitimately occur in stored prompt/output text.
pub const SNIPPET_OPEN: &str = "\u{e000}";
pub const SNIPPET_CLOSE: &str = "\u{e001}";

/// One run whose metadata (project, suite, branch, commit, description, tags)
/// matched the query.
#[derive(Debug, Clone, Serialize, TS)]
pub struct RunSearchHit {
    pub id: RunId,
    pub project: Option<String>,
    pub suite: Option<String>,
    /// RFC3339.
    pub created_at: String,
    /// Matched-field excerpt with [`SNIPPET_OPEN`]/[`SNIPPET_CLOSE`] around
    /// each matched token.
    pub snippet: String,
}

/// One case whose text (name, prompt, output, error, tags) matched the query.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CaseSearchHit {
    pub run_id: RunId,
    pub case_key: CaseKey,
    pub name: Option<String>,
    pub status: CaseStatus,
    /// The owning run's project/suite, for display and grouping.
    pub project: Option<String>,
    pub suite: Option<String>,
    /// Matched-field excerpt with [`SNIPPET_OPEN`]/[`SNIPPET_CLOSE`] around
    /// each matched token.
    pub snippet: String,
}

/// `GET /search` response: matches grouped by kind, each ranked by bm25.
#[derive(Debug, Clone, Serialize, TS)]
pub struct SearchResponse {
    pub runs: Vec<RunSearchHit>,
    pub cases: Vec<CaseSearchHit>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_response_matches_todays_wire_shape() {
        let dto = SearchResponse {
            runs: vec![RunSearchHit {
                id: RunId::from("01AAA"),
                project: Some("checkout".to_string()),
                suite: None,
                created_at: "2026-01-01T00:00:30Z".to_string(),
                snippet: format!("branch {SNIPPET_OPEN}main{SNIPPET_CLOSE}"),
            }],
            cases: vec![CaseSearchHit {
                run_id: RunId::from("01AAA"),
                case_key: CaseKey::new("deadbeef"),
                name: Some("openai::t1".to_string()),
                status: CaseStatus::Pass,
                project: Some("checkout".to_string()),
                suite: Some("regression".to_string()),
                snippet: format!("hello {SNIPPET_OPEN}world{SNIPPET_CLOSE}"),
            }],
        };
        assert_eq!(
            serde_json::to_value(&dto).unwrap(),
            json!({
                "runs": [{
                    "id": "01AAA",
                    "project": "checkout",
                    "suite": null,
                    "created_at": "2026-01-01T00:00:30Z",
                    "snippet": "branch \u{e000}main\u{e001}",
                }],
                "cases": [{
                    "run_id": "01AAA",
                    "case_key": "deadbeef",
                    "name": "openai::t1",
                    "status": "pass",
                    "project": "checkout",
                    "suite": "regression",
                    "snippet": "hello \u{e000}world\u{e001}",
                }],
            })
        );
    }
}
