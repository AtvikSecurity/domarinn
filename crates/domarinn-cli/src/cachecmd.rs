//! The `cache` subcommands: stats, path, gc, clear.
//!
//! `ls` and `show` live in [`crate::cachels`] — they only read, where these
//! four also delete, and this file is already at its own size.
//!
//! All four operate on the local disk tier only — see the `cache` help in
//! `main.rs`. Each accounts for the read-only legacy tier a run would layer
//! underneath ([`crate::cachecfg::LocalRoot`]), because a `clear` that leaves
//! it behind is a `clear` the next run undoes. Reporting that tier and deleting
//! it are separate questions, though: see [`cwd_contains`].

use std::path::{Path, PathBuf};

use clap::Subcommand;
use domarinn_cache::LocalDiskCache;
use domarinn_core::cache::{CacheBackend, PurgeFilter};

use crate::cachecfg::LocalRoot;
use crate::exit;

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Show cache entry count and size.
    Stats(Where),
    /// Print the local cache directory path.
    Path(Where),
    /// Remove cache entries matching an age window or an empty reason.
    Gc {
        /// Remove entries older than this (e.g. 30d, 12h).
        // Not `required = true`: clap's own message would say only that the
        // argument is missing, and the whole hazard here is that the obvious
        // reading of a bare `gc` is "tidy up a bit", not "delete everything".
        // The hand-written guard below names `cache clear` instead.
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        /// Keep the purge away from entries younger than this — the recent end
        /// of a window. Requires `--older-than`.
        //
        // No `env =` here, nor on `--empty-reason`. Env vars are for standing
        // policy; an environment that silently widens a one-shot destructive
        // command is how an operator deletes a corpus they never named.
        #[arg(long, value_name = "DURATION")]
        newer_than: Option<String>,
        /// Only entries whose recorded empty reason is this (repeatable).
        ///
        /// The targeted-eviction path: `--empty-reason refusal` removes the
        /// entries a bad draw froze without touching the warm cache around
        /// them.
        #[arg(long, value_name = "REASON")]
        empty_reason: Vec<String>,
        #[command(flatten)]
        which: Where,
    },
    /// Remove all cache entries.
    Clear(Where),
    /// List cache entries.
    Ls {
        /// Only entries of this kind (provider, judge, embedding, exec_assert).
        #[arg(long, value_name = "KIND")]
        kind: Option<String>,
        /// Only entries answered by this model.
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
        /// Rows to print (default 40).
        #[arg(long, value_name = "N")]
        limit: Option<usize>,
        /// One JSON object per line. What makes the local cache scriptable.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        which: Where,
    },
    /// Show one cache entry: its request, its response, and what it cost.
    Show {
        /// The `sha256:<64 hex>` key.
        #[arg(value_name = "KEY")]
        key: String,
        /// Include the provider's raw metadata.
        #[arg(long)]
        raw: bool,
        /// Emit the entry as JSON.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        which: Where,
    },
}

/// Which cache directory a subcommand operates on.
///
/// These commands used to assume `.domarinn/cache` under the process cwd, which
/// meant `domarinn cache stats` reported on whichever directory you happened to
/// be standing in — and answered "0 entries" from a repo root about a perfectly
/// warm cache one level down. A run resolves its cache against the suite, so
/// these have to be able to as well.
#[derive(clap::Args)]
pub struct Where {
    /// The suite whose cache to operate on. Defaults to the current directory.
    #[arg(value_name = "SUITE")]
    pub path: Option<PathBuf>,

    /// Operate on this directory directly, ignoring `SUITE`.
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,
}

impl Where {
    /// The suite directory this command names, which every other path resolves
    /// against. Defaults to the process cwd.
    fn suite_base(&self) -> PathBuf {
        self.path
            .clone()
            .map(|p| {
                let file = domarinn_core::loader::resolve_suite_path(&p);
                domarinn_core::loader::suite_base_dir(&file)
            })
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// Resolve to the tiers a run would use, by the same precedence as
    /// `domarinn run`: `--cache-dir`, then `DOMARINN_CACHE_DIR`, then
    /// `.domarinn/cache` beside the suite — plus any legacy tier underneath.
    /// Returns the suite directory too, since whether that legacy tier is this
    /// project's leftover or another project's live cache depends on it.
    /// Just the tiers, for the read-only subcommands that never need to know
    /// whether a legacy tier is safe to delete.
    pub fn resolve_root(&self) -> LocalRoot {
        self.resolve().1
    }

    fn resolve(&self) -> (PathBuf, LocalRoot) {
        let base = self.suite_base();
        let root = crate::cachecfg::local_root(self.cache_dir.as_deref(), &base);
        (base, root)
    }
}

/// Whether a `clear`/`gc` may purge the legacy tier as well as the primary one.
///
/// The legacy tier is `.domarinn/cache` under the *process cwd*, so for a suite
/// somewhere else entirely it is not a leftover at all — it is whatever project
/// the operator happens to be standing in, and its cache is live. `cd ~/projB
/// && domarinn cache clear ~/projA/evals` must not take projB's cache with it.
///
/// A genuine pre-0.4 leftover was written cwd-relative by invocations like
/// `domarinn run ./evals` from a project root, so it only ever pairs with a
/// suite at or under that root — which is exactly the test. Reporting is not
/// gated this way: `stats` and `path` name the directory without touching it,
/// and seeing it is how an operator finds out it is there.
fn cwd_contains(suite_base: &Path) -> bool {
    let (Ok(cwd), Ok(base)) = (
        std::env::current_dir().and_then(|d| d.canonicalize()),
        suite_base.canonicalize(),
    ) else {
        // Unreadable either way: decline to delete rather than guess.
        return false;
    };
    base.starts_with(cwd)
}

pub fn execute(cmd: CacheCmd) -> u8 {
    // The read-only subcommands resolve their own tiers and return early: they
    // never ask whether a legacy tier is safe to delete, which is the only
    // reason the rest of this function needs the suite base.
    match cmd {
        CacheCmd::Ls {
            kind,
            model,
            limit,
            json,
            which,
        } => return crate::cachels::ls(&which, kind, model, limit, json),
        CacheCmd::Show {
            key,
            raw,
            json,
            which,
        } => return crate::cachels::show(&which, key, json, raw),
        _ => {}
    }

    let (base, root) = match &cmd {
        CacheCmd::Stats(w) | CacheCmd::Path(w) | CacheCmd::Clear(w) => w.resolve(),
        CacheCmd::Gc { which, .. } => which.resolve(),
        CacheCmd::Ls { .. } | CacheCmd::Show { .. } => unreachable!("handled above"),
    };
    // The same two tiers `cachecfg::build_cache` gives a run, minus the
    // read-through wrapper: these commands address each tier, so a purge has to
    // reach the one a run only ever reads.
    let primary = LocalDiskCache::new(&root.root);
    let legacy = root.legacy.as_ref().map(LocalDiskCache::new);
    // Destructive subcommands see a narrower legacy tier than reporting ones.
    let legacy_is_ours = root.legacy.is_some() && cwd_contains(&base);

    match cmd {
        CacheCmd::Path(_) => {
            println!("{}", root.root.display());
            if let Some(legacy) = &legacy {
                println!("legacy tier: {}", legacy.root().display());
            }
            exit::OK
        }
        CacheCmd::Stats(_) => block_on(async move {
            let stats = match primary.stats().await {
                Ok(s) => s,
                Err(e) => return infra(&e),
            };
            println!("{} entries, {}", stats.entries, mib(stats.total_bytes));
            if let Some(legacy) = &legacy {
                // What `clear` would actually do with it, not what it does in
                // the common case — the guard makes those differ.
                let fate = if legacy_is_ours {
                    "`cache clear` removes it"
                } else {
                    "owned by the current directory, so `cache clear` leaves it"
                };
                match legacy.stats().await {
                    Ok(s) => println!(
                        "legacy tier {}: {} entries, {} (read-only during runs; {fate})",
                        legacy.root().display(),
                        s.entries,
                        mib(s.total_bytes)
                    ),
                    Err(e) => return infra(&e),
                }
            }
            exit::OK
        }),
        CacheCmd::Gc {
            older_than,
            newer_than,
            empty_reason,
            ..
        } => {
            // Hand-written rather than clap's `required`/`requires`, for the
            // reason on the flags themselves: the message has to say what to
            // run *instead*, because the mistake this catches is someone who
            // wanted `clear` reaching for the gentler-sounding verb.
            if older_than.is_none() && newer_than.is_none() && empty_reason.is_empty() {
                eprintln!(
                    "error: cache gc needs a bound — --older-than (e.g. --older-than 30d), \
                     --newer-than or --empty-reason; \
                     to remove every entry, use `domarinn cache clear`"
                );
                return exit::USAGE;
            }
            // `--newer-than` alone would read "delete everything since", which
            // is a whole-store wipe wearing the vocabulary of housekeeping.
            // Requiring the other bound makes that reading unconstructible
            // rather than merely discouraged — the same posture `cwd_contains`
            // takes toward the legacy tier.
            if newer_than.is_some() && older_than.is_none() {
                eprintln!(
                    "error: --newer-than names only the recent end of a window, so it needs \
                     --older-than too (e.g. --older-than 30d --newer-than 90d); \
                     to remove every entry, use `domarinn cache clear`"
                );
                return exit::USAGE;
            }
            let (older_than, newer_than) = match (
                parse_opt_duration(&older_than),
                parse_opt_duration(&newer_than),
            ) {
                (Ok(o), Ok(n)) => (o, n),
                (Err(e), _) | (_, Err(e)) => {
                    eprintln!("error: {e}");
                    return exit::USAGE;
                }
            };
            let filter = PurgeFilter {
                older_than,
                newer_than,
                empty_reason: empty_reason
                    .into_iter()
                    .map(domarinn_core::empty::EmptyReason::new)
                    .collect(),
                ..Default::default()
            };
            let legacy = legacy.filter(|_| legacy_is_ours);
            block_on(
                async move { purge_tiers(&primary, legacy.as_ref(), &filter, "removed").await },
            )
        }
        CacheCmd::Ls { .. } | CacheCmd::Show { .. } => unreachable!("handled above"),
        CacheCmd::Clear(_) => {
            let legacy = legacy.filter(|_| legacy_is_ours);
            block_on(async move {
                purge_tiers(
                    &primary,
                    legacy.as_ref(),
                    // The single most important invariant in this file: `clear`
                    // is a literally-default filter, which is the only thing the
                    // backends read as "everything".
                    &PurgeFilter::default(),
                    "cleared",
                )
                .await
            })
        }
    }
}

/// Purge both tiers, reporting each. `verb` leads each line ("removed" for a
/// `gc`, "cleared" for a `clear`).
///
/// A `gc` also reports the denominator — `removed 3 of 200` — because a
/// targeted eviction's whole claim is that it spared the rest, and a bare count
/// cannot distinguish "removed the three poisoned entries" from "removed the
/// only three entries there were". `clear` gets no denominator: it removed all
/// of them by construction, and the existing wording is what operators grep.
async fn purge_tiers(
    primary: &LocalDiskCache,
    legacy: Option<&LocalDiskCache>,
    filter: &PurgeFilter,
    verb: &str,
) -> u8 {
    let with_total = !filter.is_default();
    let total = if with_total {
        match primary.stats().await {
            Ok(s) => Some(s.entries),
            Err(e) => return infra(&e),
        }
    } else {
        None
    };
    match primary.purge(filter).await {
        Ok(n) => println!("{verb} {}", counted(n, total, "cache")),
        Err(e) => return infra(&e),
    }
    if let Some(legacy) = legacy {
        // The same filter, unmodified. An earlier version stripped `newer_than`
        // here, reasoning that a "delete the recent half" filter would turn
        // `legacy_is_ours`'s cwd rule from one that spares a stranger's cache
        // into one that empties your own. That shape is already unconstructible
        // — the guard above refuses `--newer-than` without `--older-than`, so
        // the only reachable form is a *window* — and dropping the recent bound
        // could therefore only ever widen the deletion: `--older-than 30d
        // --newer-than 90d` asks for the 30–90-day band and would have taken
        // everything older than 30 days from the legacy tier, including the
        // entries the operator explicitly bounded away.
        let legacy_filter = filter.clone();
        let total = if with_total {
            match legacy.stats().await {
                Ok(s) => Some(s.entries),
                Err(e) => return infra(&e),
            }
        } else {
            None
        };
        match legacy.purge(&legacy_filter).await {
            Ok(n) => println!(
                "{verb} {} from {}",
                counted(n, total, "legacy cache"),
                legacy.root().display()
            ),
            Err(e) => return infra(&e),
        }
    }
    exit::OK
}

/// `3 cache entries` or, when a denominator is known, `3 of 200 cache entries`.
/// Pluralized on whichever number the reader is counting.
fn counted(n: u64, total: Option<u64>, noun: &str) -> String {
    match total {
        None => format!("{n} {noun} entr{}", plural(n)),
        Some(total) => format!("{n} of {total} {noun} entr{}", plural(total)),
    }
}

fn infra(e: &domarinn_core::cache::CacheError) -> u8 {
    eprintln!("error: {e}");
    exit::INFRA
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
}

fn mib(bytes: u64) -> String {
    format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

pub(crate) fn block_on<F: std::future::Future<Output = u8>>(fut: F) -> u8 {
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(fut),
        Err(e) => {
            eprintln!("error: {e}");
            exit::INFRA
        }
    }
}

/// [`parse_duration`] over an optional spec, so both bounds are parsed the same
/// way and an absent one stays absent rather than becoming a zero — which the
/// backends read as "the cutoff is now", i.e. everything.
fn parse_opt_duration(spec: &Option<String>) -> Result<Option<chrono::Duration>, String> {
    spec.as_deref().map(parse_duration).transpose()
}

/// Parse a duration like `30d`, `12h`, `45m`, `90s`.
fn parse_duration(spec: &str) -> Result<chrono::Duration, String> {
    let spec = spec.trim();
    let (num, unit) = spec.split_at(
        spec.find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("missing unit in duration '{spec}'"))?,
    );
    let n: i64 = num.parse().map_err(|_| format!("bad number in '{spec}'"))?;
    match unit {
        "d" => Ok(chrono::Duration::days(n)),
        "h" => Ok(chrono::Duration::hours(n)),
        "m" => Ok(chrono::Duration::minutes(n)),
        "s" => Ok(chrono::Duration::seconds(n)),
        other => Err(format!("unknown duration unit '{other}' (use d/h/m/s)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30d").unwrap(), chrono::Duration::days(30));
        assert_eq!(parse_duration("12h").unwrap(), chrono::Duration::hours(12));
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("30x").is_err());
    }
}
