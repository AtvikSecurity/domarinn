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
//! * `FAKE_OUTPUT`   — the output string for `fixed` mode.
//! * `FAKE_CALL_LOG` — if set, append one line per invocation to this file, so a
//!   test can assert how many times the provider was actually called (cache
//!   hits, short-circuits, retries).
//!
//! It reads one JSON request from stdin and writes one JSON response to stdout.

use std::io::{Read, Write};

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
        let retriable = kind == "retriable";
        let resp = serde_json::json!({
            "output": "",
            "error": {"message": "scripted failure", "retriable": retriable}
        });
        let _ = writeln!(std::io::stdout(), "{resp}");
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

    let resp = serde_json::json!({
        "output": output,
        "usage": {"input_tokens": 1, "output_tokens": 1}
    });
    let _ = writeln!(std::io::stdout(), "{resp}");
}
