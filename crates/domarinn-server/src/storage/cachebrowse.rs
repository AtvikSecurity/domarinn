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

use domarinn_core::cache::CacheEntry;

use super::cacheindex::{clamp_empty_reason, entry_id_col};
use super::exec::{params, Conn, Queryable, Row, Value};
use super::ftsdialect;
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
    pub cursor: Option<(CursorValue, String)>,
}

/// The keyset position a cursor carries: the last row's sort-column value.
///
/// Integer for the timestamp/size/cost/token sorts, text for `kind`, `model`
/// and `key`. Never mixed within one sort, so the derived ordering (used by
/// the local tier's in-memory sort) only ever compares like with like.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CursorValue {
    Int(i64),
    Text(String),
}

/// Cursor encodes `{sort_value}:{key}` for integer sorts — the historical
/// format, kept byte-identical so pre-existing cursors still decode — and
/// `t:{hex(sort_value)}:{key}` for text sorts. Hex, because a kind or model
/// string may contain a colon of its own; the key's `sha256:` colon is safe
/// because the key is always the final segment.
///
/// Split on the **first** colon: a key is `sha256:<hex>` and contains one of
/// its own, so the run-list's `split_once` would decode the sort value as
/// `"sha256"` and lose the key entirely.
pub fn encode_entry_cursor(sort_value: &CursorValue, key: &str) -> String {
    match sort_value {
        CursorValue::Int(v) => format!("{v}:{key}"),
        CursorValue::Text(t) => format!("t:{}:{key}", hex::encode(t)),
    }
}

pub fn decode_entry_cursor(cursor: &str) -> Option<(CursorValue, String)> {
    let (value, rest) = cursor.split_once(':')?;
    if value == "t" {
        let (encoded, key) = rest.split_once(':')?;
        let text = String::from_utf8(hex::decode(encoded).ok()?).ok()?;
        return Some((CursorValue::Text(text), key.to_string()));
    }
    Some((CursorValue::Int(value.parse().ok()?), rest.to_string()))
}

impl CacheSort {
    /// The column (or expression) this sorts on.
    ///
    /// The plain columns are indexed. `Tokens` is an expression — SQLite
    /// sorts it via a temp b-tree, acceptable for a browse over a cache
    /// store; an expression index is possible later if it ever matters.
    fn column(self) -> &'static str {
        match self {
            CacheSort::Created => "created_at",
            CacheSort::LastAccess => "last_access_at",
            CacheSort::Size => "size",
            CacheSort::Cost => "cost_microusd",
            CacheSort::Kind => "kind",
            CacheSort::Model => "model",
            // NULL when either operand is, which `requires_non_null` excludes
            // — the same "unknown is not orderable" stance as cost.
            CacheSort::Tokens => "(input_tokens + output_tokens)",
            CacheSort::Key => "key",
        }
    }

    /// Whether ordering by this column has to exclude rows where it is NULL.
    ///
    /// The exclusion is not cosmetic: the keyset predicate is NULL-unsafe, so
    /// once a page reached the NULL tail `cost < ?` would be NULL for every
    /// remaining row and pagination would stop dead with entries left unseen.
    /// Wrapping it in `IFNULL` fixes that and destroys the index — migration
    /// 11's lesson. Excluding is the honest option: ordering *by* an unknown
    /// value is meaningless in a way that filtering on one is not. `kind` and
    /// `model` are NULL exactly on unindexed rows; the token sum is NULL when
    /// either count is.
    fn requires_non_null(self) -> bool {
        matches!(
            self,
            CacheSort::Cost | CacheSort::Kind | CacheSort::Model | CacheSort::Tokens
        )
    }
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
    conn: &mut Conn<'_>,
    filter: &CacheListFilter,
) -> anyhow::Result<CacheEntryListResponse> {
    let mut sql = format!("SELECT {LIST_COLUMNS} FROM cache_entries WHERE 1=1");
    let mut args: Vec<Value> = Vec::new();

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
        // The sanitizer means user input cannot be a syntax error in either
        // engine — an error is the server's problem, not the user's.
        match ftsdialect::cache_query(q) {
            Some(query) => {
                let dialect = conn.dialect();
                args.push(query.match_arg(dialect).into());
                let id = entry_id_col(dialect);
                let matches = ftsdialect::match_predicate(dialect, "cache_entries_fts", args.len());
                sql.push_str(&format!(
                    " AND {id} IN (SELECT {id} FROM cache_entries_fts WHERE {matches})"
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
        // TEXT comparisons here match ORDER BY because both use the column's
        // default BINARY collation.
        args.push(match value {
            CursorValue::Int(v) => (*v).into(),
            CursorValue::Text(t) => t.clone().into(),
        });
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
    // SQLite puts NULLs first under ASC and last under DESC; Postgres defaults
    // to the opposite. Every nullable sort column already has its NULLs
    // excluded by `requires_non_null`, so the annotation is inert today — it
    // pins SQLite's ordering so the engines cannot diverge if a nullable sort
    // ever stops excluding them.
    let nulls = if filter.sort.requires_non_null() {
        match filter.order {
            SortOrder::Desc => " NULLS LAST",
            SortOrder::Asc => " NULLS FIRST",
        }
    } else {
        ""
    };
    // Fetch one extra to learn whether a next page exists without counting.
    args.push((filter.limit + 1).into());
    sql.push_str(&format!(
        " ORDER BY {column} {direction}{nulls}, key {direction} LIMIT ?{}",
        args.len()
    ));

    let sort = filter.sort;
    let mut fetched: Vec<(CacheEntryListItem, CursorValue)> =
        conn.query_map(&sql, &args, |row| {
            Ok((row_to_item(row)?, sort_value(row, sort)?))
        })?;

    let next_cursor = if fetched.len() as i64 > filter.limit {
        fetched.truncate(filter.limit as usize);
        fetched
            .last()
            .map(|(item, sort_value)| encode_entry_cursor(sort_value, &item.key))
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

/// The sort value off the row already in hand (positions per [`LIST_COLUMNS`]),
/// so the cursor can be built without a second lookup.
///
/// The `Option` unwraps are for the nullable integer columns a sort does not
/// exclude (`last_access_at`); the columns behind text sorts and the token sum
/// are non-NULL under `requires_non_null`.
fn sort_value(row: &Row<'_>, sort: CacheSort) -> anyhow::Result<CursorValue> {
    Ok(match sort {
        CacheSort::Created => CursorValue::Int(row.get(2)?),
        CacheSort::LastAccess => CursorValue::Int(row.get::<Option<i64>>(3)?.unwrap_or_default()),
        CacheSort::Size => CursorValue::Int(row.get(1)?),
        CacheSort::Cost => CursorValue::Int(row.get::<Option<i64>>(9)?.unwrap_or_default()),
        CacheSort::Kind => CursorValue::Text(row.get(7)?),
        CacheSort::Model => CursorValue::Text(row.get(8)?),
        CacheSort::Tokens => CursorValue::Int(
            row.get::<Option<i64>>(10)?.unwrap_or_default()
                + row.get::<Option<i64>>(11)?.unwrap_or_default(),
        ),
        CacheSort::Key => CursorValue::Text(row.get(0)?),
    })
}

fn row_to_item(row: &Row<'_>) -> anyhow::Result<CacheEntryListItem> {
    let indexed_at: Option<i64> = row.get(5)?;
    let index_ok: Option<i64> = row.get(6)?;
    Ok(CacheEntryListItem {
        key: row.get(0)?,
        size: row.get(1)?,
        created_at: ms_to_rfc3339(row.get(2)?),
        last_access_at: row.get::<Option<i64>>(3)?.map(ms_to_rfc3339),
        entry_created_at: row.get::<Option<i64>>(4)?.map(ms_to_rfc3339),
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
    conn: &mut Conn<'_>,
    key: &str,
    include_raw: bool,
) -> anyhow::Result<Option<CacheEntryDetail>> {
    let row: Option<DetailRow> = conn.query_row_opt(
        "SELECT size, created_at, last_access_at, indexed_at, index_ok, body
           FROM cache_entries WHERE key = ?1",
        &params![key],
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
    )?;
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

fn facets(conn: &mut Conn<'_>) -> anyhow::Result<CacheFacetsResponse> {
    let kinds = conn.query_map(
        "SELECT kind, COUNT(*) FROM cache_entries
          WHERE kind IS NOT NULL GROUP BY kind ORDER BY COUNT(*) DESC",
        &[],
        |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        },
    )?;

    let models = conn.query_map(
        "SELECT model, COUNT(*) FROM cache_entries
          WHERE model IS NOT NULL GROUP BY model ORDER BY COUNT(*) DESC LIMIT ?1",
        &params![MAX_MODEL_FACETS],
        |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        },
    )?;

    // Uncapped, unlike `models`. The reason vocabulary is a handful of
    // constants plus whatever a future vendor invents, and `clamp_empty_reason`
    // bounds each value's length — so unlike a model string, this facet cannot
    // be grown without bound by whatever happens to be in the store.
    let empty_reasons = conn.query_map(
        "SELECT empty_reason, COUNT(*) FROM cache_entries
          WHERE empty_reason IS NOT NULL GROUP BY empty_reason ORDER BY COUNT(*) DESC",
        &[],
        |row| {
            Ok(CacheFacet {
                value: row.get(0)?,
                count: row.get(1)?,
            })
        },
    )?;

    let total: i64 = conn.query_row("SELECT COUNT(*) FROM cache_entries", &[], |r| r.get(0))?;
    let unindexed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE indexed_at IS NULL",
        &[],
        |r| r.get(0),
    )?;
    let unparseable: i64 = conn.query_row(
        "SELECT COUNT(*) FROM cache_entries WHERE index_ok = 0",
        &[],
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
        let cursor = encode_entry_cursor(&CursorValue::Int(1_700_000_000_000), key);
        assert_eq!(
            decode_entry_cursor(&cursor),
            Some((CursorValue::Int(1_700_000_000_000), key.to_string()))
        );
    }

    /// The integer encoding is the historical format, byte for byte, so a
    /// cursor issued before text sorts existed still decodes.
    #[test]
    fn an_integer_cursor_keeps_the_historical_encoding() {
        assert_eq!(
            encode_entry_cursor(&CursorValue::Int(42), "sha256:aa"),
            "42:sha256:aa"
        );
    }

    /// Text sort values are hex-wrapped so a value containing a colon — or
    /// anything else — cannot be confused with the `value:key` framing.
    #[test]
    fn a_text_cursor_round_trips_a_value_that_contains_a_colon() {
        let key = "sha256:0123456789abcdef";
        for value in ["chat", "weird:model/v1", ""] {
            let cursor = encode_entry_cursor(&CursorValue::Text(value.to_string()), key);
            assert_eq!(
                decode_entry_cursor(&cursor),
                Some((CursorValue::Text(value.to_string()), key.to_string())),
                "value {value:?}"
            );
        }
    }

    #[test]
    fn a_malformed_cursor_decodes_to_nothing() {
        assert_eq!(decode_entry_cursor("nonsense"), None);
        assert_eq!(decode_entry_cursor("notanumber:sha256:aa"), None);
        // A `t:` cursor whose hex segment is junk, or that lacks a key.
        assert_eq!(decode_entry_cursor("t:zz:sha256:aa"), None);
        assert_eq!(decode_entry_cursor("t:63686174"), None);
    }

    /// fts5 answers stray operators with a syntax error. Quoting every term
    /// removes the operator surface entirely. (The sanitizer lives in
    /// `ftsdialect`; this pins the rendering this module splices into SQL.)
    #[test]
    fn a_search_box_never_produces_fts_syntax() {
        use crate::storage::exec::Dialect;
        let render = |q: &str| ftsdialect::cache_query(q).map(|f| f.match_arg(Dialect::Sqlite));
        assert_eq!(
            render("refund policy").as_deref(),
            Some(r#""refund" "policy""#)
        );
        assert_eq!(render("NEAR(").as_deref(), Some(r#""NEAR""#));
        assert_eq!(render(r#""unbalanced"#).as_deref(), Some(r#""unbalanced""#));
        assert_eq!(render("*"), None);
        assert_eq!(render("   "), None);
    }
}
