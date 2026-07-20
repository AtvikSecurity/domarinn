//! A fake provider speaking the exec JSON protocol, for tests and examples.
//!
//! Behavior is scripted via the `FAKE_MODE` environment variable:
//! * `echo` (default) — output is the `user_input` var, or the whole vars map.
//! * `fixed` — output is the value of `FAKE_OUTPUT`.
//! * `exit:<n>` — exit with code `<n>` before writing anything (infra error).
//! * `garbage` — write non-JSON to stdout (protocol violation).
//!
//! It reads one JSON request from stdin and writes one JSON response to stdout.

use std::io::{Read, Write};

fn main() {
    let mode = std::env::var("FAKE_MODE").unwrap_or_else(|_| "echo".to_string());

    if let Some(code) = mode.strip_prefix("exit:") {
        std::process::exit(code.parse().unwrap_or(1));
    }
    if mode == "garbage" {
        print!("this is not json");
        let _ = std::io::stdout().flush();
        return;
    }

    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        eprintln!("fake-provider: failed to read stdin");
        std::process::exit(1);
    }
    let req: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::Value::Null);

    let output = match mode.as_str() {
        "fixed" => serde_json::Value::String(std::env::var("FAKE_OUTPUT").unwrap_or_default()),
        _ => {
            // echo: prefer a `user_input` var, else the entire vars object.
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
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{resp}");
}
