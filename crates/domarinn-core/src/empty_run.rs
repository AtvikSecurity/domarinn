//! Why a run resolved to zero cases.
//!
//! A suite that graded nothing used to exit 0 with a full pass rate over no
//! cells — indistinguishable, from CI's point of view, from a suite that graded
//! everything and found no problems. That is the worst possible outcome for a
//! gate: it is green, and it is green because it did nothing.
//!
//! The variants are separate because the *fix* is different for each. A suite
//! with no `tests:` block is a missing block; a glob that matched nothing is
//! usually a path; a filter that excluded everything is usually a typo or a
//! shard that legitimately has no work. Collapsing them into "no tests to run"
//! would make the reader do the diagnosis the harness already did.

use crate::result::FilterSpec;

/// A `file://` source and what it actually produced.
///
/// Recorded during expansion rather than derived afterwards, so a zero-case run
/// can name the glob that matched no files instead of reporting a total of
/// zero and leaving the reader to work out which of five sources was empty.
#[derive(Debug, Clone)]
pub struct GlobReport {
    pub spec: String,
    pub files: usize,
    pub cases: usize,
}

#[derive(Debug, Clone)]
pub enum EmptyRun {
    /// The suite declares no test sources at all.
    NoTestSources,
    /// Every source resolved, but together they produced nothing.
    SourcesProducedNothing {
        empty_globs: Vec<String>,
        empty_generators: Vec<Vec<String>>,
    },
    /// Tests exist, but the active filters excluded every one of them.
    FilteredOut {
        tests: usize,
        filters: FilterSpec,
        /// A few ids from the suite, so a typo'd `--filter` is visible against
        /// what was actually there rather than described in the abstract.
        examples: Vec<String>,
    },
    /// `--provider` matched none of the suite's providers.
    NoProvidersSelected {
        requested: Vec<String>,
        available: Vec<String>,
    },
    /// `--prompt` matched none of the suite's prompts.
    NoPromptsSelected {
        requested: Vec<String>,
        available: Vec<String>,
    },
}

/// How many suite test ids to quote back when a filter matched nothing.
const MAX_EXAMPLES: usize = 5;

impl EmptyRun {
    /// Up to [`MAX_EXAMPLES`] ids, for the `FilteredOut` hint.
    pub fn examples(ids: impl IntoIterator<Item = String>) -> Vec<String> {
        ids.into_iter().take(MAX_EXAMPLES).collect()
    }
}

impl std::fmt::Display for EmptyRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "this run would grade nothing: ")?;
        match self {
            EmptyRun::NoTestSources => write!(
                f,
                "the suite defines no tests. Add a `tests:` block with inline \
                 cases, a `file://` glob, or a generator."
            ),
            EmptyRun::SourcesProducedNothing {
                empty_globs,
                empty_generators,
            } => {
                write!(f, "every test source produced zero tests")?;
                for spec in empty_globs {
                    write!(f, "; `{spec}` matched no files")?;
                }
                for command in empty_generators {
                    write!(f, "; generator {command:?} returned no tests")?;
                }
                write!(f, ". Pass --allow-empty if that is expected.")
            }
            EmptyRun::FilteredOut {
                tests,
                filters,
                examples,
            } => {
                write!(f, "the active filters excluded all {tests} tests (")?;
                let mut parts = Vec::new();
                if !filters.tags.is_empty() {
                    parts.push(format!("tags={:?}", filters.tags));
                }
                if !filters.filters.is_empty() {
                    parts.push(format!("filters={:?}", filters.filters));
                }
                if !filters.providers.is_empty() {
                    parts.push(format!("providers={:?}", filters.providers));
                }
                if !filters.prompts.is_empty() {
                    parts.push(format!("prompts={:?}", filters.prompts));
                }
                write!(f, "{})", parts.join(", "))?;
                if !examples.is_empty() {
                    write!(f, ". Test ids include: {}", examples.join(", "))?;
                }
                write!(f, ". Pass --allow-empty if an empty shard is expected.")
            }
            EmptyRun::NoProvidersSelected {
                requested,
                available,
            } => write!(
                f,
                "--provider {requested:?} matches none of the suite's providers: {}",
                available.join(", ")
            ),
            EmptyRun::NoPromptsSelected {
                requested,
                available,
            } => write!(
                f,
                "--prompt {requested:?} matches none of the suite's prompts: {}",
                available.join(", ")
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> FilterSpec {
        FilterSpec {
            tags: vec!["smoke".into()],
            filters: vec!["expert/*".into()],
            providers: Vec::new(),
            prompts: Vec::new(),
        }
    }

    #[test]
    fn no_sources_says_what_to_add() {
        let msg = EmptyRun::NoTestSources.to_string();
        assert!(msg.contains("defines no tests"));
        assert!(msg.contains("tests:"), "should name the block to add");
    }

    /// The reason `SourcesProducedNothing` carries the specs at all: a suite
    /// with five globs should not make the reader bisect to find the empty one.
    #[test]
    fn an_empty_glob_is_named() {
        let msg = EmptyRun::SourcesProducedNothing {
            empty_globs: vec!["file://cases/*.yaml".into()],
            empty_generators: vec![vec!["python3".into(), "gen.py".into()]],
        }
        .to_string();
        assert!(msg.contains("file://cases/*.yaml"));
        assert!(msg.contains("gen.py"));
        assert!(msg.contains("--allow-empty"));
    }

    #[test]
    fn a_filter_miss_quotes_the_filters_and_some_real_ids() {
        let msg = EmptyRun::FilteredOut {
            tests: 42,
            filters: spec(),
            examples: vec!["experts/0".into(), "experts/1".into()],
        }
        .to_string();
        assert!(msg.contains("all 42 tests"));
        assert!(msg.contains("smoke"));
        assert!(msg.contains("expert/*"));
        assert!(msg.contains("experts/0"), "real ids make a typo visible");
    }

    /// A misspelled `--provider` was a completely silent green run. Naming what
    /// was available turns it into a one-glance fix.
    #[test]
    fn an_unknown_provider_names_the_available_ones() {
        let msg = EmptyRun::NoProvidersSelected {
            requested: vec!["clade".into()],
            available: vec!["claude".into(), "gpt".into()],
        }
        .to_string();
        assert!(msg.contains("clade"));
        assert!(msg.contains("claude, gpt"));
    }

    #[test]
    fn examples_are_capped() {
        let ids = (0..50).map(|i| format!("t{i}"));
        assert_eq!(EmptyRun::examples(ids).len(), MAX_EXAMPLES);
    }
}
