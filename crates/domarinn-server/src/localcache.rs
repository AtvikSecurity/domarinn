//! A read-only view of a local disk cache directory, mounted as a second
//! browsable tier.
//!
//! # Why this exists
//!
//! `cache.backend` defaults to `disk`, so an ordinary developer's runs fill a
//! directory beside their suite and never touch this server at all. A browser
//! that could only see `cache.db` would be empty for exactly the setup most
//! people have. Pointing the server at that directory closes the gap without
//! asking anyone to change how their suites are configured.
//!
//! # What this tier cannot do, and says so
//!
//! There is no index here — just files. So:
//!
//! * Filtering, sorting and search happen in memory over a snapshot, which is
//!   why the walk is bounded and reports [`CacheEntryListResponse::truncated`]
//!   rather than pretending to be exhaustive.
//! * Search is a case-insensitive substring match over the request summary and
//!   output preview, not fts5. `/meta` advertises which kind of search each
//!   tier offers, because "search" meaning two different things without saying
//!   so is worse than a missing feature.
//! * `last_access_at` is always `None`. Access times are unreliable under
//!   `relatime`/`noatime`, and reporting mtime in that field would be a
//!   different measurement wearing the same name.
//!
//! Nothing here ever writes. The directory belongs to whichever runs are using
//! it, and a browser is a reader.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use domarinn_cache::LocalDiskCache;
use domarinn_core::cache::{CacheBackend, CacheEnumerate, CacheKey};
use tokio::sync::RwLock;

use crate::domain::{CacheSort, SortOrder};
use crate::dto::cacheentries::{
    CacheEntryDetail, CacheEntryListItem, CacheEntryListResponse, CacheFacet, CacheFacetsResponse,
};
use crate::storage::cacheindex::EntryIndex;
use crate::storage::{decode_entry_cursor, encode_entry_cursor, CacheListFilter, CursorValue};

/// Files stat'ed before the walk gives up and reports truncation.
///
/// Overridable with `DOMARINN_LOCAL_CACHE_MAX_SCAN`. The walk sorts by mtime
/// before truncating, so what is dropped is the oldest — the part of a cache
/// nobody opens a browser to look at.
pub const DEFAULT_MAX_SCAN: usize = 20_000;

/// How long a snapshot serves before it is rebuilt.
///
/// Short enough that a run finishing mid-browse shows up on the next
/// interaction, long enough that paging does not re-walk and re-parse the whole
/// directory once per page.
const SNAPSHOT_TTL: Duration = Duration::from_secs(30);

/// The cheap projection of one entry, plus what a detail view needs to find it
/// again. Deliberately not the whole entry: 20,000 bodies held in memory for a
/// browsing feature would be hundreds of megabytes resident.
#[derive(Clone)]
struct Row {
    item: CacheEntryListItem,
    /// Millis, for sorting and cursors without re-parsing the RFC3339 strings.
    created_ms: i64,
    cost_microusd: Option<i64>,
    /// Lowercased haystack for substring search.
    haystack: String,
}

struct Snapshot {
    built_at: Instant,
    rows: Arc<Vec<Row>>,
    truncated: bool,
}

pub struct LocalTier {
    disk: LocalDiskCache,
    root: PathBuf,
    max_scan: usize,
    snapshot: RwLock<Option<Snapshot>>,
}

impl LocalTier {
    /// Mount a directory, or refuse with a reason.
    ///
    /// A misconfigured path must never take the server down: the caller logs
    /// and mounts nothing, so a stale environment variable costs one tier
    /// rather than the process.
    pub fn open(root: PathBuf, max_scan: Option<usize>) -> anyhow::Result<LocalTier> {
        let root = root
            .canonicalize()
            .map_err(|e| anyhow::anyhow!("resolving {}: {e}", root.display()))?;
        if !root.is_dir() {
            anyhow::bail!("{} is not a directory", root.display());
        }
        Ok(LocalTier {
            disk: LocalDiskCache::new(&root),
            root,
            max_scan: max_scan.unwrap_or(DEFAULT_MAX_SCAN),
            snapshot: RwLock::new(None),
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    async fn rows(&self) -> anyhow::Result<(Arc<Vec<Row>>, bool)> {
        if let Some(snap) = self.snapshot.read().await.as_ref() {
            if snap.built_at.elapsed() < SNAPSHOT_TTL {
                return Ok((snap.rows.clone(), snap.truncated));
            }
        }

        let found = self
            .disk
            .enumerate(self.max_scan)
            .await
            .map_err(|e| anyhow::anyhow!("walking the local cache: {e}"))?;

        let mut rows = Vec::with_capacity(found.entries.len());
        for entry in found.entries {
            // A file that vanished between the walk and the read is a prune or
            // a `cache clear` racing the browser; skipping it is the whole
            // handling it needs.
            let Ok(Some(parsed)) = self.disk.get(&entry.key).await else {
                rows.push(unreadable_row(&entry.key, entry.size, entry.modified));
                continue;
            };
            let index = EntryIndex::from_entry(&parsed);
            let created_ms = entry.modified.unwrap_or_else(Utc::now).timestamp_millis();
            let haystack = format!(
                "{} {}",
                index.request_summary.clone().unwrap_or_default(),
                index.output_preview.clone().unwrap_or_default()
            )
            .to_lowercase();
            rows.push(Row {
                item: CacheEntryListItem {
                    key: entry.key.0.clone(),
                    size: entry.size as i64,
                    created_at: entry.modified.unwrap_or_else(Utc::now).to_rfc3339(),
                    // No honest value on a filesystem; see the module docs.
                    last_access_at: None,
                    entry_created_at: Some(parsed.created_at.to_rfc3339()),
                    indexed: true,
                    parseable: Some(true),
                    kind: index.kind.clone(),
                    model: index.model.clone(),
                    cost_usd: index.cost_microusd.map(|m| m as f64 / 1_000_000.0),
                    input_tokens: index.input_tokens,
                    output_tokens: index.output_tokens,
                    request_summary: index.request_summary.clone(),
                    output_preview: index.output_preview.clone(),
                    empty_reason: index.empty_reason.clone(),
                },
                created_ms,
                cost_microusd: index.cost_microusd,
                haystack,
            });
        }

        let rows = Arc::new(rows);
        *self.snapshot.write().await = Some(Snapshot {
            built_at: Instant::now(),
            rows: rows.clone(),
            truncated: found.truncated,
        });
        Ok((rows, found.truncated))
    }

    pub async fn list(&self, filter: CacheListFilter) -> anyhow::Result<CacheEntryListResponse> {
        let (rows, truncated) = self.rows().await?;
        let needle = filter.q.as_ref().map(|q| q.to_lowercase());

        let mut matched: Vec<&Row> = rows
            .iter()
            .filter(|row| {
                match filter.kind.as_deref() {
                    // Nothing here is ever un-indexed: a row exists only once
                    // its body has been read. `unparseable` is the one state
                    // that can occur, when a file will not deserialize.
                    Some("unindexed") => return false,
                    Some("unparseable") => {
                        if row.item.parseable != Some(false) {
                            return false;
                        }
                    }
                    Some(kind) if row.item.kind.as_deref() != Some(kind) => return false,
                    Some(_) => {}
                    None => {}
                }
                if let Some(model) = &filter.model {
                    if row.item.model.as_deref() != Some(model.as_str()) {
                        return false;
                    }
                }
                if let Some(reason) = &filter.empty_reason {
                    if row.item.empty_reason.as_deref() != Some(reason.as_str()) {
                        return false;
                    }
                }
                if let Some(since) = filter.since {
                    if row.created_ms < since {
                        return false;
                    }
                }
                if let Some(until) = filter.until {
                    if row.created_ms > until {
                        return false;
                    }
                }
                if let Some(min) = filter.min_cost_microusd {
                    if row.cost_microusd.is_none_or(|c| c < min) {
                        return false;
                    }
                }
                if let Some(max) = filter.max_cost_microusd {
                    if row.cost_microusd.is_none_or(|c| c > max) {
                        return false;
                    }
                }
                if let Some(needle) = &needle {
                    if !row.haystack.contains(needle.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sorts over a nullable value exclude the rows where it is unknown,
        // for the same reason the server tier's `requires_non_null` does:
        // ordering by a value that is not there is meaningless. The token
        // exclusion mirrors SQL's `(input + output) IS NOT NULL` — gone when
        // either operand is unknown.
        match filter.sort {
            CacheSort::Cost => matched.retain(|row| row.cost_microusd.is_some()),
            CacheSort::Kind => matched.retain(|row| row.item.kind.is_some()),
            CacheSort::Model => matched.retain(|row| row.item.model.is_some()),
            CacheSort::Tokens => matched
                .retain(|row| row.item.input_tokens.is_some() && row.item.output_tokens.is_some()),
            CacheSort::Created | CacheSort::LastAccess | CacheSort::Size | CacheSort::Key => {}
        }

        let sort_value = |row: &Row| -> CursorValue {
            match filter.sort {
                CacheSort::Size => CursorValue::Int(row.item.size),
                CacheSort::Cost => CursorValue::Int(row.cost_microusd.unwrap_or_default()),
                // There is no access time on this tier, so `last_access` falls
                // back to mtime rather than erroring — it is the only clock a
                // filesystem offers, and the column renders empty regardless.
                CacheSort::Created | CacheSort::LastAccess => CursorValue::Int(row.created_ms),
                // The unwraps-to-default are unreachable: the retain above
                // dropped every row where the value is unknown.
                CacheSort::Kind => CursorValue::Text(row.item.kind.clone().unwrap_or_default()),
                CacheSort::Model => CursorValue::Text(row.item.model.clone().unwrap_or_default()),
                CacheSort::Tokens => CursorValue::Int(
                    row.item.input_tokens.unwrap_or_default()
                        + row.item.output_tokens.unwrap_or_default(),
                ),
                CacheSort::Key => CursorValue::Text(row.item.key.clone()),
            }
        };
        matched.sort_by(|a, b| {
            let (av, bv) = (sort_value(a), sort_value(b));
            let primary = match filter.order {
                SortOrder::Desc => bv.cmp(&av),
                SortOrder::Asc => av.cmp(&bv),
            };
            primary.then_with(|| match filter.order {
                SortOrder::Desc => b.item.key.cmp(&a.item.key),
                SortOrder::Asc => a.item.key.cmp(&b.item.key),
            })
        });

        // The cursor is resolved by position within the sorted list. Stable for
        // a snapshot's lifetime and shifting across one, exactly like any live
        // list — the same property the server tier's keyset cursor has.
        let start = match filter.cursor.as_ref().and_then(|(value, key)| {
            let _ = value;
            matched.iter().position(|row| &row.item.key == key)
        }) {
            Some(index) => index + 1,
            None => 0,
        };

        let window: Vec<CacheEntryListItem> = matched
            .iter()
            .skip(start)
            .take(filter.limit as usize)
            .map(|row| row.item.clone())
            .collect();
        let next_cursor = matched
            .get(start + window.len())
            .and(window.last())
            .map(|last| encode_entry_cursor(&CursorValue::Int(0), &last.key));

        Ok(CacheEntryListResponse {
            entries: window,
            next_cursor,
            truncated,
        })
    }

    pub async fn detail(
        &self,
        key: &str,
        include_raw: bool,
    ) -> anyhow::Result<Option<CacheEntryDetail>> {
        let key = CacheKey(key.to_string());
        let Ok(Some(entry)) = self.disk.get(&key).await else {
            return Ok(None);
        };
        let index = EntryIndex::from_entry(&entry);
        // Read off the index rather than off `entry` so the clamp applies here
        // too: the server tier's detail view does the same, and the two tiers
        // must not describe the same field two different ways.
        let empty_reason = index.empty_reason.clone();
        Ok(Some(CacheEntryDetail {
            key: key.0,
            // The body is the entry; its serialized length is the honest size
            // without a second stat.
            size: serde_json::to_vec(&entry)
                .map(|b| b.len() as i64)
                .unwrap_or(0),
            created_at: entry.created_at.to_rfc3339(),
            last_access_at: None,
            entry_created_at: Some(entry.created_at.to_rfc3339()),
            indexed: true,
            parseable: Some(true),
            kind: index.kind,
            model: entry.model,
            cost_usd: entry.cost_usd,
            input_tokens: entry.usage.as_ref().map(|u| u.input_tokens as i64),
            output_tokens: entry.usage.as_ref().map(|u| u.output_tokens as i64),
            attempts: entry.attempts,
            provider_latency_ms: entry.provider_latency_ms,
            stop_reason: entry.stop_reason,
            empty_reason,
            domarinn_version: Some(entry.domarinn_version),
            request: entry.request,
            provider_fingerprint: entry.provider_fingerprint,
            output: Some(entry.output),
            reasoning: entry.reasoning,
            tool_calls: entry.tool_calls,
            raw: include_raw.then_some(entry.raw).flatten(),
        }))
    }

    pub async fn facets(&self) -> anyhow::Result<CacheFacetsResponse> {
        let (rows, _) = self.rows().await?;
        let tally = |pick: fn(&Row) -> Option<&String>| {
            let mut counts: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
            for row in rows.iter() {
                if let Some(value) = pick(row) {
                    *counts.entry(value.as_str()).or_default() += 1;
                }
            }
            let mut out: Vec<CacheFacet> = counts
                .into_iter()
                .map(|(value, count)| CacheFacet {
                    value: value.to_string(),
                    count,
                })
                .collect();
            out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
            out
        };
        Ok(CacheFacetsResponse {
            kinds: tally(|r| r.item.kind.as_ref()),
            models: tally(|r| r.item.model.as_ref()),
            empty_reasons: tally(|r| r.item.empty_reason.as_ref()),
            total: rows.len() as i64,
            // Nothing is ever pending here: a row exists only once read.
            unindexed: 0,
            unparseable: rows
                .iter()
                .filter(|r| r.item.parseable == Some(false))
                .count() as i64,
        })
    }
}

/// A file that is in the layout but will not deserialize.
///
/// Listed rather than hidden, for the same reason the server tier lists one: an
/// entry this build cannot read is still an entry, and hiding it would make the
/// count disagree with the directory.
fn unreadable_row(key: &CacheKey, size: u64, modified: Option<chrono::DateTime<Utc>>) -> Row {
    let created = modified.unwrap_or_else(Utc::now);
    Row {
        item: CacheEntryListItem {
            key: key.0.clone(),
            size: size as i64,
            created_at: created.to_rfc3339(),
            last_access_at: None,
            entry_created_at: None,
            indexed: true,
            parseable: Some(false),
            kind: None,
            model: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            request_summary: None,
            output_preview: None,
            // `None`, not "unknown": the row exists because the body would not
            // deserialize, so nothing about *why an output was empty* has been
            // established. `parseable: false` above is the field that says so;
            // inventing a reason here would make it look examined.
            empty_reason: None,
        },
        created_ms: created.timestamp_millis(),
        cost_microusd: None,
        haystack: String::new(),
    }
}

/// Decoding a cursor for this tier only needs the key half — position in the
/// sorted list is what resolves it, not a sort value.
pub fn cursor_key(cursor: &str) -> Option<String> {
    decode_entry_cursor(cursor).map(|(_, key)| key)
}
