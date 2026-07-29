//! The `cache` subcommands: stats, path, gc, clear.
//!
//! All four operate on the local disk tier only — see the `cache` help in
//! `main.rs`. Each reports the read-only legacy tier a run would layer
//! underneath ([`crate::cachecfg::LocalRoot`]) whenever one exists, because a
//! `clear` that leaves it behind is a `clear` the next run undoes.

use std::path::PathBuf;

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
    /// Remove cache entries older than a duration (e.g. 30d, 12h).
    Gc {
        /// Required: `gc` is an age-bounded purge. To remove everything, use
        /// `domarinn cache clear`.
        // Not `required = true`: clap's own message would say only that the
        // argument is missing, and the whole hazard here is that the obvious
        // reading of a bare `gc` is "tidy up a bit", not "delete everything".
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        #[command(flatten)]
        which: Where,
    },
    /// Remove all cache entries.
    Clear(Where),
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
    /// Resolve to the tiers a run would use, by the same precedence as
    /// `domarinn run`: `--cache-dir`, then `DOMARINN_CACHE_DIR`, then
    /// `.domarinn/cache` beside the suite — plus any legacy tier underneath.
    fn local_root(&self) -> LocalRoot {
        let base = self
            .path
            .clone()
            .map(|p| {
                let file = domarinn_core::loader::resolve_suite_path(&p);
                domarinn_core::loader::suite_base_dir(&file)
            })
            .unwrap_or_else(|| PathBuf::from("."));
        crate::cachecfg::local_root(self.cache_dir.as_deref(), &base)
    }
}

pub fn execute(cmd: CacheCmd) -> u8 {
    let root = match &cmd {
        CacheCmd::Stats(w) | CacheCmd::Path(w) | CacheCmd::Clear(w) => w.local_root(),
        CacheCmd::Gc { which, .. } => which.local_root(),
    };
    // The same two tiers `cachecfg::build_cache` gives a run, minus the
    // read-through wrapper: these commands address each tier, so a purge has to
    // reach the one a run only ever reads.
    let primary = LocalDiskCache::new(&root.root);
    let legacy = root.legacy.as_ref().map(LocalDiskCache::new);

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
                match legacy.stats().await {
                    Ok(s) => println!(
                        "legacy tier {}: {} entries, {} (read-only during runs; `cache clear` removes it)",
                        legacy.root().display(),
                        s.entries,
                        mib(s.total_bytes)
                    ),
                    Err(e) => return infra(&e),
                }
            }
            exit::OK
        }),
        CacheCmd::Gc { older_than, .. } => {
            let Some(spec) = older_than else {
                eprintln!(
                    "error: cache gc needs --older-than (e.g. --older-than 30d); \
                     to remove every entry, use `domarinn cache clear`"
                );
                return exit::USAGE;
            };
            let older_than = match parse_duration(&spec) {
                Ok(d) => Some(d),
                Err(e) => {
                    eprintln!("error: {e}");
                    return exit::USAGE;
                }
            };
            block_on(async move {
                purge_tiers(
                    &primary,
                    legacy.as_ref(),
                    &PurgeFilter { older_than },
                    "removed",
                )
                .await
            })
        }
        CacheCmd::Clear(_) => block_on(async move {
            purge_tiers(
                &primary,
                legacy.as_ref(),
                &PurgeFilter::default(),
                "cleared",
            )
            .await
        }),
    }
}

/// Purge both tiers, reporting each. `verb` leads each line ("removed" for a
/// `gc`, "cleared" for a `clear`).
async fn purge_tiers(
    primary: &LocalDiskCache,
    legacy: Option<&LocalDiskCache>,
    filter: &PurgeFilter,
    verb: &str,
) -> u8 {
    match primary.purge(filter).await {
        Ok(n) => println!("{verb} {n} cache entr{}", plural(n)),
        Err(e) => return infra(&e),
    }
    if let Some(legacy) = legacy {
        match legacy.purge(filter).await {
            Ok(n) => println!(
                "{verb} {n} legacy cache entr{} from {}",
                plural(n),
                legacy.root().display()
            ),
            Err(e) => return infra(&e),
        }
    }
    exit::OK
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

fn block_on<F: std::future::Future<Output = u8>>(fut: F) -> u8 {
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(fut),
        Err(e) => {
            eprintln!("error: {e}");
            exit::INFRA
        }
    }
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
