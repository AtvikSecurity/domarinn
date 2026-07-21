//! Selecting which tests, providers, and prompts a run includes.
//!
//! Semantics (from the CLI contract): within a kind, repeated `--tag` / `--filter`
//! flags are OR'd; across kinds they are AND'd. `--filter` globs test ids.

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::config::TestCase;

/// A compiled filter built from CLI options.
pub struct Filter {
    tags: Vec<String>,
    id_globs: Option<GlobSet>,
    providers: Vec<String>,
    prompts: Vec<String>,
}

/// Raw filter options (as passed on the CLI).
#[derive(Debug, Clone, Default)]
pub struct FilterOpts {
    pub tags: Vec<String>,
    pub filters: Vec<String>,
    pub providers: Vec<String>,
    pub prompts: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("bad --filter glob '{pattern}': {source}")]
pub struct FilterError {
    pattern: String,
    #[source]
    source: globset::Error,
}

impl Filter {
    pub fn build(opts: &FilterOpts) -> Result<Filter, FilterError> {
        let id_globs = if opts.filters.is_empty() {
            None
        } else {
            let mut builder = GlobSetBuilder::new();
            for pattern in &opts.filters {
                let glob = Glob::new(pattern).map_err(|source| FilterError {
                    pattern: pattern.clone(),
                    source,
                })?;
                builder.add(glob);
            }
            Some(builder.build().map_err(|source| FilterError {
                pattern: opts.filters.join(","),
                source,
            })?)
        };
        Ok(Filter {
            tags: opts.tags.clone(),
            id_globs,
            providers: opts.providers.clone(),
            prompts: opts.prompts.clone(),
        })
    }

    /// True when a test passes the tag and id filters.
    pub fn matches_test(&self, tc: &TestCase) -> bool {
        let tag_ok = self.tags.is_empty() || self.tags.iter().any(|t| tc.tags.contains(t));
        let id_ok = match &self.id_globs {
            None => true,
            Some(globs) => tc
                .id
                .as_deref()
                .map(|id| globs.is_match(id))
                .unwrap_or(false),
        };
        tag_ok && id_ok
    }

    /// True when a provider id is included, honoring both the `--provider`
    /// allowlist and the test's own `only_providers` / `skip_providers`.
    pub fn provider_included(&self, provider_id: &str, tc: &TestCase) -> bool {
        if !self.providers.is_empty() && !self.providers.iter().any(|p| p == provider_id) {
            return false;
        }
        if !tc.only_providers.is_empty() && !tc.only_providers.iter().any(|p| p == provider_id) {
            return false;
        }
        if tc.skip_providers.iter().any(|p| p == provider_id) {
            return false;
        }
        true
    }

    /// True when a prompt id is included by the `--prompt` allowlist.
    pub fn prompt_included(&self, prompt_id: &str) -> bool {
        self.prompts.is_empty() || self.prompts.iter().any(|p| p == prompt_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_with(id: &str, tags: &[&str]) -> TestCase {
        TestCase {
            id: Some(id.to_string()),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn no_filters_matches_everything() {
        let f = Filter::build(&FilterOpts::default()).unwrap();
        assert!(f.matches_test(&test_with("a", &[])));
    }

    #[test]
    fn tags_or_within_kind() {
        let f = Filter::build(&FilterOpts {
            tags: vec!["x".into(), "y".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(f.matches_test(&test_with("a", &["y"])));
        assert!(!f.matches_test(&test_with("a", &["z"])));
    }

    #[test]
    fn tags_and_filters_are_anded_across_kinds() {
        let f = Filter::build(&FilterOpts {
            tags: vec!["scope".into()],
            filters: vec!["expert/*".into()],
            ..Default::default()
        })
        .unwrap();
        assert!(f.matches_test(&test_with("expert/sqli", &["scope"])));
        // right id, wrong tag
        assert!(!f.matches_test(&test_with("expert/sqli", &["other"])));
        // right tag, wrong id
        assert!(!f.matches_test(&test_with("worker/x", &["scope"])));
    }

    #[test]
    fn provider_allowlist_and_skip() {
        let f = Filter::build(&FilterOpts {
            providers: vec!["claude".into()],
            ..Default::default()
        })
        .unwrap();
        let tc = test_with("a", &[]);
        assert!(f.provider_included("claude", &tc));
        assert!(!f.provider_included("gpt", &tc));
    }

    #[test]
    fn test_only_and_skip_providers() {
        let f = Filter::build(&FilterOpts::default()).unwrap();
        let mut tc = test_with("a", &[]);
        tc.skip_providers = vec!["gpt".into()];
        assert!(!f.provider_included("gpt", &tc));
        assert!(f.provider_included("claude", &tc));

        tc.skip_providers.clear();
        tc.only_providers = vec!["claude".into()];
        assert!(f.provider_included("claude", &tc));
        assert!(!f.provider_included("gpt", &tc));
    }
}
