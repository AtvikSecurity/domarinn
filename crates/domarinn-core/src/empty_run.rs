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
    /// `--provider` named a `fallback_only` provider, or every provider in the
    /// suite is one. Distinct from [`EmptyRun::NoProvidersSelected`] because
    /// the fix is different: the id *is* in the suite, it just never forms
    /// cells.
    FallbackOnlyExcluded {
        /// The full `--provider` list (empty when no filter was involved).
        requested: Vec<String>,
        /// The requested ids that are `fallback_only` (or, with no filter,
        /// every `fallback_only` id in the suite).
        fallback_only: Vec<String>,
        /// Providers that can form cells (empty when the whole suite is
        /// `fallback_only`).
        available: Vec<String>,
    },
    /// `--provider` named an embeddings provider — a grader helper that never
    /// forms cells, whatever the filter says.
    EmbeddingsSelected {
        /// The full `--provider` list.
        requested: Vec<String>,
        /// The requested ids that are embeddings providers.
        embeddings: Vec<String>,
        /// Providers that can form cells.
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

    /// Refuse a `--provider` list that names anything unable to form cells —
    /// **whether or not** other entries could. A list that is half right must
    /// not silently shrink the matrix: a CI job that believes it measured the
    /// dropped provider gets a green gate that measured nothing of it.
    ///
    /// Ordered most-actionable first: an unknown id is a typo with a one-line
    /// fix; a `fallback_only` or embeddings id is a category error with its
    /// own explanation. Returns `None` when every requested id forms cells (or
    /// the list is empty — no filter, nothing to check).
    pub fn check_provider_filter(
        providers: &[crate::config::Provider],
        requested: &[String],
    ) -> Option<EmptyRun> {
        if requested.is_empty() {
            return None;
        }
        let cell_capable: Vec<String> = providers
            .iter()
            .filter(|p| p.forms_cells())
            .map(|p| p.id.clone())
            .collect();

        let unknown: Vec<String> = requested
            .iter()
            .filter(|r| !providers.iter().any(|p| &&p.id == r))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Some(EmptyRun::NoProvidersSelected {
                requested: unknown,
                available: cell_capable,
            });
        }

        let fallback_only: Vec<String> = requested
            .iter()
            .filter(|r| providers.iter().any(|p| &&p.id == r && p.fallback_only))
            .cloned()
            .collect();
        if !fallback_only.is_empty() {
            return Some(EmptyRun::FallbackOnlyExcluded {
                requested: requested.to_vec(),
                fallback_only,
                available: cell_capable,
            });
        }

        let embeddings: Vec<String> = requested
            .iter()
            .filter(|r| {
                providers.iter().any(|p| {
                    &&p.id == r && matches!(p.kind, crate::config::ProviderKind::Embeddings { .. })
                })
            })
            .cloned()
            .collect();
        if !embeddings.is_empty() {
            return Some(EmptyRun::EmbeddingsSelected {
                requested: requested.to_vec(),
                embeddings,
                available: cell_capable,
            });
        }
        None
    }

    /// Diagnose a selection that produced no providers at all, after
    /// [`Self::check_provider_filter`] has already vetted any `--provider`
    /// list. Lives here rather than in the runner so the diagnosis stays next
    /// to the messages it produces.
    ///
    /// With the filter vetted, reaching this with providers empty means the
    /// suite itself cannot form cells — every system under test is
    /// `fallback_only` (its own diagnosis), or the suite has only grader
    /// helpers (the generic one).
    pub fn no_providers(providers: &[crate::config::Provider], requested: &[String]) -> EmptyRun {
        let fallback_only_ids: Vec<String> = providers
            .iter()
            .filter(|p| !matches!(p.kind, crate::config::ProviderKind::Embeddings { .. }))
            .filter(|p| p.fallback_only)
            .map(|p| p.id.clone())
            .collect();
        let cell_capable = providers.iter().any(|p| p.forms_cells());

        if requested.is_empty() && !fallback_only_ids.is_empty() && !cell_capable {
            return EmptyRun::FallbackOnlyExcluded {
                requested: Vec::new(),
                fallback_only: fallback_only_ids,
                available: Vec::new(),
            };
        }
        EmptyRun::NoProvidersSelected {
            requested: requested.to_vec(),
            available: providers
                .iter()
                .filter(|p| p.forms_cells())
                .map(|p| p.id.clone())
                .collect(),
        }
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
            } => {
                write!(
                    f,
                    "--provider {requested:?} matches none of the suite's providers"
                )?;
                if available.is_empty() {
                    write!(f, ".")
                } else {
                    // Only the runnable ids: recommending a `fallback_only` or
                    // embeddings provider here would send the user straight
                    // into the next usage error.
                    write!(f, ". Run one of: {}", available.join(", "))
                }
            }
            EmptyRun::NoPromptsSelected {
                requested,
                available,
            } => write!(
                f,
                "--prompt {requested:?} matches none of the suite's prompts: {}",
                available.join(", ")
            ),
            EmptyRun::FallbackOnlyExcluded {
                requested,
                fallback_only,
                available,
            } => {
                if requested.is_empty() {
                    write!(
                        f,
                        "every provider is `fallback_only`, so the matrix is empty; at \
                         least one provider must be able to run its own cells. Remove \
                         `fallback_only: true` from one of: {}",
                        fallback_only.join(", ")
                    )
                } else {
                    write!(
                        f,
                        "--provider names `fallback_only` provider(s) `{}`, which never form \
                         cells",
                        fallback_only.join("`, `")
                    )?;
                    if available.is_empty() {
                        write!(
                            f,
                            ". Remove `fallback_only: true` from `{}` to run it directly.",
                            fallback_only.join("`, `")
                        )
                    } else {
                        write!(
                            f,
                            ". Run one of: {} — or remove `fallback_only: true` from `{}`.",
                            available.join(", "),
                            fallback_only.join("`, `")
                        )
                    }
                }
            }
            EmptyRun::EmbeddingsSelected {
                requested: _,
                embeddings,
                available,
            } => {
                write!(
                    f,
                    "--provider names embeddings provider(s) `{}` — grader helpers that never \
                     form cells",
                    embeddings.join("`, `")
                )?;
                if available.is_empty() {
                    write!(f, ".")
                } else {
                    write!(f, ". Run one of: {}", available.join(", "))
                }
            }
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

    /// `--provider reserve` where `reserve` is `fallback_only` is not a typo,
    /// and telling the user it "matches none of the suite's providers" would
    /// send them hunting for a spelling error that does not exist.
    #[test]
    fn naming_a_fallback_only_provider_offers_both_ways_out() {
        let msg = EmptyRun::FallbackOnlyExcluded {
            requested: vec!["reserve".into()],
            fallback_only: vec!["reserve".into()],
            available: vec!["primary".into()],
        }
        .to_string();
        assert!(msg.contains("fallback_only"));
        assert!(msg.contains("never form cells"));
        assert!(msg.contains("primary"), "names the runnable alternative");
        assert!(msg.contains("`reserve`"), "names the flag to remove");
    }

    #[test]
    fn an_all_fallback_only_suite_says_so_without_blaming_a_filter() {
        let msg = EmptyRun::FallbackOnlyExcluded {
            requested: Vec::new(),
            fallback_only: vec!["a".into(), "b".into()],
            available: Vec::new(),
        }
        .to_string();
        assert!(msg.contains("every provider is `fallback_only`"));
        assert!(!msg.contains("--provider"), "no filter was involved");
        assert!(msg.contains("a, b"));
    }

    fn mixed_suite() -> Vec<crate::config::Provider> {
        serde_yaml_ng::from_str(
            "- id: primary\n  type: exec\n  command: [x]\n\
             - id: reserve\n  fallback_only: true\n  type: exec\n  command: [x]\n\
             - id: embedder\n  type: embeddings\n  model: m\n",
        )
        .unwrap()
    }

    /// A typo'd id alongside a `fallback_only` one is diagnosed as the typo:
    /// that is the mistake with the one-line fix.
    #[test]
    fn a_typo_outranks_the_fallback_only_diagnosis() {
        let providers = mixed_suite();
        match EmptyRun::check_provider_filter(&providers, &["reserve".into(), "gohst".into()]) {
            Some(EmptyRun::NoProvidersSelected {
                requested,
                available,
            }) => {
                assert_eq!(
                    requested,
                    vec!["gohst".to_string()],
                    "only the typo is quoted"
                );
                assert_eq!(
                    available,
                    vec!["primary".to_string()],
                    "only runnable ids offered"
                );
            }
            other => panic!("expected the typo diagnosis, got {other:?}"),
        }
        assert!(matches!(
            EmptyRun::check_provider_filter(&providers, &["reserve".into()]),
            Some(EmptyRun::FallbackOnlyExcluded { .. })
        ));
    }

    /// The half-right list is the dangerous one: a runnable id beside a
    /// `fallback_only` id must refuse, not silently shrink the matrix.
    #[test]
    fn a_mixed_list_with_a_fallback_only_id_is_refused() {
        let providers = mixed_suite();
        match EmptyRun::check_provider_filter(&providers, &["primary".into(), "reserve".into()]) {
            Some(EmptyRun::FallbackOnlyExcluded { fallback_only, .. }) => {
                assert_eq!(fallback_only, vec!["reserve".to_string()]);
            }
            other => panic!("expected FallbackOnlyExcluded, got {other:?}"),
        }
    }

    /// An embeddings id is a category error of its own, not a `fallback_only`
    /// one — the message must not tell the user to remove a flag the provider
    /// does not carry.
    #[test]
    fn an_embeddings_id_gets_its_own_diagnosis() {
        let providers = mixed_suite();
        match EmptyRun::check_provider_filter(&providers, &["primary".into(), "embedder".into()]) {
            Some(diag @ EmptyRun::EmbeddingsSelected { .. }) => {
                let msg = diag.to_string();
                assert!(msg.contains("embeddings"));
                assert!(!msg.contains("fallback_only"));
                assert!(msg.contains("primary"));
            }
            other => panic!("expected EmbeddingsSelected, got {other:?}"),
        }
    }

    /// The all-clear: a list of runnable ids has nothing to refuse.
    #[test]
    fn a_fully_runnable_list_passes_the_filter_check() {
        let providers = mixed_suite();
        assert!(EmptyRun::check_provider_filter(&providers, &["primary".into()]).is_none());
        assert!(EmptyRun::check_provider_filter(&providers, &[]).is_none());
    }
}
