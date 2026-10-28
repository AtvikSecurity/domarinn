//! Where each `run --format` goes.
//!
//! `--format` takes `FMT` or `FMT=PATH`, so one run can write JSON to one file
//! and JUnit to another. Planning the destinations is kept apart from
//! [`crate::output`], which renders: every refusal here happens before the
//! suite loads, so a command that would have overwritten one format with
//! another costs no provider calls.

use std::path::{Component, Path, PathBuf};

use crate::output::Format;

/// One `run --format` argument: a format, optionally bound to its own file
/// (`json=results.json`). A bare format goes wherever `--out` points, or to
/// stdout.
#[derive(Clone, Debug)]
pub struct FormatSpec {
    pub format: Format,
    pub path: Option<PathBuf>,
}

/// The clap value parser for [`FormatSpec`]: `FMT` or `FMT=PATH`.
///
/// Split on the *first* `=`, so a path that itself contains one still works —
/// no format name does.
pub fn parse_format_spec(s: &str) -> Result<FormatSpec, String> {
    let (name, path) = match s.split_once('=') {
        Some((name, path)) => (name, Some(path)),
        None => (s, None),
    };
    let format = <Format as clap::ValueEnum>::from_str(name, false).map_err(|_| {
        format!("unknown format '{name}' (expected one of: table, json, jsonl, junit, md)")
    })?;
    let path = match path {
        Some("") => {
            return Err(format!(
                "'{s}' names no file after '='; write `{name}=PATH`, or drop the '=' \
                 to use --out or stdout"
            ));
        }
        Some(p) => Some(PathBuf::from(p)),
        None => None,
    };
    Ok(FormatSpec { format, path })
}

/// One rendering a run will write: a format and its destination (`None` is
/// stdout).
#[derive(Debug)]
pub struct Emission {
    pub format: Format,
    pub out: Option<PathBuf>,
}

/// Resolve `run`'s `--format` / `--out` arguments into one [`Emission`] per
/// format, refusing any combination where two formats would share a
/// destination.
///
/// Sharing one is never what anybody wanted: two formats written to one file
/// leave only the last (each write truncates), and two on stdout concatenate
/// into a stream no parser of either format can read. Both used to happen
/// silently. The refusal runs before the suite does, so getting it wrong costs
/// no provider calls. The single-format forms — `--format X --out f`, and no
/// `--format` at all — resolve exactly as they always have.
pub fn plan_emissions(specs: &[FormatSpec], out: Option<&Path>) -> Result<Vec<Emission>, String> {
    if specs.is_empty() {
        return Ok(vec![Emission {
            format: Format::Table,
            out: out.map(Path::to_path_buf),
        }]);
    }
    if out.is_some() && specs.iter().all(|s| s.path.is_some()) {
        return Err(
            "--out has nothing to write: every --format names its own file (FMT=PATH). \
             Drop --out, or pass one bare --format for it to receive"
                .to_string(),
        );
    }
    let emissions: Vec<Emission> = specs
        .iter()
        .map(|s| Emission {
            format: s.format,
            out: s.path.clone().or_else(|| out.map(Path::to_path_buf)),
        })
        .collect();
    // Compared by where they land, not by how they are spelled: `./r.json`,
    // `r.json` and `out/../r.json` are one file.
    let keys: Vec<Option<PathBuf>> = emissions
        .iter()
        .map(|e| e.out.as_deref().map(destination_key))
        .collect();
    for (i, later) in emissions.iter().enumerate() {
        let Some(earlier) = (0..i)
            .find(|&j| same_destination(&keys[j], &keys[i]))
            .map(|j| &emissions[j])
        else {
            continue;
        };
        let (a, b) = (earlier.format, later.format);
        let clash = match (&earlier.out, &later.out) {
            (Some(first), Some(second)) if first != second => format!(
                "would both write one file ({} and {} are the same path), and only the \
                 last would survive",
                first.display(),
                second.display()
            ),
            (_, Some(path)) => format!(
                "would both write {}, and only the last would survive",
                path.display()
            ),
            (_, None) => {
                "would both write to stdout, interleaving into output neither parser can read"
                    .to_string()
            }
        };
        let remedy = if a == b {
            format!("pass --format {} once", a.name())
        } else {
            format!(
                "give each its own file: --format {}={} --format {}={}",
                a.name(),
                a.example_file(),
                b.name(),
                b.example_file()
            )
        };
        return Err(format!(
            "--format {} and --format {} {clash}; {remedy}",
            a.name(),
            b.name()
        ));
    }
    Ok(emissions)
}

/// What a destination path names, as near as can be told before writing it.
///
/// Absolute against the current directory, with `.` dropped and `..` resolved
/// lexically. Where the parent directory exists it is canonicalized as well, so
/// a symlinked directory and its target compare equal. A lexical `..` can be
/// wrong under a symlink — the price is refusing a combination that would in
/// fact have been safe, never allowing one that overwrites.
fn destination_key(path: &Path) -> PathBuf {
    let absolute = match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    };
    let mut lexical = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                lexical.pop();
            }
            other => lexical.push(other),
        }
    }
    match (lexical.parent(), lexical.file_name()) {
        (Some(parent), Some(name)) => match parent.canonicalize() {
            Ok(parent) => parent.join(name),
            Err(_) => lexical,
        },
        _ => lexical,
    }
}

/// Two destination keys name one place: both stdout, the same normalized
/// path, or — when both files already exist — the same file under two names
/// (a symlink, or a hard link on Unix).
fn same_destination(a: &Option<PathBuf>, b: &Option<PathBuf>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a == b || same_existing_file(a, b),
        _ => false,
    }
}

#[cfg(unix)]
fn same_existing_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_existing_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> FormatSpec {
        parse_format_spec(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn format_spec_parses_bare_and_paired_forms() {
        let bare = spec("junit");
        assert!(bare.format == Format::Junit && bare.path.is_none());
        let paired = spec("json=out/r.json");
        assert!(paired.format == Format::Json);
        assert_eq!(paired.path.as_deref(), Some(Path::new("out/r.json")));
        // Split on the first `=` only: the path keeps the rest.
        assert_eq!(spec("md=a=b.md").path.as_deref(), Some(Path::new("a=b.md")));
    }

    #[test]
    fn format_spec_rejects_unknown_formats_and_empty_paths() {
        assert!(parse_format_spec("xml")
            .unwrap_err()
            .contains("unknown format 'xml'"));
        assert!(parse_format_spec("xml=r.xml").is_err());
        assert!(parse_format_spec("json=")
            .unwrap_err()
            .contains("names no file"));
    }

    fn plan(specs: &[&str], out: Option<&str>) -> Result<Vec<Emission>, String> {
        let specs: Vec<FormatSpec> = specs.iter().map(|s| spec(s)).collect();
        plan_emissions(&specs, out.map(Path::new))
    }

    #[test]
    fn plan_keeps_the_single_format_forms_unchanged() {
        let none = plan(&[], None).unwrap();
        assert!(none.len() == 1 && none[0].format == Format::Table && none[0].out.is_none());
        let table_out = plan(&[], Some("t.txt")).unwrap();
        assert_eq!(table_out[0].out.as_deref(), Some(Path::new("t.txt")));
        // The composite action's invocation.
        let junit = plan(&["junit"], Some("results.xml")).unwrap();
        assert!(junit.len() == 1 && junit[0].format == Format::Junit);
        assert_eq!(junit[0].out.as_deref(), Some(Path::new("results.xml")));
    }

    #[test]
    fn plan_routes_each_paired_format_to_its_own_file() {
        let e = plan(&["json=r.json", "junit=r.xml"], None).unwrap();
        assert_eq!(e[0].out.as_deref(), Some(Path::new("r.json")));
        assert_eq!(e[1].out.as_deref(), Some(Path::new("r.xml")));
        // One bare format beside paired ones: the bare one takes `--out`...
        let e = plan(&["junit", "json=r.json"], Some("r.xml")).unwrap();
        assert_eq!(e[0].out.as_deref(), Some(Path::new("r.xml")));
        // ...or stdout without it.
        let e = plan(&["table", "json=r.json"], None).unwrap();
        assert!(e[0].out.is_none());
    }

    #[test]
    fn plan_refuses_two_formats_sharing_a_destination() {
        let err = plan(&["json", "junit"], Some("f")).unwrap_err();
        assert!(err.contains("only the last would survive"), "{err}");
        assert!(
            err.contains("--format json=results.json --format junit=results.xml"),
            "the remedy is spelled out: {err}"
        );
        let err = plan(&["json", "junit"], None).unwrap_err();
        assert!(err.contains("stdout"), "{err}");
        let err = plan(&["json=r", "junit=r"], None).unwrap_err();
        assert!(err.contains("would both write r"), "{err}");
        let err = plan(&["json", "json"], None).unwrap_err();
        assert!(err.contains("pass --format json once"), "{err}");
    }

    /// The same file spelled two ways is still one file: comparing spellings
    /// let one format silently overwrite the other.
    #[test]
    fn plan_refuses_one_file_spelled_two_ways() {
        let err = plan(&["json=./r.json", "junit=r.json"], None).unwrap_err();
        assert!(err.contains("only the last would survive"), "{err}");
        assert!(
            err.contains("./r.json and r.json are the same path"),
            "the user's own spellings are quoted: {err}"
        );
        // A bare format takes `--out`, which can collide with a paired path.
        let err = plan(&["json=r.json", "junit"], Some("./r.json")).unwrap_err();
        assert!(err.contains("r.json and ./r.json"), "{err}");
        let err = plan(&["json=out/../r.json", "md=r.json"], None).unwrap_err();
        assert!(err.contains("only the last would survive"), "{err}");
        // Normalizing must not merge files that really are different.
        plan(&["json=a/r.json", "junit=b/r.json"], None).unwrap();
    }

    /// Two names for one existing file — a symlink — are caught too.
    #[cfg(unix)]
    #[test]
    fn plan_refuses_a_symlink_to_another_destination() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("r.json");
        let link = dir.path().join("link.json");
        std::fs::write(&real, "{}").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (real, link) = (real.display().to_string(), link.display().to_string());
        let err = plan(&[&format!("json={real}"), &format!("md={link}")], None).unwrap_err();
        assert!(err.contains("only the last would survive"), "{err}");
    }

    #[test]
    fn plan_refuses_an_out_nothing_would_be_written_to() {
        let err = plan(&["json=r.json"], Some("f")).unwrap_err();
        assert!(err.contains("--out has nothing to write"), "{err}");
    }
}
