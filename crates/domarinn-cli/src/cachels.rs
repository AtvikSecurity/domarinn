//! `cache ls` and `cache show`: read what is in the local cache.
//!
//! Split from [`crate::cachecmd`], which is already at its own size, and
//! because these two only read where those four also delete.
//!
//! # Why these exist alongside the web browser
//!
//! The developer asking "why did this case replay a stale answer" is at a
//! terminal, in a repo, with a warm `.domarinn/cache` and no server running.
//! A browser-only answer serves exactly the deployment where that question is
//! least often asked. These also make an existing warning actionable:
//! `warn_on_program_drift` says you are replaying answers from a different
//! build of a provider's program, and until now offered no way to *look at*
//! one.
//!
//! Local disk only, preserving the contract `cachecmd` and `docs/reference/
//! cli.md` both state. Server-side browsing belongs with the other
//! server-talking commands, not here.

use std::collections::BTreeMap;

use domarinn_cache::LocalDiskCache;
use domarinn_core::cache::{CacheBackend, CacheEntry, CacheEnumerate, CacheKey};

use crate::cachecmd::Where;
use crate::exit;

/// Entries stat'ed before `ls` reports truncation.
///
/// The walk sorts by modification time before truncating, so what a bound drops
/// is the oldest — which is not what anyone is looking for when they run this.
const MAX_SCAN: usize = 20_000;

/// Default rows printed. `--limit` raises it; `--json` is for anything larger.
const DEFAULT_LIMIT: usize = 40;

pub fn ls(
    which: &Where,
    kind: Option<String>,
    model: Option<String>,
    limit: Option<usize>,
    json: bool,
) -> u8 {
    let root = which.resolve_root();
    // Both tiers, because a run reads both: an `ls` that omitted the read-only
    // legacy tier would be an `ls` that lies about what the next run can hit.
    let tiers: Vec<LocalDiskCache> = std::iter::once(LocalDiskCache::new(&root.root))
        .chain(root.legacy.as_ref().map(LocalDiskCache::new))
        .collect();

    crate::cachecmd::block_on(async move {
        let mut rows: Vec<Row> = Vec::new();
        let mut truncated = false;
        for tier in &tiers {
            let found = match tier.enumerate(MAX_SCAN).await {
                Ok(found) => found,
                Err(e) => {
                    eprintln!("error: {e}");
                    return exit::INFRA;
                }
            };
            truncated |= found.truncated;
            for entry in found.entries {
                let parsed = tier.get(&entry.key).await.ok().flatten();
                rows.push(Row::new(entry.key, entry.size, parsed));
            }
        }

        rows.retain(|row| {
            kind.as_ref().is_none_or(|k| row.kind.as_deref() == Some(k))
                && model
                    .as_ref()
                    .is_none_or(|m| row.model.as_deref() == Some(m))
        });
        let limit = limit.unwrap_or(DEFAULT_LIMIT);
        let shown = rows.len().min(limit);

        if json {
            // One object per line, so the output composes with `jq`, `grep` and
            // `head` without holding the whole cache in memory anywhere.
            for row in rows.iter().take(limit) {
                println!(
                    "{}",
                    serde_json::to_string(&row.to_json()).unwrap_or_default()
                );
            }
            return exit::OK;
        }

        if rows.is_empty() {
            println!("no entries");
            return exit::OK;
        }
        for row in rows.iter().take(limit) {
            println!(
                "{}  {:<12}  {:<24}  {:>9}  {}",
                short_key(&row.key),
                row.kind.as_deref().unwrap_or("-"),
                row.model.as_deref().unwrap_or("-"),
                bytes(row.size),
                row.summary.as_deref().unwrap_or("-"),
            );
        }
        if shown < rows.len() {
            println!("… {} more (use --limit or --json)", rows.len() - shown);
        }
        if truncated {
            println!("note: stopped after {MAX_SCAN} files; older entries not listed");
        }
        exit::OK
    })
}

pub fn show(which: &Where, key: String, json: bool, raw: bool) -> u8 {
    if !CacheKey::is_valid(&key) {
        eprintln!("error: not a cache key: {key} (expected sha256:<64 hex>)");
        return exit::USAGE;
    }
    let root = which.resolve_root();
    let tiers: Vec<LocalDiskCache> = std::iter::once(LocalDiskCache::new(&root.root))
        .chain(root.legacy.as_ref().map(LocalDiskCache::new))
        .collect();
    let cache_key = CacheKey(key.clone());

    crate::cachecmd::block_on(async move {
        for tier in &tiers {
            let Ok(Some(entry)) = tier.get(&cache_key).await else {
                continue;
            };
            if json {
                let mut value = serde_json::to_value(&entry).unwrap_or_default();
                if !raw {
                    // The largest member and the least often wanted; `--raw`
                    // asks for it explicitly.
                    if let Some(map) = value.as_object_mut() {
                        map.remove("raw");
                    }
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                );
            } else {
                print_entry(&key, &entry, raw);
            }
            return exit::OK;
        }
        eprintln!("error: no entry for {key} in {}", root.root.display());
        exit::INFRA
    })
}

struct Row {
    key: CacheKey,
    size: u64,
    kind: Option<String>,
    model: Option<String>,
    summary: Option<String>,
}

impl Row {
    fn new(key: CacheKey, size: u64, entry: Option<CacheEntry>) -> Row {
        let Some(entry) = entry else {
            // A file in the layout this build cannot read. Listed, not hidden:
            // an entry it cannot describe is still an entry, and omitting it
            // would make the count disagree with the directory.
            return Row {
                key,
                size,
                kind: Some("unreadable".into()),
                model: None,
                summary: None,
            };
        };
        Row {
            key,
            size,
            // The same ladder the server uses, so `cache ls` and the web
            // browser never disagree about the same entry.
            kind: entry.inferred_kind(),
            model: entry.model,
            summary: entry.request.as_ref().and_then(request_summary),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "key": self.key.0,
            "size": self.size,
            "kind": self.kind,
            "model": self.model,
            "request": self.summary,
        })
    }
}

/// Where the request went — never prompt text, matching what the server's list
/// view shows and for the same reason.
fn request_summary(request: &serde_json::Value) -> Option<String> {
    match request.get("transport").and_then(|v| v.as_str()) {
        Some("http") => {
            let method = request
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("POST");
            let url = request.get("url").and_then(|v| v.as_str())?;
            let end = url.find(['?', '#']).unwrap_or(url.len());
            Some(format!("{method} {}", &url[..end]))
        }
        Some("exec") => request
            .get("command")
            .and_then(|v| v.as_str())
            .map(|c| format!("exec {c}")),
        _ => None,
    }
}

fn print_entry(key: &str, entry: &CacheEntry, raw: bool) {
    let mut fields: BTreeMap<&str, String> = BTreeMap::new();
    fields.insert("key", key.to_string());
    fields.insert("created", entry.created_at.to_rfc3339());
    fields.insert("version", entry.domarinn_version.clone());
    if let Some(kind) = &entry.kind {
        fields.insert("kind", kind.as_str().to_string());
    }
    if let Some(model) = &entry.model {
        fields.insert("model", model.clone());
    }
    if let Some(cost) = entry.cost_usd {
        fields.insert("cost", format!("${cost:.6}"));
    }
    if let Some(usage) = &entry.usage {
        fields.insert(
            "tokens",
            format!("{} in / {} out", usage.input_tokens, usage.output_tokens),
        );
    }
    for (name, value) in fields {
        println!("{name:<9} {value}");
    }
    if let Some(request) = &entry.request {
        println!("\n--- request ---");
        println!(
            "{}",
            serde_json::to_string_pretty(request).unwrap_or_default()
        );
    } else if let Some(fingerprint) = &entry.provider_fingerprint {
        println!("\n--- provider fingerprint (entry predates request capture) ---");
        println!(
            "{}",
            serde_json::to_string_pretty(fingerprint).unwrap_or_default()
        );
    }
    println!("\n--- output ---");
    match &entry.output {
        domarinn_core::types::Output::Text(text) => println!("{text}"),
        domarinn_core::types::Output::Json(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_default()
            )
        }
    }
    if raw {
        if let Some(payload) = &entry.raw {
            println!("\n--- provider metadata ---");
            println!(
                "{}",
                serde_json::to_string_pretty(payload).unwrap_or_default()
            );
        }
    }
}

fn short_key(key: &CacheKey) -> String {
    let hex = key.0.strip_prefix("sha256:").unwrap_or(&key.0);
    if hex.len() < 12 {
        return key.0.clone();
    }
    format!("{}…{}", &hex[..6], &hex[hex.len() - 4..])
}

fn bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_summary_never_carries_a_query_string() {
        let request = serde_json::json!({
            "transport": "http",
            "method": "POST",
            "url": "https://gateway/v1/messages?api_key=secret",
        });
        let summary = request_summary(&request).unwrap();
        assert!(!summary.contains("secret"), "{summary}");
        assert_eq!(summary, "POST https://gateway/v1/messages");
    }

    #[test]
    fn a_short_key_keeps_both_ends() {
        let key = CacheKey(format!("sha256:{}", "ab".repeat(32)));
        assert_eq!(short_key(&key), "ababab…abab");
    }
}
