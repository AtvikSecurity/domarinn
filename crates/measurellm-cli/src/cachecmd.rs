//! The `cache` subcommands: stats, path, gc, clear.

use clap::Subcommand;
use measurellm_cache::LocalDiskCache;
use measurellm_core::cache::{CacheBackend, PurgeFilter};

use crate::exit;

#[derive(Subcommand)]
pub enum CacheCmd {
    /// Show cache entry count and size.
    Stats,
    /// Print the local cache directory path.
    Path,
    /// Remove cache entries older than a duration (e.g. 30d, 12h).
    Gc {
        #[arg(long)]
        older_than: Option<String>,
    },
    /// Remove all cache entries.
    Clear,
}

pub fn execute(cmd: CacheCmd) -> u8 {
    let cache = LocalDiskCache::default_project();
    match cmd {
        CacheCmd::Path => {
            println!("{}", cache.root().display());
            exit::OK
        }
        CacheCmd::Stats => block_on(async {
            match cache.stats().await {
                Ok(s) => {
                    println!(
                        "{} entries, {:.2} MiB",
                        s.entries,
                        s.total_bytes as f64 / (1024.0 * 1024.0)
                    );
                    exit::OK
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit::INFRA
                }
            }
        }),
        CacheCmd::Gc { older_than } => {
            let filter = match older_than {
                Some(spec) => match parse_duration(&spec) {
                    Ok(d) => PurgeFilter {
                        older_than: Some(d),
                    },
                    Err(e) => {
                        eprintln!("error: {e}");
                        return exit::USAGE;
                    }
                },
                None => PurgeFilter::default(),
            };
            block_on(async {
                match cache.purge(&filter).await {
                    Ok(n) => {
                        println!("removed {n} cache entr{}", if n == 1 { "y" } else { "ies" });
                        exit::OK
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        exit::INFRA
                    }
                }
            })
        }
        CacheCmd::Clear => block_on(async {
            match cache.purge(&PurgeFilter::default()).await {
                Ok(n) => {
                    println!("cleared {n} cache entr{}", if n == 1 { "y" } else { "ies" });
                    exit::OK
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit::INFRA
                }
            }
        }),
    }
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
