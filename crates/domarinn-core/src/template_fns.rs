//! Extra minijinja filters and functions, registered once per [`TemplateEngine`].
//!
//! Two groups, split by reproducibility:
//!
//! * **Deterministic** — same input, same output, forever. Encoders/decoders
//!   (`json_encode`/`to_json`, `json_decode`/`from_json`, `b64encode`/
//!   `b64decode`), hashes (`sha256`, `md5`, `blake3` — hex), regex helpers
//!   (`regex_replace`, `regex_match`), and text shaping (`slugify`, `truncate`).
//!   These stack on minijinja's own builtins (`trim`, `default`, `int`, `float`,
//!   `lower`, `upper`, …).
//!
//! * **Pinned non-deterministic** — a rendered prompt is persisted with a run and
//!   diffed across runs, so uncontrolled randomness would make every render
//!   spuriously differ. `now()` therefore honors `DOMARINN_NOW` (an RFC3339
//!   instant) when set and falls back to the wall clock otherwise; `uuid`,
//!   `rand`, and `randint` **require an explicit seed** — an unseeded call is a
//!   template error, not a fresh random value.
//!
//! [`TemplateEngine`]: crate::template::TemplateEngine

use minijinja::{Environment, Error, ErrorKind, Value};

/// Register every domarinn filter and function on `env`.
pub fn register(env: &mut Environment) {
    // --- Deterministic filters ---------------------------------------------
    env.add_filter("json_encode", json_encode);
    env.add_filter("to_json", json_encode);
    env.add_filter("json_decode", json_decode);
    env.add_filter("from_json", json_decode);
    env.add_filter("b64encode", b64encode);
    env.add_filter("b64decode", b64decode);
    env.add_filter("sha256", sha256_hex);
    env.add_filter("md5", md5_hex);
    env.add_filter("blake3", blake3_hex);
    env.add_filter("regex_replace", regex_replace);
    env.add_filter("regex_match", regex_match);
    env.add_filter("slugify", slugify);
    env.add_filter("truncate", truncate);

    // --- Pinned non-deterministic functions --------------------------------
    env.add_function("now", now);
    env.add_function("uuid", uuid);
    env.add_function("rand", rand);
    env.add_function("randint", randint);
}

// ---------------------------------------------------------------------------
// Deterministic
// ---------------------------------------------------------------------------

/// `value | to_json` / `value | json_encode` — serialize any value to a compact
/// JSON string.
fn json_encode(value: Value) -> Result<String, Error> {
    serde_json::to_string(&value)
        .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("json_encode: {e}")))
}

/// `str | from_json` / `str | json_decode` — parse a JSON string into a value.
fn json_decode(s: String) -> Result<Value, Error> {
    let parsed: serde_json::Value = serde_json::from_str(&s)
        .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("json_decode: {e}")))?;
    Ok(Value::from_serialize(&parsed))
}

/// `str | b64encode` — standard (padded) base64 of the string's UTF-8 bytes.
fn b64encode(s: String) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    STANDARD.encode(s.as_bytes())
}

/// `str | b64decode` — decode standard base64 back to a UTF-8 string.
fn b64decode(s: String) -> Result<String, Error> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let bytes = STANDARD
        .decode(s.as_bytes())
        .map_err(|e| Error::new(ErrorKind::InvalidOperation, format!("b64decode: {e}")))?;
    String::from_utf8(bytes).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("b64decode: not valid UTF-8: {e}"),
        )
    })
}

/// `str | sha256` — lower-case hex SHA-256 digest.
fn sha256_hex(s: String) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    to_hex(&hasher.finalize())
}

/// `str | md5` — lower-case hex MD5 digest (for legacy interop; not for security).
fn md5_hex(s: String) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(s.as_bytes());
    to_hex(&hasher.finalize())
}

/// `str | blake3` — lower-case hex BLAKE3 digest.
fn blake3_hex(s: String) -> String {
    blake3::hash(s.as_bytes()).to_hex().to_string()
}

/// `str | regex_replace(pattern, replacement)` — replace every match; `$1` etc.
/// capture-group references work in the replacement.
fn regex_replace(s: String, pattern: String, replacement: String) -> Result<String, Error> {
    let re = regex::Regex::new(&pattern).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("regex_replace: invalid pattern: {e}"),
        )
    })?;
    Ok(re.replace_all(&s, replacement.as_str()).into_owned())
}

/// `str | regex_match(pattern)` — true when the pattern matches anywhere in the
/// string.
fn regex_match(s: String, pattern: String) -> Result<bool, Error> {
    let re = regex::Regex::new(&pattern).map_err(|e| {
        Error::new(
            ErrorKind::InvalidOperation,
            format!("regex_match: invalid pattern: {e}"),
        )
    })?;
    Ok(re.is_match(&s))
}

/// `str | slugify` — an ASCII slug: runs of non-alphanumerics collapse to a
/// single `-`, letters are lower-cased, and there are no leading/trailing dashes.
/// Non-ASCII characters act as separators (they are not transliterated).
fn slugify(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_sep = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_sep && !out.is_empty() {
                out.push('-');
            }
            pending_sep = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_sep = true;
        }
    }
    out
}

/// `str | truncate(n)` — the first `n` characters (by Unicode scalar, not byte).
/// Strings of `n` or fewer characters are returned unchanged.
fn truncate(s: String, n: usize) -> String {
    s.chars().take(n).collect()
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Pinned non-deterministic
// ---------------------------------------------------------------------------

/// `now()` → RFC3339 timestamp; `now(fmt)` → strftime-formatted. Reads
/// `DOMARINN_NOW` (RFC3339) when set so persisted renders stay reproducible;
/// otherwise uses the wall clock.
fn now(fmt: Option<String>) -> Result<String, Error> {
    use chrono::format::{Item, StrftimeItems};
    use chrono::{DateTime, FixedOffset, Utc};

    let base: DateTime<FixedOffset> = match std::env::var("DOMARINN_NOW") {
        Ok(s) => DateTime::parse_from_rfc3339(&s).map_err(|e| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("DOMARINN_NOW is not a valid RFC3339 instant: {e}"),
            )
        })?,
        Err(_) => Utc::now().fixed_offset(),
    };

    match fmt {
        Some(f) => {
            // chrono panics when *rendering* an invalid strftime spec; detect it
            // up front and surface a clean template error instead.
            if StrftimeItems::new(&f).any(|item| matches!(item, Item::Error)) {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("now(): invalid strftime format `{f}`"),
                ));
            }
            Ok(base.format(&f).to_string())
        }
        None => Ok(base.to_rfc3339()),
    }
}

/// `uuid(seed)` — a deterministic, version-4-shaped UUID derived from `seed`.
/// Unseeded `uuid()` is a template error (persisted renders must be reproducible).
fn uuid(seed: Option<Value>) -> Result<String, Error> {
    let seed = seed.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidOperation,
            "uuid(seed) requires an explicit seed so renders stay reproducible, e.g. uuid(case.id)",
        )
    })?;
    let h = seed_hash(&seed);
    let mut b = [0u8; 16];
    b.copy_from_slice(&h[..16]);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10xx
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14],
        b[15]
    ))
}

/// `rand(seed)` — a deterministic float in `[0, 1)` derived from `seed`.
/// Unseeded `rand()` is a template error.
fn rand(seed: Option<Value>) -> Result<f64, Error> {
    let seed = seed.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidOperation,
            "rand(seed) requires an explicit seed so renders stay reproducible, e.g. rand(42)",
        )
    })?;
    // 53 significant bits → a double in [0, 1).
    let bits = seed_u64(&seed) >> 11;
    Ok(bits as f64 / (1u64 << 53) as f64)
}

/// `randint(low, high, seed)` — a deterministic integer in `[low, high]`
/// (inclusive) derived from `seed`.
fn randint(low: i64, high: i64, seed: Option<Value>) -> Result<i64, Error> {
    let seed = seed.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidOperation,
            "randint(low, high, seed) requires an explicit seed so renders stay reproducible",
        )
    })?;
    if high < low {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("randint: high ({high}) must be >= low ({low})"),
        ));
    }
    let span = (high - low) as u64 + 1;
    let offset = (seed_u64(&seed) % span) as i64;
    Ok(low + offset)
}

/// Hash a seed value to 32 bytes. The seed is canonicalized as JSON first, so
/// `42` and `"42"` are distinct seeds but each is stable across runs.
fn seed_hash(seed: &Value) -> [u8; 32] {
    let canonical = serde_json::to_string(seed).unwrap_or_else(|_| seed.to_string());
    *blake3::hash(canonical.as_bytes()).as_bytes()
}

fn seed_u64(seed: &Value) -> u64 {
    let h = seed_hash(seed);
    u64::from_le_bytes(h[..8].try_into().expect("32-byte hash has 8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment<'static> {
        let mut e = Environment::new();
        register(&mut e);
        e
    }

    fn render(tpl: &str) -> String {
        env().render_str(tpl, ()).unwrap()
    }

    #[test]
    fn json_round_trip() {
        assert_eq!(render("{{ [1, 2, 3] | to_json }}"), "[1,2,3]");
        assert_eq!(render("{{ {'a': 1} | json_encode }}"), r#"{"a":1}"#);
        assert_eq!(render(r#"{{ ('{"a": 7}' | from_json).a }}"#), "7");
        assert_eq!(render(r#"{{ ('[9, 8]' | json_decode)[1] }}"#), "8");
    }

    #[test]
    fn base64_round_trip() {
        assert_eq!(render("{{ 'hello' | b64encode }}"), "aGVsbG8=");
        assert_eq!(render("{{ 'aGVsbG8=' | b64decode }}"), "hello");
    }

    #[test]
    fn b64decode_rejects_garbage() {
        let err = env()
            .render_str("{{ '!!!!' | b64decode }}", ())
            .unwrap_err();
        assert!(err.to_string().contains("b64decode"), "{err}");
    }

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            render("{{ '' | sha256 }}"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            render("{{ 'abc' | sha256 }}"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn md5_known_vector() {
        assert_eq!(
            render("{{ 'abc' | md5 }}"),
            "900150983cd24fb0d6963f7d28e17f72"
        );
    }

    #[test]
    fn blake3_matches_the_crate_and_is_64_hex() {
        let out = render("{{ 'abc' | blake3 }}");
        assert_eq!(out, blake3::hash(b"abc").to_hex().to_string());
        assert_eq!(out.len(), 64);
        assert!(out.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn regex_replace_and_match() {
        assert_eq!(
            render("{{ 'a1b2c3' | regex_replace('[0-9]', '#') }}"),
            "a#b#c#"
        );
        // capture-group backreference
        assert_eq!(
            render(r"{{ 'John Smith' | regex_replace('(\\w+) (\\w+)', '$2 $1') }}"),
            "Smith John"
        );
        assert_eq!(render("{{ 'foobar' | regex_match('^foo') }}"), "true");
        assert_eq!(render("{{ 'foobar' | regex_match('^bar') }}"), "false");
    }

    #[test]
    fn regex_replace_bad_pattern_errors() {
        let err = env()
            .render_str("{{ 'x' | regex_replace('[', 'y') }}", ())
            .unwrap_err();
        assert!(err.to_string().contains("regex_replace"), "{err}");
    }

    #[test]
    fn slugify_basics() {
        assert_eq!(render("{{ 'Hello, World!' | slugify }}"), "hello-world");
        assert_eq!(render("{{ '  --Trim  Me-- ' | slugify }}"), "trim-me");
        assert_eq!(render("{{ 'Rust 1.83' | slugify }}"), "rust-1-83");
    }

    #[test]
    fn truncate_by_chars() {
        assert_eq!(render("{{ 'hello world' | truncate(5) }}"), "hello");
        assert_eq!(render("{{ 'hi' | truncate(5) }}"), "hi");
    }

    #[test]
    fn now_honors_domarinn_now() {
        std::env::set_var("DOMARINN_NOW", "2020-01-02T03:04:05Z");
        assert_eq!(render("{{ now() }}"), "2020-01-02T03:04:05+00:00");
        assert_eq!(render("{{ now('%Y-%m-%d') }}"), "2020-01-02");
        // A malformed strftime spec is a clean template error, never a panic.
        let err = env().render_str("{{ now('%Q') }}", ()).unwrap_err();
        assert!(err.to_string().contains("invalid strftime"), "{err}");
        std::env::remove_var("DOMARINN_NOW");
    }

    #[test]
    fn rand_requires_a_seed_and_is_stable() {
        // Unseeded rand() is a template error.
        assert!(env().render_str("{{ rand() }}", ()).is_err());
        // Seeded rand is stable across renders and lands in [0, 1).
        let a = render("{{ rand(42) }}");
        let b = render("{{ rand(42) }}");
        assert_eq!(a, b);
        let v: f64 = a.parse().unwrap();
        assert!((0.0..1.0).contains(&v), "rand(42) = {v}");
        // A different seed (very likely) gives a different value.
        assert_ne!(render("{{ rand(43) }}"), a);
    }

    #[test]
    fn randint_is_seeded_and_in_range() {
        assert!(env().render_str("{{ randint(1, 10) }}", ()).is_err());
        for _ in 0..5 {
            let v: i64 = render("{{ randint(1, 6, 7) }}").parse().unwrap();
            assert!((1..=6).contains(&v), "randint out of range: {v}");
        }
        // Deterministic.
        assert_eq!(
            render("{{ randint(1, 100, 'x') }}"),
            render("{{ randint(1, 100, 'x') }}")
        );
        // high < low is an error.
        assert!(env().render_str("{{ randint(10, 1, 5) }}", ()).is_err());
    }

    #[test]
    fn uuid_requires_a_seed_and_is_deterministic() {
        assert!(env().render_str("{{ uuid() }}", ()).is_err());
        let a = render("{{ uuid('case-1') }}");
        assert_eq!(a, render("{{ uuid('case-1') }}"));
        assert_ne!(a, render("{{ uuid('case-2') }}"));
        // Canonical v4 shape: 8-4-4-4-12, version nibble 4, variant 8/9/a/b.
        assert_eq!(a.len(), 36);
        let parts: Vec<&str> = a.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_hexdigit()));
        assert_eq!(&a[14..15], "4");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"));
    }
}
