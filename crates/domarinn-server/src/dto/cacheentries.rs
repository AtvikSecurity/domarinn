//! DTOs for browsing the cache: `GET /cache/entries`, `/cache/entries/{key}`
//! and `/cache/facets`.
//!
//! Separate from [`super::cache`], which serves the client-facing stats and
//! prune surface. These describe what an entry *is*, not how the cache is
//! doing.

use serde::Serialize;
use serde_json::Value as Json;
use ts_rs::TS;

use domarinn_core::result::ToolCall;
use domarinn_core::types::Output;

/// One row of the entries table.
///
/// Everything below `last_access_at` is derived from the entry body, so all of
/// it is nullable twice over: the body may not have carried the field, and the
/// body may not have been examined yet. `indexed` and `parseable` are how a
/// client tells those two apart instead of rendering both as "unknown".
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheEntryListItem {
    pub key: String,
    pub size: i64,
    /// When this instance was handed the entry, RFC3339.
    pub created_at: String,
    /// RFC3339. `None` on a tier that cannot report access times honestly.
    pub last_access_at: Option<String>,
    /// When the *call* happened, as the entry records it, RFC3339. Differs from
    /// `created_at` by however long the entry sat on a laptop before upload.
    pub entry_created_at: Option<String>,
    /// Whether the body has been examined yet. `false` means the columns below
    /// are unknown rather than absent — say "indexing", not "no model".
    pub indexed: bool,
    /// Whether examining it found an entry this server understands. `None`
    /// while `indexed` is false.
    pub parseable: Option<bool>,
    pub kind: Option<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    /// Where the request went — transport plus host or command, never prompt
    /// text and never a query string. See `storage::cacheindex`.
    pub request_summary: Option<String>,
    pub output_preview: Option<String>,
    /// Why the output was empty, when it was. A plain `Option` with **no**
    /// `skip_serializing_if`, unlike `dto::cases` and `dto::runs`, which omit
    /// theirs: this module's rule is that an absent member is an explicit
    /// `null` (pinned by `an_unindexed_row_serializes_every_unknown_as_explicit_null`),
    /// because a client here has to distinguish "not examined" from "examined,
    /// nothing to report" — which is also why the column is plain `NULL` rather
    /// than the runs side's `''` sentinel.
    pub empty_reason: Option<String>,
}

/// `GET /cache/entries` response.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheEntryListResponse {
    pub entries: Vec<CacheEntryListItem>,
    pub next_cursor: Option<String>,
    /// True when the tier stopped looking before it ran out of entries. Only a
    /// scanning tier can set this; the server tier answers from an index and is
    /// always exhaustive.
    pub truncated: bool,
}

/// `GET /cache/entries/{key}` response.
///
/// Every body-derived member is `None` when `parseable` is false, which is a
/// real state rather than an error: the cache stores what it is handed, so an
/// entry written by a newer domarinn is served intact by `GET /cache/{key}`
/// and simply cannot be described here.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheEntryDetail {
    pub key: String,
    pub size: i64,
    pub created_at: String,
    pub last_access_at: Option<String>,
    pub entry_created_at: Option<String>,
    pub indexed: bool,
    pub parseable: Option<bool>,
    pub kind: Option<String>,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub attempts: Option<u32>,
    /// The original call's in-flight time, not the cache read's.
    pub provider_latency_ms: Option<u64>,
    pub stop_reason: Option<String>,
    /// Why the output was empty. See [`CacheEntryListItem::empty_reason`] for
    /// why this is an explicit `null` rather than an omitted key.
    pub empty_reason: Option<String>,
    pub domarinn_version: Option<String>,
    /// The redacted canonical request. Credentials live in headers and are
    /// structurally absent from both envelope shapes.
    #[ts(type = "unknown")]
    pub request: Option<Json>,
    /// What selected the provider that answered. Only on entries written before
    /// the request itself was recorded — the two are never both present.
    #[ts(type = "unknown")]
    pub provider_fingerprint: Option<Json>,
    pub output: Option<Output>,
    pub reasoning: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    /// Raw provider metadata. Withheld unless `?raw=true`: it is the largest
    /// member and the least often wanted, and sending it on every drawer open
    /// would cost the whole payload a second time.
    #[ts(type = "unknown")]
    pub raw: Option<Json>,
}

/// One case that used an entry.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheEntryRunRef {
    pub run_id: String,
    pub project: Option<String>,
    pub suite: Option<String>,
    pub created_at: String,
    pub case_key: String,
    pub name: Option<String>,
    pub status: String,
    /// Whether this particular case was served from cache. A miss that *wrote*
    /// the entry is listed too — the key belongs to the request, so both are
    /// runs that used this entry.
    pub cached: bool,
}

/// `GET /cache/entries/{key}/runs` response.
///
/// An empty list is ambiguous and readers must say so: cross-linking only works
/// for runs recorded by a version that wrote the key, and no backfill can
/// supply it for older ones. "No runs" is not evidence that an entry is unused.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheEntryRunsResponse {
    pub cases: Vec<CacheEntryRunRef>,
    pub next_cursor: Option<String>,
}

/// One value a filter can take, and how many entries carry it.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheFacet {
    pub value: String,
    pub count: i64,
}

/// `GET /cache/facets` response — what to put in the filter dropdowns.
#[derive(Debug, Clone, Serialize, TS)]
pub struct CacheFacetsResponse {
    pub kinds: Vec<CacheFacet>,
    /// Capped at the most common values: a store polluted with pathological
    /// model strings must not be able to produce an unbounded response.
    pub models: Vec<CacheFacet>,
    /// Deliberately uncapped, unlike `models`: the reason vocabulary is a
    /// handful of constants, each value is length-clamped on the way into the
    /// index, and this is the dropdown someone reaches for when a refusal has
    /// poisoned a shared cache — truncating its tail would hide the rare
    /// reason they are hunting.
    pub empty_reasons: Vec<CacheFacet>,
    pub total: i64,
    /// Entries whose body has not been examined yet. They are listed, but no
    /// `kind`/`model` filter can match them and search cannot reach them.
    pub unindexed: i64,
    /// Entries whose body this server could not parse.
    pub unparseable: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item() -> CacheEntryListItem {
        CacheEntryListItem {
            key: "sha256:aa".into(),
            size: 12,
            created_at: "2026-01-01T00:00:00+00:00".into(),
            last_access_at: Some("2026-01-02T00:00:00+00:00".into()),
            entry_created_at: None,
            indexed: true,
            parseable: Some(true),
            kind: Some("provider".into()),
            model: Some("claude-opus-5".into()),
            cost_usd: Some(0.5),
            input_tokens: Some(10),
            output_tokens: Some(20),
            request_summary: Some("POST https://api/x".into()),
            output_preview: Some("hi".into()),
            empty_reason: None,
        }
    }

    #[test]
    fn list_item_matches_todays_wire_shape() {
        assert_eq!(
            serde_json::to_value(item()).unwrap(),
            json!({
                "key": "sha256:aa",
                "size": 12,
                "created_at": "2026-01-01T00:00:00+00:00",
                "last_access_at": "2026-01-02T00:00:00+00:00",
                "entry_created_at": null,
                "indexed": true,
                "parseable": true,
                "kind": "provider",
                "model": "claude-opus-5",
                "cost_usd": 0.5,
                "input_tokens": 10,
                "output_tokens": 20,
                "request_summary": "POST https://api/x",
                "output_preview": "hi",
                "empty_reason": null,
            })
        );
    }

    /// Absent members are `null`, never omitted — the whole DTO layer's rule,
    /// and what lets the UI distinguish "no value" from "field not in this
    /// server's version".
    #[test]
    fn an_unindexed_row_serializes_every_unknown_as_explicit_null() {
        let mut row = item();
        row.indexed = false;
        row.parseable = None;
        row.kind = None;
        row.model = None;
        row.cost_usd = None;
        row.input_tokens = None;
        row.output_tokens = None;
        row.request_summary = None;
        row.output_preview = None;
        row.empty_reason = None;

        let v = serde_json::to_value(row).unwrap();
        for field in [
            "parseable",
            "kind",
            "model",
            "cost_usd",
            "input_tokens",
            "output_tokens",
            "request_summary",
            "output_preview",
            "empty_reason",
        ] {
            assert!(v.get(field).is_some(), "{field} was omitted");
            assert!(v[field].is_null(), "{field} was not null");
        }
    }

    #[test]
    fn an_exhausted_list_reports_a_null_cursor() {
        let v = serde_json::to_value(CacheEntryListResponse {
            entries: vec![],
            next_cursor: None,
            truncated: false,
        })
        .unwrap();
        assert!(v.get("next_cursor").is_some());
        assert!(v["next_cursor"].is_null());
        assert_eq!(v["truncated"], json!(false));
    }
}
