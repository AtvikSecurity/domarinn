//! Where each `run --format` goes.
//!
//! `--format` takes `FMT` or `FMT=PATH`, so one run can write JSON to one file
//! and JUnit to another. Planning the destinations is kept apart from
//! [`crate::output`], which renders: every refusal here happens before the
//! suite loads, so a command that would have overwritten one format with
//! another costs no provider calls.

use std::path::{Path, PathBuf};

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
    for (i, later) in emissions.iter().enumerate() {
        let Some(earlier) = emissions[..i].iter().find(|e| e.out == later.out) else {
            continue;
        };
        let (a, b) = (earlier.format, later.format);
        let clash = match &later.out {
            None => "would both write to stdout, interleaving into output neither parser can read"
                .to_string(),
            Some(path) => format!(
                "would both write {}, and only the last would survive",
                path.display()
            ),
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

    #[test]
    fn plan_refuses_an_out_nothing_would_be_written_to() {
        let err = plan(&["json=r.json"], Some("f")).unwrap_err();
        assert!(err.contains("--out has nothing to write"), "{err}");
    }
}
