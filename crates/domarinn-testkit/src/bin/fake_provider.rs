//! A fake provider speaking the exec JSON protocol, for tests and complexity
//! harnesses.
//!
//! Behavior is scripted via environment variables:
//! * `FAKE_MODE` (default `echo`):
//!   - `echo`        — output is the `user_input` var, or the whole vars map.
//!   - `fixed`       — output is the value of `FAKE_OUTPUT`.
//!   - `delay:<ms>`  — sleep `<ms>` milliseconds, then echo (latency/concurrency).
//!   - `error:<retriable|fatal>` — return a protocol-level error.
//!   - `exit:<n>`    — exit with code `<n>` before writing (infra error).
//!   - `garbage`     — write non-JSON to stdout (protocol violation).
//!   - `empty:<r>`   — empty output reported with `empty_reason: <r>`.
//!   - `usage:cache` — echo, with cache-read and cache-write token counts.
//! * `FAKE_OUTPUT`   — the output string for `fixed` mode.
//! * `FAKE_STOP_REASON` — sets `stop_reason` on the response.
//! * `FAKE_MODEL`    — sets `model` (the model the child actually used).
//! * `FAKE_ERROR_CLASS`   — sets `error.class` in the `error:` modes.
//! * `FAKE_ERROR_DETAILS` — raw JSON for `error.details` in the `error:` modes.
//! * `FAKE_RETRY_AFTER_MS` — sets `error.retry_after_ms`.
//! * `FAKE_CALL_LOG` — if set, append one line per invocation to this file, so a
//!   test can assert how many times the provider was actually called (cache
//!   hits, short-circuits, retries).
//!
//! It reads one JSON request from stdin and writes one JSON response to stdout.
//!
//! Responses are built through `domarinn_protocol`'s types rather than `json!`
//! literals, so this doubles as the workspace's smoke test that the protocol
//! crate is actually pleasant to write a provider against.

use std::io::{Read, Write};

use domarinn_protocol::{ProtocolError, ProviderResp, Usage};

fn main() {
    let mode = std::env::var("FAKE_MODE").unwrap_or_else(|_| "echo".to_string());

    // Read the whole request first so a broken pipe never surprises the parent.
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let req: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);

    if let Some(path) = std::env::var_os("FAKE_CALL_LOG") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "call");
        }
    }

    if let Some(code) = mode.strip_prefix("exit:") {
        std::process::exit(code.parse().unwrap_or(1));
    }
    if mode == "garbage" {
        print!("this is not json");
        let _ = std::io::stdout().flush();
        return;
    }
    if let Some(ms) = mode.strip_prefix("delay:") {
        if let Ok(ms) = ms.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
    if let Some(kind) = mode.strip_prefix("error:") {
        emit(ProviderResp {
            output: serde_json::Value::String(String::new()),
            error: Some(ProtocolError {
                message: "scripted failure".into(),
                retriable: kind == "retriable",
                class: env("FAKE_ERROR_CLASS"),
                details: env("FAKE_ERROR_DETAILS").and_then(|raw| serde_json::from_str(&raw).ok()),
                retry_after_ms: env("FAKE_RETRY_AFTER_MS").and_then(|v| v.parse().ok()),
            }),
            ..Default::default()
        });
        return;
    }
    if let Some(reason) = mode.strip_prefix("empty:") {
        emit(ProviderResp {
            output: serde_json::Value::String(String::new()),
            empty_reason: Some(reason.to_string()),
            stop_reason: env("FAKE_STOP_REASON"),
            model: env("FAKE_MODEL"),
            ..Default::default()
        });
        return;
    }

    let output = match mode.as_str() {
        "fixed" => serde_json::Value::String(std::env::var("FAKE_OUTPUT").unwrap_or_default()),
        _ => {
            let vars = req.get("vars").cloned().unwrap_or(serde_json::Value::Null);
            match vars.get("user_input") {
                Some(v) => v.clone(),
                None => vars,
            }
        }
    };

    let usage = if mode == "usage:cache" {
        Usage {
            input_tokens: 1,
            output_tokens: 1,
            cache_read_tokens: Some(100),
            cache_write_tokens: Some(40),
            cache_write_1h_tokens: Some(10),
        }
    } else {
        Usage {
            input_tokens: 1,
            output_tokens: 1,
            ..Default::default()
        }
    };

    emit(ProviderResp {
        output,
        usage: Some(usage),
        stop_reason: env("FAKE_STOP_REASON"),
        model: env("FAKE_MODEL"),
        ..Default::default()
    });
}

/// Write the one response document this process is allowed to produce.
fn emit(resp: ProviderResp) {
    let body = serde_json::to_string(&resp).expect("a ProviderResp serializes");
    let _ = writeln!(std::io::stdout(), "{body}");
}

/// A set, non-empty environment variable. Empty is treated as unset so a test
/// can neutralize an inherited value without unsetting it.
fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
