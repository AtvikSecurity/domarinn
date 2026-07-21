//! Shared tracing-subscriber setup for domarinn binaries.
//! Libraries emit `tracing` events; only binaries call [`init`], once, at startup.

/// How log lines are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Resolve via env var, then profile + TTY. See [`resolve_format`].
    #[default]
    Auto,
    /// Human-readable, multi-line. ANSI when stderr is a TTY; no timestamps
    /// in [`LogProfile::Cli`] (short commands don't need wall-clock times).
    Pretty,
    /// Human-readable, single-line, with timestamps. ANSI when stderr is a TTY.
    Compact,
    /// One JSON object per line, for log aggregation.
    Json,
}

/// Which kind of binary is initializing logging, used to pick sane defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogProfile {
    /// A short-lived command invocation. Defaults to level `warn`.
    Cli,
    /// A long-running service. Defaults to level `info`, including `tower_http`.
    Server,
}

/// Options controlling [`init`] / [`try_init`].
#[derive(Debug, Clone)]
pub struct LogOptions {
    /// Which binary kind is initializing logging.
    pub profile: LogProfile,
    /// clap `-v` occurrence count; raises the default level.
    pub verbose: u8,
    /// Explicit format override (`--log-format`); `Auto` consults env/TTY.
    pub format: LogFormat,
}

/// Install the global `tracing` subscriber. Idempotent: a second call
/// (from this or any other init path) is a no-op rather than a panic.
pub fn init(opts: &LogOptions) {
    let _ = try_init(opts);
}

/// Install the global `tracing` subscriber, reporting failure instead of
/// silently ignoring it (e.g. because a subscriber is already set).
pub fn try_init(opts: &LogOptions) -> Result<(), tracing_subscriber::util::TryInitError> {
    use std::io::IsTerminal;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new(default_directives(opts.profile, opts.verbose))
    });

    let stderr_is_tty = std::io::stderr().is_terminal();
    let ansi = stderr_is_tty && std::env::var_os("NO_COLOR").is_none();
    let env_format = std::env::var("DOMARINN_LOG_FORMAT").ok();
    let format = resolve_format(
        opts.format,
        env_format.as_deref(),
        opts.profile,
        stderr_is_tty,
    );

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(ansi);

    match format {
        LogFormat::Json => builder.json().flatten_event(true).finish().try_init(),
        LogFormat::Compact => builder.compact().finish().try_init(),
        // Pretty in the Cli profile omits timestamps: a short-lived command's
        // own output is the clock; Server keeps them since it's long-running.
        LogFormat::Pretty if opts.profile == LogProfile::Cli => {
            builder.pretty().without_time().finish().try_init()
        }
        LogFormat::Pretty => builder.pretty().finish().try_init(),
        // resolve_format never returns Auto; explicit non-Auto values pass
        // through, and env/autodetect always resolve to a concrete format.
        LogFormat::Auto => unreachable!("resolve_format never returns Auto"),
    }
}

/// Compute the default `EnvFilter` directive string for a profile/verbosity,
/// used when `RUST_LOG` is unset.
///
/// Pure so it's unit-testable without touching process-global env state.
fn default_directives(profile: LogProfile, verbose: u8) -> String {
    const LEVELS: [&str; 4] = ["warn", "info", "debug", "trace"];
    let base = match profile {
        LogProfile::Cli => 0,
        LogProfile::Server => 1,
    };
    let index = usize::min(base + verbose as usize, LEVELS.len() - 1);
    let level = LEVELS[index];

    // `domarinn={level}` prefix-matches every `domarinn_*` crate target:
    // EnvFilter directives match targets by prefix, so this one directive
    // covers domarinn_core, domarinn_cache, domarinn_server, etc.
    match profile {
        LogProfile::Cli => format!("domarinn={level}"),
        LogProfile::Server => format!("domarinn={level},tower_http={level}"),
    }
}

/// Resolve the effective [`LogFormat`], given an explicit flag value, the
/// raw `DOMARINN_LOG_FORMAT` env value (if any), the active profile, and
/// whether stderr is a TTY.
///
/// Pure: takes env value and TTY-ness as parameters instead of reading
/// `std::env` / `IsTerminal` itself, so tests don't need to mutate
/// process-global env state.
fn resolve_format(
    explicit: LogFormat,
    env_value: Option<&str>,
    profile: LogProfile,
    stderr_is_tty: bool,
) -> LogFormat {
    if explicit != LogFormat::Auto {
        return explicit;
    }

    if let Some(value) = env_value {
        match value {
            "pretty" => return LogFormat::Pretty,
            "compact" => return LogFormat::Compact,
            "json" => return LogFormat::Json,
            other => eprintln!(
                "domarinn-logging: ignoring invalid DOMARINN_LOG_FORMAT value {other:?}; autodetecting instead"
            ),
        }
    }

    match (profile, stderr_is_tty) {
        (LogProfile::Server, true) => LogFormat::Pretty,
        // Not a TTY on a server usually means Docker/an aggregator downstream.
        (LogProfile::Server, false) => LogFormat::Json,
        (LogProfile::Cli, true) => LogFormat::Pretty,
        // Not a TTY on a CLI is usually a human reading CI logs, not a
        // machine parser, so stay human-readable rather than emitting JSON.
        (LogProfile::Cli, false) => LogFormat::Compact,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- default_directives -------------------------------------------------

    #[test]
    fn cli_directives_by_verbosity() {
        assert_eq!(default_directives(LogProfile::Cli, 0), "domarinn=warn");
        assert_eq!(default_directives(LogProfile::Cli, 1), "domarinn=info");
        assert_eq!(default_directives(LogProfile::Cli, 2), "domarinn=debug");
        assert_eq!(default_directives(LogProfile::Cli, 3), "domarinn=trace");
    }

    #[test]
    fn cli_directives_clamp_at_trace() {
        // verbose way past the table should still clamp to trace, not panic.
        assert_eq!(default_directives(LogProfile::Cli, 10), "domarinn=trace");
    }

    #[test]
    fn server_directives_include_tower_http() {
        assert_eq!(
            default_directives(LogProfile::Server, 0),
            "domarinn=info,tower_http=info"
        );
    }

    #[test]
    fn server_directives_reach_trace_at_verbose_2() {
        // base index 1 (info) + verbose 2 == index 3 == trace, exactly at
        // the top of the table (not yet clamped).
        assert_eq!(
            default_directives(LogProfile::Server, 2),
            "domarinn=trace,tower_http=trace"
        );
    }

    #[test]
    fn server_directives_clamp_beyond_trace() {
        // base index 1 + verbose 3 == index 4, out of bounds; must clamp to
        // trace rather than panicking on the table index.
        assert_eq!(
            default_directives(LogProfile::Server, 3),
            "domarinn=trace,tower_http=trace"
        );
    }

    // -- resolve_format -------------------------------------------------

    #[test]
    fn explicit_flag_beats_everything() {
        assert_eq!(
            resolve_format(LogFormat::Json, Some("pretty"), LogProfile::Cli, true),
            LogFormat::Json
        );
        assert_eq!(
            resolve_format(LogFormat::Compact, None, LogProfile::Server, false),
            LogFormat::Compact
        );
    }

    #[test]
    fn env_beats_autodetect() {
        assert_eq!(
            resolve_format(LogFormat::Auto, Some("json"), LogProfile::Cli, true),
            LogFormat::Json
        );
        assert_eq!(
            resolve_format(LogFormat::Auto, Some("pretty"), LogProfile::Server, false),
            LogFormat::Pretty
        );
        assert_eq!(
            resolve_format(LogFormat::Auto, Some("compact"), LogProfile::Cli, false),
            LogFormat::Compact
        );
    }

    #[test]
    fn invalid_env_falls_through_to_autodetect() {
        assert_eq!(
            resolve_format(LogFormat::Auto, Some("bogus"), LogProfile::Cli, true),
            LogFormat::Pretty
        );
    }

    #[test]
    fn autodetect_server_tty_is_pretty() {
        assert_eq!(
            resolve_format(LogFormat::Auto, None, LogProfile::Server, true),
            LogFormat::Pretty
        );
    }

    #[test]
    fn autodetect_server_non_tty_is_json() {
        assert_eq!(
            resolve_format(LogFormat::Auto, None, LogProfile::Server, false),
            LogFormat::Json
        );
    }

    #[test]
    fn autodetect_cli_tty_is_pretty() {
        assert_eq!(
            resolve_format(LogFormat::Auto, None, LogProfile::Cli, true),
            LogFormat::Pretty
        );
    }

    #[test]
    fn autodetect_cli_non_tty_is_compact() {
        assert_eq!(
            resolve_format(LogFormat::Auto, None, LogProfile::Cli, false),
            LogFormat::Compact
        );
    }

    // -- init / try_init idempotency -------------------------------------------------

    #[test]
    fn double_init_is_idempotent() {
        let opts = LogOptions {
            profile: LogProfile::Cli,
            verbose: 0,
            format: LogFormat::Compact,
        };

        assert!(
            try_init(&opts).is_ok(),
            "first try_init call should install the subscriber"
        );
        assert!(
            try_init(&opts).is_err(),
            "second try_init call should report a subscriber is already set"
        );

        // init() must never panic, even once a subscriber is already installed.
        init(&opts);
        init(&opts);
    }
}
