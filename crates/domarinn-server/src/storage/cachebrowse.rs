//! Reading the cache for a human: list, filter, search, inspect, and count.
//!
//! # Every query here runs on a reader
//!
//! `cache_get` is a *write* — it bumps `last_access_at` and the hit counter
//! inside an IMMEDIATE transaction. Nothing in this module may do either, for
//! two separate reasons:
//!
//! - `hits`/`misses` back the UI's lookup hit-rate tile. Paging a list through
//!   the counting read would add a hit per row and make the tile lie. This is
//!   `cache_has`'s argument taken one step further.
//! - `last_access_at` is what `cache_prune` evicts on. A browse that refreshed
//!   it would mean *looking at the cache changes what the cache evicts* — an
//!   admin skimming a page would silently rescue the coldest entries in the
//!   store from the next retention pass.
//!
//! Using [`super::Db::read`] makes that a property of the connection rather
//! than of discipline: a pooled reader cannot execute the `UPDATE` at all.

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use domarinn_core::cache::CacheEntry;

use super::cacheindex::clamp_empty_reason;
use super::{from_microusd, ms_to_rfc3339, Storage};
use crate::domain::{CacheSort, SortOrder};
use crate::dto::cacheentries::{
    CacheEntryDetail, CacheEntryListItem, CacheEntryListResponse, CacheFacet, CacheFacetsResponse,
};

/// Distinct models reported by `/cache/facets`.
///
/// Capped so a store polluted with pathological model strings cannot produce an
/// unbounded response. The dropdown wants the common ones; the free-text filter
/// covers the tail.
const MAX_MODEL_FACETS: i64 = 100;

/// The `kind=` pseudo-value for entries whose body has not been examined yet.
///
/// Needed because a real `kind` filter provably cannot match them — nothing has
/// been established about them — so without this they would be unreachable for
/// as long as the backfill runs, which is exactly when someone wants to look.
pub const KIND_UNINDEXED: &str = "unindexed";
/// The `kind=` pseudo-value for entries this server could not parse.
pub const KIND_UNPARSEABLE: &str = "unparseable";

/// Filters for `GET /cache/entries`.
#[derive(Debug, Clone)]
pub struct CacheListFilter {
    pub kind: Option<String>,
    pub model: Option<String>,
    /// Exactly one reason. Plural selection belongs to prune, which is a
    /// destructive operation people script; a browser narrows one facet at a
    /// time and the URL stays readable.
    pub empty_reason: Option<String>,
    pub q: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub min_cost_microusd: Option<i64>,
    pub max_cost_microusd: Option<i64>,
    pub sort: CacheSort,
    pub order: SortOrder,
    pub limit: i64,
    pub cursor: Option<(i64, String)>,
}

/// Cursor encodes `{sort_value}:{key}`.
///
/// Split on the **first** colon: a key is `sha256:<hex>` and contains one of
/// its own, so the run-list's `split_once` would decode the sort value as
/// `"sha256"` and lose the key entirely.
pub fn encode_entry_cursor(sort_value: i64, key: &str) -> String {
    format!("{sort_value}:{key}")
}

pub fn decode_entry_cursor(cursor: &str) -> Option<(i64, String)> {
    let (value, key) = cursor.split_once(':')?;
    Some((value.parse().ok()?, key.to_string()))
}

impl CacheSort {
    /// The column this sorts on. Every one is indexed.
    fn column(self) -> &'static str {
        match self {
            CacheSort::Created => "created_at",
            CacheSort::LastAccess => "last_access_at",
            CacheSort::Size => "size",
            CacheSort::Cost => "cost_microusd",
        }
    }

    /// Whether ordering by this column has to exclude rows where it is NULL.
    ///
    /// Only `cost_microusd` is nullable, and the exclusion is not cosmetic: the
    /// keyset predicate is NULL-unsafe, so once a page reached the NULL tail
    /// `cost < ?` would be NULL for every remaining row and pagination would
    /// stop dead with entries left unseen. Wrapping it in `IFNULL` fixes that
    /// and destroys the index — migration 11's lesson. Excluding is the honest
    /// option: ordering *by* an unknown value is meaningless in a way that
    /// filtering on one is not.
    fn requires_non_null(self) -> bool {
        matches!(self, CacheSort::Cost)
    }
}

/// Turn a user's search box into an fts5 query that cannot be a syntax error.
///
/// Users type quotes, stars and stray parens; fts5 answers those with an error,
/// and an error is the server's problem, not the user's. Each run of
/// alphanumerics becomes its own quoted phrase, so the result is an implicit
/// AND of literal terms with no operator surface at all. `None` means the input
/// contained nothing searchable — which matches no entry, rather than silently
/// matching every entry.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

impl Storage {
    pub async fn cache_list_entries(
        &self,
        filter: CacheListFilter,
    ) -> anyhow::Result<CacheEntryListResponse> {
        self.cache
            .read(move |conn| list_entries(conn, &filter))
            .await
    }

    pub async fn cache_entry_detail(
        &self,
        key: String,
        include_raw: bool,
    ) -> anyhow::Result<Option<CacheEntryDetail>> {
        self.cache
            .read(move |conn| entry_detail(conn, &key, include_raw))
            .await
    }

    pub async fn cache_facets(&self) -> anyhow::Result<CacheFacetsResponse> {
        self.cache.read(facets).await
    }
}

/// The columns a list row needs, in the order `row_to_item` reads them.
///
/// Hand-maintained and positional: adding one here means adding the matching
/// `row.get(n)` to [`row_to_item`], and appending rather than inserting is what
/// keeps every other index from shifting under it.
const LIST_COLUMNS: &str = "key, size, created_at, last_access_at, entry_created_at,
     indexed_at, index_ok, kind, model, cost_microusd,
     input_tokens, output_tokens, request_summary, output_preview,
     empty_reason";

fn list_entries(
    conn: &Connection,
    filter: &CacheListFilter,
) -> anyhow::Result<CacheEntryListResponse> {
    let mut sql = format!("SELECT {LIST_COLUMNS} FROM cache_entries WHERE 1=1");
    let mut args: Vec<SqlValue> = Vec::new();

    match filter.kind.as_deref() {
        // The two pseudo-values. Both name a state rather than a kind, and both
        // exist so the rows they name stay reachable while the backfill runs.
        Some(KIND_UNINDEXED) => sql.push_str(" AND indexed_at IS NULL"),
        Some(KIND_UNPARSEABLE) => sql.push_str(" AND index_ok = 0"),
        Some(kind) => {
            args.push(kind.to_string().into());
            sql.push_str(&format!(" AND kind = ?{}", args.len()));
        }
        None => {}
    }
    if let Some(model) = &filter.model {
        args.push(model.clone().into());
        sql.push_str(&format!(" AND model = ?{}", args.len()));
    }
    if let Some(reason) = &filter.empty_reason {
        args.push(reason.clone().into());
        sql.push_str(&format!(" AND empty_reason = ?{}", args.len()));
    }
    if let Some(since) = filter.since {
        args.push(since.into());
        sql.push_str(&format!(" AND created_at >= ?{}", args.len()));
    }
    if let Some(until) = filter.until {
        args.push(until.into());
        sql.push_str(&format!(" AND created_at <= ?{}", args.len()));
    }
    if let Some(min) = filter.min_cost_microusd {
        args.push(min.into());
        sql.push_str(&format!(" AND cost_microusd >= ?{}", args.len()));
    }
    if let Some(max) = filter.max_cost_microusd {
        args.push(max.into());
        sql.push_str(&format!(" AND cost_microusd <= ?{}", args.len()));
    }
    if let Some(q) = &filter.q {
        match fts_query(q) {
            Some(query) => {
                args.push(query.into());
                sql.push_str(&format!(
                    " AND rowid IN (SELECT rowid FROM cache_entries_fts \
                      WHERE cache_entries_fts MATCH ?{})",
                    args.len()
                ));
            }
            // Nothing searchable in the box: match nothing rather than
            // everything, so an accidental `q=***` does not read as "no filter".
            None => return Ok(empty_page()),
        }
    }

    let column = filter.sort.column();
    if filter.sort.requires_non_null() {
        sql.push_str(&format!(" AND {column} IS NOT NULL"));
    }

    if let Some((value, key)) = &filter.cursor {
        let comparison = match filter.order {
            SortOrder::Desc => '<',
            SortOrder::Asc => '>',
        };
        args.push((*value).into());
        let value_arg = args.len();
        args.push(key.clone().into());
        let key_arg = args.len();
        // Tie-break on the key so a page boundary landing inside a run of equal
        // sort values neither repeats nor skips.
        sql.push_str(&format!(
            " AND ({column} {comparison} ?{value_arg} \
              OR ({column} = ?{value_arg} AND key {comparison} ?{key_arg}))"
        ));
    }

    let direction = match filter.order {
        SortOrder::Desc => "DESC",
        SortOrder::Asc => "ASC",
    };
    // Fetch one extra to learn whether a next page exists without counting.
    args.push((filter.limit + 1).into());
    sql.push_str(&format!(
        " ORDER BY {column} {direction}, key {direction} LIMIT ?{}",
        args.len()
    ));

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
        Ok((
            row_to_item(row)?,
            row.get::<_, Option<i64>>(sort_index(filter.sort))?,
        ))
    })?;
    let mut fetched: Vec<(CacheEntryListItem, Option<i64>)> = rows.collect::<Result<_, _>>()?;

    let next_cursor = if fetched.len() as i64 > filter.limit {
        fetched.truncate(filter.limit as usize);
        fetched.last().map(|(item, sort_value)| {
            encode_entry_cursor(sort_value.unwrap_or_default(), &item.key)
        })
    } else {
        None
    };

    Ok(CacheEntryListResponse {
        entries: fetched.into_iter().map(|(item, _)| item).collect(),
        next_cursor,
        truncated: false,
    })
}

fn empty_page() -> CacheEntryListResponse {
    CacheEntryListResponse {
        entries: Vec::new(),
        next_cursor: None,
        truncated: false,
    }
}

/// Which selected column carries the sort value, so the cursor can be built
/// from the row already in hand rather than a second lookup.
fn sort_index(sort: CacheSort) -> usize {
    match sort {
        CacheSort::Created => 2,
        CacheSort::LastAccess => 3,
        CacheSort::Size => 1,
        CacheSort::Cost => 9,
    }
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<CacheEntryListItem> {
    let indexed_at: Option<i64> = row.get(5)?;
    let index_ok: Option<i64> = row.get(6)?;
    Ok(CacheEntryListItem {
        key: row.get(0)?,
        size: row.get(1)?,
        created_at: ms_to_rfc3339(row.get(2)?),
        last_access_at: row.get::<_, Option<i64>>(3)?.map(ms_to_rfc3339),
        entry_created_at: row.get::<_, Option<i64>>(4)?.map(ms_to_rfc3339),
        indexed: indexed_at.is_some(),
        parseable: index_ok.map(|ok| ok != 0),
        kind: row.get(7)?,
        model: row.get(8)?,
        cost_usd: from_microusd(row.get(9)?),
        input_tokens: row.get(10)?,
        output_tokens: row.get(11)?,
        request_summary: row.get(12)?,
        output_preview: row.get(13)?,
        empty_reason: row.get(14)?,
    })
}

/// What a detail lookup reads before it decides whether the body parses.
struct DetailRow {
    size: i64,
    created_at: i64,
    last_access_at: Option<i64>,
    indexed_at: Option<i64>,
    index_ok: Option<i64>,
    body: Vec<u8>,
}

fn entry_detail(
    conn: &Connection,
    key: &str,
    include_raw: bool,
) -> anyhow::Result<Option<CacheEntryDetail>> {
    let row: Option<DetailRow> = conn
        .query_row(
            "SELECT size, created_at, last_access_at, indexed_at, index_ok, body
               FROM cache_entries WHERE key = ?1",
            params![key],
            |row| {
                Ok(DetailRow {
                    size: row.get(0)?,
                    created_at: row.get(1)?,
                    last_access_at: row.get(2)?,
                    indexed_at: row.get(3)?,
                    index_ok: row.get(4)?,
                    body: row.get(5)?,
                })
            },
        )
        .optional()?;
    let Some(row) = row else {
        return Ok(None);
    };
    let DetailRow {
        size,
        created_at,
        last_access_at,
        indexed_at,
        index_ok,
        body,
    } = row;

    let mut detail = CacheEntryDetail {
        key: key.to_string(),
        size,
        created_at: ms_to_rfc3339(created_at),
        last_access_at: last_access_at.map(ms_to_rfc3339),
        entry_created_at: None,
        indexed: indexed_at.is_some(),
        parseable: index_ok.map(|ok| ok != 0),
        kind: None,
        model: None,
        cost_usd: None,
        input_tokens: None,
        output_tokens: None,
        attempts: None,
        provider_latency_ms: None,
        stop_reason: None,
        empty_reason: None,
        domarinn_version: None,
        request: None,
        provider_fingerprint: None,
        output: None,
        reasoning: None,
        tool_calls: Vec::new(),
        raw: None,
    };

    // Parsed here rather than read from the promoted columns: those hold what a
    // list needs, and a detail view wants the whole entry. A body that will not
    // parse is described as such, never an error — it is still perfectly
    // serviceable through the client read path.
    let Ok(entry) = serde_json::from_slice::<CacheEntry>(&body) else {
        // A row that predates the backfill has no verdict on itself yet; one
        // that has been examined and failed already says so.
        detail.parseable = Some(false);
        return Ok(Some(detail));
    };

    detail.parseable = Some(true);
    detail.entry_created_at = Some(entry.created_at.to_rfc3339());
    detail.kind = entry.kind.map(|k| k.as_str().to_string());
    detail.model = entry.model;
    detail.cost_usd = entry.cost_usd;
    detail.input_tokens = entry.usage.as_ref().map(|u| u.input_tokens as i64);
    detail.output_tokens = entry.usage.as_ref().map(|u| u.output_tokens as i64);
    detail.attempts = entry.attempts;
    detail.provider_latency_ms = entry.provider_latency_ms;
    detail.stop_reason = entry.stop_reason;
    // Clamped, not verbatim, even though this comes straight off the parsed
    // entry: the list column is clamped, and a drawer that disagreed with the
    // row it was opened from would look like two different entries.
    detail.empty_reason = entry.empty_reason.as_ref().map(clamp_empty_reason);
    detail.domarinn_version = Some(entry.domarinn_version);
    detail.request = entry.request;
    detail.provider_fingerprint = entry.provider_fingerprint;
    detail.output = Some(entry.output);
    detail.reasoning = entry.reasoning;
    detail.tool_calls = entry.tool_calls;
    if include_raw {
        detail.raw = entry.raw;
    }
    Ok(Some(detail))
}

fn facets(conn: &Connection) -> anyhow::Result<CacheFacetsResponse> {
    let kinds = {
        let mut stmt = conn.prepare(
            "SELECT kind, COUNT(*) FROM cache_entries
              WHERE kind IS NOT NULL GROUP BY kind ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let models = {
        let mut stmt = conn.prepare(
            "SELECT model, COUNT(*) FROM cache_entries
              WHERE model IS NOT NULL GROUP BY model ORDER BY COUNT(*) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![MAX_MODEL_FACETS], |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    // Uncapped, unlike `models`. The reason vocabulary is a handful of
    // constants plus whatever a future vendor invents, and `clamp_empty_reason`
    // bounds each value's length — so unlike a model string, this facet cannot
    // be grown without bound by whatever happens to be in the store.
    let empty_reasons = {
        let mut stmt = conn.prepare(
            "SELECT empty_reason, COUNT(*) FROM cache_entries
              WHERE empty_reason IS NOT NULL GROUP BY empty_reason ORDER BY COUNT(*) DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let total: i64 = conn.query_row("SELECT COUNT(*) FROM cache_entries", [], |r| r.get(0))?;
    let unindexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE indexed_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    let unparseable: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE index_ok = 0",
        [],
        |r| r.get(0),
    )?;

    Ok(CacheFacetsResponse {
        kinds,
        models,
        empty_reasons,
        total,
        unindexed,
        unparseable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key contains a colon of its own, so the cursor has to split on the
    /// first one. Splitting on the last — or using the run list's helper — hands
    /// back a truncated key and pagination silently loops.
    #[test]
    fn a_cursor_round_trips_a_key_that_contains_a_colon() {
        let key = "sha256:0123456789abcdef";
        let cursor = encode_entry_cursor(1_700_000_000_000, key);
        assert_eq!(
            decode_entry_cursor(&cursor),
            Some((1_700_000_000_000, key.to_string()))
        );
    }

    #[test]
    fn a_malformed_cursor_decodes_to_nothing() {
        assert_eq!(decode_entry_cursor("nonsense"), None);
        assert_eq!(decode_entry_cursor("notanumber:sha256:aa"), None);
    }

    /// fts5 answers stray operators with a syntax error. Quoting every term
    /// removes the operator surface entirely.
    #[test]
    fn a_search_box_never_produces_fts_syntax() {
        assert_eq!(
            fts_query("refund policy").as_deref(),
            Some(r#""refund" "policy""#)
        );
        assert_eq!(fts_query("NEAR(").as_deref(), Some(r#""NEAR""#));
        assert_eq!(
            fts_query(r#""unbalanced"#).as_deref(),
            Some(r#""unbalanced""#)
        );
        assert_eq!(fts_query("*"), None);
        assert_eq!(fts_query("   "), None);
    }
}
