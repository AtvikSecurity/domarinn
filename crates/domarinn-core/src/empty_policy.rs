//! What an empty output means for this run: whether it is worth storing, and
//! whether prose counts as a refusal.
//!
//! Both questions have to be answered identically in two places — the cache
//! write gate in [`crate::runner::runner_cache`], and the case assembly in
//! `runner_cell` — or a response is graded as one thing and stored as another.
//! One type, built once per run, is what keeps those two answers the same.

use crate::config::{StoreEmptyOutputs, Suite};
use crate::empty::EmptyReason;
use crate::provider::{Provider, ProviderResponse};
use crate::types::Output;

/// The empty reasons [`StoreEmptyOutputs::Reproducible`] keeps.
///
/// The test is not "is this bad news" — a refusal is perfectly real news. It is
/// "will the same request produce this again". `truncated` will, until
/// `max_tokens` changes; `tool_use_only` will, for a model that reaches for the
/// same tool; `output_expr_empty` will, because a selector that matches nothing
/// matches nothing every time. A gateway that returns an empty body on one call
/// in five will not, and that is the whole bug.
const REPRODUCIBLE: &[&str] = &[
    EmptyReason::TRUNCATED,
    EmptyReason::TOOL_USE_ONLY,
    EmptyReason::OUTPUT_EXPR_EMPTY,
];

/// Regexes that promote a non-empty output to a refusal.
///
/// Empty by default. Compiled once per run rather than per case: a suite with
/// four patterns and two thousand cells would otherwise rebuild the same
/// automata eight thousand times.
#[derive(Debug, Default, Clone)]
pub struct RefusalMatcher {
    patterns: Vec<regex::Regex>,
}

impl RefusalMatcher {
    /// Compile the suite's patterns.
    ///
    /// An invalid regex is reported, not fatal: `validate` already refuses the
    /// suite for it, and a run that dies at cell one over a diagnostic pattern
    /// is worse than one that runs without it. The `Err` names the offender so
    /// the log says which.
    pub fn compile(patterns: &[String]) -> Result<Self, (String, regex::Error)> {
        let mut compiled = Vec::with_capacity(patterns.len());
        for p in patterns {
            match regex::Regex::new(p) {
                Ok(re) => compiled.push(re),
                Err(e) => return Err((p.clone(), e)),
            }
        }
        Ok(RefusalMatcher { patterns: compiled })
    }

    /// True when this output reads as a refusal.
    ///
    /// Only ever consulted for output the provider did *not* already classify,
    /// so a vendor's own diagnosis always outranks a pattern guess.
    fn matches(&self, output: &Output) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        // A JSON output is matched against its rendered form: a refusal that
        // arrived as `{"error": "I can't help with that"}` is still a refusal,
        // and asking a suite to write two patterns for the two shapes would be
        // asking it to know which one it got.
        let rendered;
        let text = match output {
            Output::Text(s) => s.as_str(),
            Output::Json(v) => {
                rendered = v.to_string();
                rendered.as_str()
            }
        };
        self.patterns.iter().any(|re| re.is_match(text))
    }
}

/// One run's answer to "is this empty, and do we keep it".
#[derive(Debug, Default, Clone)]
pub struct EmptyPolicy {
    store: StoreEmptyOutputs,
    refusal: RefusalMatcher,
}

impl EmptyPolicy {
    /// Read the policy off a suite, defaulting where it says nothing.
    ///
    /// Returns the compile error rather than swallowing it so the caller can
    /// warn once, at run start, instead of once per case.
    pub fn from_suite(suite: &Suite) -> Result<Self, (String, regex::Error)> {
        let store = suite
            .cache
            .as_ref()
            .and_then(|c| c.store_empty_outputs)
            .unwrap_or_default();
        let patterns = suite
            .runner
            .as_ref()
            .map(|r| r.refusal_patterns.as_slice())
            .unwrap_or_default();
        Ok(EmptyPolicy {
            store,
            refusal: RefusalMatcher::compile(patterns)?,
        })
    }

    /// Build one directly, for a caller that is not reading a suite.
    pub fn new(store: StoreEmptyOutputs, refusal: RefusalMatcher) -> Self {
        EmptyPolicy { store, refusal }
    }

    /// Override the store policy, for the CLI flag and its environment variable.
    pub fn with_store(mut self, store: StoreEmptyOutputs) -> Self {
        self.store = store;
        self
    }

    /// The reason this response has nothing gradeable in it, if it has nothing.
    ///
    /// The provider's own diagnosis first — it saw the finish reason and the
    /// block types, which are gone by the time anything else looks — then the
    /// suite's refusal patterns for the case the provider cannot see: prose.
    ///
    /// This is the single definition. Both the cache write gate and case
    /// assembly call it, because a response stored as gradeable and graded as
    /// empty (or the reverse) is a bug that only shows up on the second run.
    pub fn effective_reason(
        &self,
        provider: &dyn Provider,
        response: &ProviderResponse,
    ) -> Option<EmptyReason> {
        provider.classify_empty(response).or_else(|| {
            self.refusal
                .matches(&response.output)
                .then(|| EmptyReason::new(EmptyReason::REFUSAL))
        })
    }

    /// Whether a response carrying this reason is worth writing to the cache.
    ///
    /// `None` — a real answer — is always stored. That is the overwhelming
    /// majority of calls, and nothing about this feature should change them.
    pub fn should_store(&self, reason: Option<&EmptyReason>) -> bool {
        let Some(reason) = reason else { return true };
        match self.store {
            StoreEmptyOutputs::Always => true,
            StoreEmptyOutputs::Never => false,
            StoreEmptyOutputs::Reproducible => REPRODUCIBLE.contains(&reason.as_str()),
        }
    }

    /// Whether an entry *already in the store* carries a reason this suite would
    /// no longer write.
    ///
    /// Reads the recorded reason rather than re-classifying: by the time an
    /// entry is read back, the response object it was built from is long gone.
    /// Used for the replay warning, and to stop a legacy entry being re-filed
    /// under today's key — a re-file is a brand-new immutable entry, which is
    /// the exact thing this prevents, arriving through a side door.
    pub fn stored_entry_is_declined(&self, entry: &crate::cache::CacheEntry) -> bool {
        !self.should_store(entry.empty_reason.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reason(s: &str) -> EmptyReason {
        EmptyReason::new(s)
    }

    #[test]
    fn a_real_answer_is_always_stored_under_every_policy() {
        for store in [
            StoreEmptyOutputs::Never,
            StoreEmptyOutputs::Reproducible,
            StoreEmptyOutputs::Always,
        ] {
            let p = EmptyPolicy::new(store, RefusalMatcher::default());
            assert!(p.should_store(None), "{store:?} refused a real answer");
        }
    }

    #[test]
    fn reproducible_keeps_truncated_and_drops_a_refusal() {
        let p = EmptyPolicy::default();
        assert!(p.should_store(Some(&reason(EmptyReason::TRUNCATED))));
        assert!(p.should_store(Some(&reason(EmptyReason::TOOL_USE_ONLY))));
        assert!(p.should_store(Some(&reason(EmptyReason::OUTPUT_EXPR_EMPTY))));
        assert!(!p.should_store(Some(&reason(EmptyReason::REFUSAL))));
        assert!(!p.should_store(Some(&reason(EmptyReason::CONTENT_FILTER))));
    }

    /// The reported symptom in issue #79: a flaky gateway returns an empty body
    /// with an ordinary finish reason, which classifies as `no_content_blocks`
    /// or `blank` — never as `refusal`. A denylist naming only `refusal` would
    /// have left this cached, which is the bug.
    #[test]
    fn reproducible_drops_the_empty_body_a_flaky_gateway_produces() {
        let p = EmptyPolicy::default();
        assert!(!p.should_store(Some(&reason(EmptyReason::NO_CONTENT_BLOCKS))));
        assert!(!p.should_store(Some(&reason(EmptyReason::EMPTY_BODY))));
        assert!(!p.should_store(Some(&reason(EmptyReason::BLANK))));
    }

    /// `EmptyReason` is open, so the policy has to answer for reasons that did
    /// not exist when it was written. Not storing is the safe answer: it costs
    /// a re-draw, where the other way costs a frozen mystery.
    #[test]
    fn a_reason_this_build_has_never_heard_of_is_not_stored() {
        let p = EmptyPolicy::default();
        assert!(!p.should_store(Some(&reason("some_future_vendor_reason"))));
    }

    #[test]
    fn always_restores_the_pre_0_10_behaviour() {
        let p = EmptyPolicy::new(StoreEmptyOutputs::Always, RefusalMatcher::default());
        assert!(p.should_store(Some(&reason(EmptyReason::REFUSAL))));
        assert!(p.should_store(Some(&reason("some_future_vendor_reason"))));
    }

    #[test]
    fn never_drops_even_a_reproducible_reason() {
        let p = EmptyPolicy::new(StoreEmptyOutputs::Never, RefusalMatcher::default());
        assert!(!p.should_store(Some(&reason(EmptyReason::TRUNCATED))));
        assert!(p.should_store(None));
    }

    #[test]
    fn a_refusal_pattern_matches_prose_and_an_empty_list_matches_nothing() {
        let m = RefusalMatcher::compile(&["(?i)^i (can't|cannot) help with".to_string()]).unwrap();
        assert!(m.matches(&Output::Text("I can't help with that request.".to_string())));
        assert!(!m.matches(&Output::Text("Paris is the capital.".to_string())));
        assert!(!RefusalMatcher::default().matches(&Output::Text("I can't help with".to_string())));
    }

    #[test]
    fn an_invalid_pattern_names_itself() {
        let err = RefusalMatcher::compile(&["valid".to_string(), "((unclosed".to_string()]);
        let (offender, _) = err.expect_err("an unclosed group must not compile");
        assert_eq!(offender, "((unclosed");
    }
}
