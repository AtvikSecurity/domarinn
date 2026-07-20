//! Running test generators over the exec protocol.
//!
//! A generator command receives a `generate_tests` request and returns either a
//! `{ "tests": [...] }` object or JSONL (one test object per line). Produced
//! tests flow through the same defaults-merge and id pipeline as file/inline
//! tests.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::Value as Json;

use crate::config::{GeneratorSpec, TestCase};
use crate::exec::{run_exec_raw, ExecError};
use crate::exec_protocol::{Envelope, GenerateReq, Kind};

const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error(transparent)]
    Exec(#[from] ExecError),
    #[error("generator {command:?} returned invalid tests: {message}")]
    BadTests {
        command: Vec<String>,
        message: String,
    },
}

/// Run all generators and return the produced test cases (ids assigned).
pub async fn resolve_generators(
    generators: &[GeneratorSpec],
    base_dir: &Path,
) -> Result<Vec<TestCase>, GenerateError> {
    let mut out = Vec::new();
    for gen in generators {
        out.extend(run_one(gen, base_dir).await?);
    }
    Ok(out)
}

async fn run_one(gen: &GeneratorSpec, base_dir: &Path) -> Result<Vec<TestCase>, GenerateError> {
    let request = serde_json::to_value(GenerateReq {
        envelope: Envelope::new(Kind::GenerateTests),
        config: gen.config.clone().unwrap_or(Json::Null),
    })
    .map_err(|e| GenerateError::BadTests {
        command: gen.command.clone(),
        message: format!("serializing request: {e}"),
    })?;

    let timeout = Duration::from_millis(gen.timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS));
    let stdout = run_exec_raw(
        &gen.command,
        &BTreeMap::new(),
        Some(base_dir),
        timeout,
        &request,
    )
    .await?;

    let values = parse_tests(&stdout, &gen.command)?;
    let stem = gen
        .command
        .first()
        .and_then(|c| Path::new(c).file_stem())
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "generated".to_string());

    let mut cases = Vec::with_capacity(values.len());
    for (i, value) in values.into_iter().enumerate() {
        let mut tc: TestCase =
            serde_json::from_value(value).map_err(|e| GenerateError::BadTests {
                command: gen.command.clone(),
                message: e.to_string(),
            })?;
        if tc.id.is_none() {
            tc.id = Some(format!("{stem}/{i}"));
        }
        cases.push(tc);
    }
    Ok(cases)
}

/// Accept either a `{ "tests": [...] }` object or JSONL (one object per line).
fn parse_tests(stdout: &str, command: &[String]) -> Result<Vec<Json>, GenerateError> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    // Sniff: a leading '{' that parses as a whole document → object form.
    if trimmed.starts_with('{') {
        if let Ok(Json::Object(mut map)) = serde_json::from_str::<Json>(trimmed) {
            match map.remove("tests") {
                Some(Json::Array(items)) => return Ok(items),
                _ => {
                    return Err(GenerateError::BadTests {
                        command: command.to_vec(),
                        message: "object form must have a 'tests' array".into(),
                    })
                }
            }
        }
    }
    // JSONL form.
    let mut items = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Json = serde_json::from_str(line).map_err(|e| GenerateError::BadTests {
            command: command.to_vec(),
            message: format!("invalid JSONL line: {e}"),
        })?;
        items.push(value);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn object_form_generator() {
        let gen = GeneratorSpec {
            command: vec![
                "sh".into(),
                "-c".into(),
                "cat >/dev/null; printf '{\"tests\":[{\"vars\":{\"x\":\"1\"}},{\"id\":\"named\",\"vars\":{\"x\":\"2\"}}]}'".into(),
            ],
            config: None,
            timeout_ms: Some(5000),
        };
        let cases = resolve_generators(std::slice::from_ref(&gen), Path::new("."))
            .await
            .unwrap();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].id.as_deref(), Some("sh/0"));
        assert_eq!(cases[1].id.as_deref(), Some("named"));
    }

    #[tokio::test]
    async fn jsonl_form_generator() {
        let gen = GeneratorSpec {
            command: vec![
                "sh".into(),
                "-c".into(),
                "cat >/dev/null; printf '{\"vars\":{\"x\":\"1\"}}\\n{\"vars\":{\"x\":\"2\"}}\\n'"
                    .into(),
            ],
            config: None,
            timeout_ms: Some(5000),
        };
        let cases = resolve_generators(std::slice::from_ref(&gen), Path::new("."))
            .await
            .unwrap();
        assert_eq!(cases.len(), 2);
    }
}
