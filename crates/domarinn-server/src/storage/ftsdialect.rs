//! The one genuinely dialect-divergent surface: full-text search.
//!
//! SQLite has FTS5 virtual tables (`MATCH`, `snippet()`, bm25 `rank`);
//! Postgres has `tsvector`/`tsquery` (`@@`, `ts_headline`, `ts_rank_cd`)
//! over the mirror tables in `schema_pg`, generated with the `'simple'`
//! configuration — no stemming, the closest analogue to FTS5's unicode61
//! tokenizer. The *write* paths stay shared (the mirror tables accept the
//! same INSERTs); only query construction branches, and all of it branches
//! here so the query modules stay dialect-blind.
//!
//! Documented approximate-parity (visible, not silent): hit *sets* match,
//! but ranking differs (bm25 weighs corpus-wide rarity, `ts_rank_cd` does
//! not), snippets pick slightly different windows (FTS5 auto-selects the
//! best column; `ts_headline` works over the concatenated document), and
//! `'simple'` keeps diacritics that unicode61 strips.

use super::exec::Dialect;

/// A sanitized user query: bare tokens plus whether they prefix-match.
/// Constructed only by [`search_query`]/[`cache_query`] so every renderer
/// sees input that cannot be a syntax error in either engine.
pub(super) struct FtsQuery {
    tokens: Vec<String>,
    prefix: bool,
}

/// The run/case search box: each whitespace token becomes a prefix term,
/// joined by implicit AND. Double quotes are stripped (FTS5 cannot escape
/// them inside a phrase); tokens with no alphanumeric characters are dropped
/// (quoted, they tokenize to an empty phrase, which FTS5 rejects).
pub(super) fn search_query(input: &str) -> Option<FtsQuery> {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', ""))
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect();
    (!tokens.is_empty()).then_some(FtsQuery {
        tokens,
        prefix: true,
    })
}

/// The cache browser's search box: split on every non-alphanumeric character
/// (so tokens are plain words), exact terms, implicit AND.
pub(super) fn cache_query(input: &str) -> Option<FtsQuery> {
    let tokens: Vec<String> = input
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect();
    (!tokens.is_empty()).then_some(FtsQuery {
        tokens,
        prefix: false,
    })
}

impl FtsQuery {
    /// The string bound to the match parameter: an FTS5 `MATCH` expression,
    /// or `to_tsquery('simple', …)` input.
    ///
    /// FTS5: `"tok"*` quoted-phrase prefix terms, space-joined (implicit
    /// AND); quoting neutralizes FTS5 operators (`AND`, `NEAR`, parens).
    /// Postgres: `'tok'` quoted lexemes joined with ` & `; quoting makes a
    /// multi-lexeme token a phrase, mirroring FTS5's quoted-phrase behavior.
    /// ASCII punctuation inside a token becomes spaces first — the same split
    /// `schema_pg`'s generated tsvectors apply to the document side — so
    /// `feature/fts-search` matches as the phrase FTS5's unicode61 tokenizer
    /// would produce instead of the single glued lexeme Postgres's parser
    /// makes of it. That substitution also removes every single quote, so no
    /// tsquery quote-doubling is left to do.
    pub fn match_arg(&self, dialect: Dialect) -> String {
        match dialect {
            Dialect::Sqlite => {
                let star = if self.prefix { "*" } else { "" };
                self.tokens
                    .iter()
                    .map(|t| format!("\"{t}\"{star}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Dialect::Postgres => {
                let star = if self.prefix { ":*" } else { "" };
                self.tokens
                    .iter()
                    .map(|t| {
                        // Never empty after trim: every token kept by the
                        // sanitizers has an alphanumeric char, and those are
                        // exactly what this substitution preserves.
                        let words: String = t
                            .chars()
                            .map(|c| {
                                if c.is_ascii() && !c.is_ascii_alphanumeric() {
                                    ' '
                                } else {
                                    c
                                }
                            })
                            .collect();
                        format!("'{}'{star}", words.trim())
                    })
                    .collect::<Vec<_>>()
                    .join(" & ")
            }
        }
    }
}

/// `WHERE` fragment matching `table` against the query bound at `?{param}`.
pub(super) fn match_predicate(dialect: Dialect, table: &str, param: usize) -> String {
    match dialect {
        Dialect::Sqlite => format!("{table} MATCH ?{param}"),
        Dialect::Postgres => format!("{table}.tsv @@ to_tsquery('simple', ?{param})"),
    }
}

/// SELECT-list fragment producing the highlighted excerpt.
///
/// `doc_cols` are the table's text columns in indexing order; FTS5 ignores
/// them (`snippet(t, -1, …)` auto-picks the best column), Postgres headlines
/// over their concatenation. The `\u{e000}`/`\u{e001}` markers and the
/// 12-token window are inlined — they are compile-time constants, and
/// keeping them out of the bind list lets both dialects share one layout
/// (`?{query_param}` = match arg, reused by every fragment).
pub(super) fn snippet_expr(
    dialect: Dialect,
    table: &str,
    doc_cols: &[&str],
    query_param: usize,
) -> String {
    use crate::dto::search::{SNIPPET_CLOSE, SNIPPET_OPEN};
    match dialect {
        Dialect::Sqlite => {
            format!("snippet({table}, -1, '{SNIPPET_OPEN}', '{SNIPPET_CLOSE}', '…', 12)")
        }
        Dialect::Postgres => {
            let doc = doc_cols
                .iter()
                .map(|c| format!("{table}.{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "ts_headline('simple', concat_ws(' ', {doc}), \
                 to_tsquery('simple', ?{query_param}), \
                 'StartSel={SNIPPET_OPEN}, StopSel={SNIPPET_CLOSE}, MaxWords=12, MinWords=5')"
            )
        }
    }
}

/// `ORDER BY` expression putting the best hit first.
pub(super) fn rank_order(dialect: Dialect, table: &str, query_param: usize) -> String {
    match dialect {
        // FTS5's `rank` is bm25, ascending = best first.
        Dialect::Sqlite => "rank".to_owned(),
        Dialect::Postgres => {
            format!("ts_rank_cd({table}.tsv, to_tsquery('simple', ?{query_param})) DESC")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_match_arg_matches_the_legacy_sanitizers() {
        // Pins byte-compatibility with the pre-dialect fts_match_query /
        // fts_query output, which the FTS5 corpus was built against.
        assert_eq!(
            search_query("empty cart")
                .unwrap()
                .match_arg(Dialect::Sqlite),
            "\"empty\"* \"cart\"*"
        );
        assert_eq!(
            search_query("a\"b OR(c")
                .unwrap()
                .match_arg(Dialect::Sqlite),
            "\"ab\"* \"OR(c\"*"
        );
        assert_eq!(
            cache_query("refund policy")
                .unwrap()
                .match_arg(Dialect::Sqlite),
            "\"refund\" \"policy\""
        );
        assert_eq!(
            cache_query("NEAR(").unwrap().match_arg(Dialect::Sqlite),
            "\"NEAR\""
        );
    }

    #[test]
    fn no_indexable_tokens_is_none() {
        assert!(search_query("").is_none());
        assert!(search_query("   ").is_none());
        assert!(search_query("-- ((( \"\"").is_none());
        assert!(cache_query("*").is_none());
        assert!(cache_query("   ").is_none());
    }

    #[test]
    fn postgres_match_arg_is_valid_tsquery_input() {
        assert_eq!(
            search_query("empty cart")
                .unwrap()
                .match_arg(Dialect::Postgres),
            "'empty':* & 'cart':*"
        );
        // ASCII punctuation splits into a phrase, exactly as unicode61 splits
        // the same token on SQLite ("don" + "t", "feature" + "fts" + "search").
        assert_eq!(
            search_query("don't").unwrap().match_arg(Dialect::Postgres),
            "'don t':*"
        );
        assert_eq!(
            search_query("feature/fts-search")
                .unwrap()
                .match_arg(Dialect::Postgres),
            "'feature fts search':*"
        );
        assert_eq!(
            cache_query("refund policy")
                .unwrap()
                .match_arg(Dialect::Postgres),
            "'refund' & 'policy'"
        );
    }
}
