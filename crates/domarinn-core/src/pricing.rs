//! What a provider call cost.
//!
//! domarinn ships a `cost` assertion and a run-level cost figure, and until now
//! neither had a number behind it for the built-in providers: `cost_usd` was
//! hardcoded `None`, so `cost: {max: 0.01}` was a guaranteed pass. This module
//! is the rate table and the arithmetic that make both mean something.
//!
//! # Money is an integer here
//!
//! [`MicroUsd`] is USD millionths. Costs are summed across thousands of cases,
//! and a float accumulator makes the total depend on the order it was added in
//! — the same run, re-summed, disagreeing with itself. The *wire* stays an f64
//! `cost_usd`: every value we emit is a whole number of micro-dollars over 1e6,
//! which round-trips exactly, so the precision was only ever at risk in the
//! summation this fixes.
//!
//! # An unknown model costs nothing, not a guess
//!
//! Resolution returns `None` for an id the table does not know, and callers
//! leave `cost_usd` absent. That keeps the `cost` assertion's "not reported"
//! branch meaningful. A fallback rate would convert "we don't know" into a
//! number that silently passes or fails a budget gate, which is worse than the
//! loud no-op it replaced — and there is no defensible "average model price" to
//! fall back *to*. The escape hatch is a per-provider `pricing:` block.
//!
//! Cost estimation is instrumentation, never a gate: nothing in here fails a
//! run.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::types::TokenUsage;

/// The rate table, authored in `pricing.yaml` next to this file.
///
/// Data rather than a `const` table for two reasons: `serde_yaml_ng` is already
/// a dependency so it costs no new crate, and a `.yaml` is outside the
/// per-file line ratchet's extension list — so the table can grow with the
/// model catalogue without dragging this module toward the cap or mixing churny
/// price data into reviewed logic.
const TABLE_YAML: &str = include_str!("pricing.yaml");

/// USD in millionths. The accumulation unit; never the wire type.
///
/// `u64` rather than `i64`: a cost is never negative, and making that
/// unrepresentable is cheaper than checking for it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct MicroUsd(pub u64);

impl MicroUsd {
    pub const ZERO: MicroUsd = MicroUsd(0);

    /// Saturating, because these are sums over provider-reported counts: an
    /// absurd report should skew a number, not panic a release build by
    /// wrapping.
    pub fn saturating_add(self, other: MicroUsd) -> MicroUsd {
        MicroUsd(self.0.saturating_add(other.0))
    }

    pub fn to_usd(self) -> f64 {
        self.0 as f64 / 1_000_000.0
    }

    /// Convert a dollar figure a provider reported for itself. Negative and
    /// non-finite inputs clamp to zero rather than wrapping into a huge `u64`.
    pub fn from_usd(usd: f64) -> MicroUsd {
        if !usd.is_finite() || usd <= 0.0 {
            return MicroUsd::ZERO;
        }
        MicroUsd((usd * 1_000_000.0).round() as u64)
    }
}

impl std::iter::Sum for MicroUsd {
    fn sum<I: Iterator<Item = MicroUsd>>(iter: I) -> MicroUsd {
        iter.fold(MicroUsd::ZERO, MicroUsd::saturating_add)
    }
}

/// One model's rates, in USD per million tokens.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ModelRate {
    pub input: f64,
    pub output: f64,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write_5m: Option<f64>,
    #[serde(default)]
    pub cache_write_1h: Option<f64>,
    /// The date a human last checked this row against the vendor's published
    /// price sheet. See `pricing.yaml` for why this is the only verification
    /// marker.
    pub as_of: String,
}

/// One embedding model's rate, in USD per million input tokens.
///
/// Its own type rather than a [`ModelRate`] with zeros: an embedding call bills
/// exactly one component, so the other fields would not be "unknown" or "free",
/// they would be meaningless. Keeping them unrepresentable is also what lets
/// the table's sanity test keep insisting a chat row has a positive output rate.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EmbeddingRate {
    pub input: f64,
    /// The date a human last checked this row. Same rule as [`ModelRate`].
    pub as_of: String,
}

#[derive(Debug, Deserialize)]
struct RateTable {
    #[serde(default)]
    exact: BTreeMap<String, ModelRate>,
    #[serde(default)]
    families: BTreeMap<String, ModelRate>,
    #[serde(default)]
    embeddings: BTreeMap<String, EmbeddingRate>,
}

static TABLE: LazyLock<RateTable> = LazyLock::new(|| {
    serde_yaml_ng::from_str(TABLE_YAML).expect("pricing.yaml is well-formed (pinned by a test)")
});

/// Strip the vendor-neutral decorations an id can pick up in transit.
///
/// Bedrock prefixes the id with a (sometimes regional) namespace and Vertex
/// suffixes it with `@version`. Normalizing means those ids *can* resolve to a
/// first-party row — which is only correct when the operator has confirmed the
/// rates match, hence the warning in `pricing.yaml` and the `pricing:` override.
fn normalize(id: &str) -> &str {
    let id = id.trim();
    let id = [
        "us.anthropic.",
        "eu.anthropic.",
        "apac.anthropic.",
        "anthropic.",
    ]
    .iter()
    .find_map(|p| id.strip_prefix(p))
    .unwrap_or(id);
    match id.split_once('@') {
        Some((base, _version)) => base,
        None => id,
    }
}

/// Drop a trailing snapshot-date suffix, if present.
///
/// A dated snapshot bills at its family's rate, so `claude-haiku-4-5-20251001`
/// should not need its own row. The two vendors write the date differently and
/// **both** shapes have to be handled: Anthropic uses a compact
/// `-YYYYMMDD`, OpenAI a hyphenated `-YYYY-MM-DD`. Recognizing only the first is
/// not a cosmetic gap — it leaves every pinned OpenAI snapshot (`gpt-4o-2024-11-20`,
/// the id the vendor's own docs recommend) with no rate at all, so `cost_usd`
/// stays absent and a `cost: {max: …}` budget takes its "not reported; budget not
/// enforced" pass branch. That is precisely the silence this module exists to end.
fn strip_snapshot_date(id: &str) -> Option<&str> {
    let (base, tail) = id.rsplit_once('-')?;
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());

    // Anthropic: `-20251001`.
    if digits(tail, 8) {
        return Some(base);
    }
    // OpenAI: `-2024-11-20`. Peeled one component at a time so a model id that
    // merely ends in digits (`gpt-4o-mini-2`) is not mistaken for a date.
    if digits(tail, 2) {
        let (base, month) = base.rsplit_once('-')?;
        let (base, year) = base.rsplit_once('-')?;
        if digits(month, 2) && digits(year, 4) {
            return Some(base);
        }
    }
    None
}

/// The built-in rate for a model id, or `None` if the table does not know it.
pub fn built_in_rate(model_id: &str) -> Option<&'static ModelRate> {
    let id = normalize(model_id);

    if let Some(rate) = TABLE.exact.get(id) {
        return Some(rate);
    }
    if let Some(rate) = strip_snapshot_date(id).and_then(|base| TABLE.exact.get(base)) {
        return Some(rate);
    }
    // Longest prefix wins, so a narrow family entry always beats a broad one
    // regardless of the order they appear in the file.
    TABLE
        .families
        .iter()
        .filter(|(prefix, _)| id.starts_with(prefix.as_str()))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, rate)| rate)
}

/// The built-in rate for an embedding model id, or `None` if the table does not
/// know it. Exact plus the snapshot-date strip; no family fallback.
pub fn built_in_embedding_rate(model_id: &str) -> Option<&'static EmbeddingRate> {
    let id = normalize(model_id);
    TABLE
        .embeddings
        .get(id)
        .or_else(|| strip_snapshot_date(id).and_then(|base| TABLE.embeddings.get(base)))
}

/// Cost the tokens in `usage` at `rate`, or `None` if any *reported* component
/// has no rate.
///
/// The all-or-nothing rule is the point: a partial total presented as a whole
/// one is a number that looks authoritative and is quietly low. If a provider
/// reports cache-write tokens and the rate sheet has no cache-write price, the
/// honest answer is that we cannot price this call.
pub fn cost_of(usage: &TokenUsage, rate: &ModelRate) -> Option<MicroUsd> {
    let mut total = component(usage.input_tokens, Some(rate.input))?;
    total = total.saturating_add(component(usage.output_tokens, Some(rate.output))?);

    if let Some(read) = usage.cache_read_tokens.filter(|t| *t > 0) {
        total = total.saturating_add(component(read, rate.cache_read)?);
    }

    if let Some(write) = usage.cache_write_tokens.filter(|t| *t > 0) {
        // The long-TTL portion is a subset of the total written, so the
        // remainder bills at the short-TTL rate. Saturating: a provider
        // reporting a 1h figure larger than the total should not underflow.
        let long = usage.cache_write_1h_tokens.unwrap_or(0).min(write);
        let short = write - long;
        if short > 0 {
            total = total.saturating_add(component(short, rate.cache_write_5m)?);
        }
        if long > 0 {
            total = total.saturating_add(component(long, rate.cache_write_1h)?);
        }
    }

    Some(total)
}

/// `tokens` at `usd_per_mtok`, rounded half-up to a whole micro-dollar.
///
/// Each component rounds independently and then sums, so the result does not
/// depend on the order components were added — the property the integer unit
/// exists for. `u128` intermediate so a large count times a large rate cannot
/// overflow before the divide.
fn component(tokens: u64, usd_per_mtok: Option<f64>) -> Option<MicroUsd> {
    let rate = usd_per_mtok?;
    if tokens == 0 {
        return Some(MicroUsd::ZERO);
    }
    // usd_per_mtok -> micro-USD per million tokens, as an integer.
    let micro_per_mtok = (rate * 1_000_000.0).round().max(0.0) as u128;
    let micros = (tokens as u128 * micro_per_mtok).div_ceil(1_000_000);
    Some(MicroUsd(micros.min(u64::MAX as u128) as u64))
}

/// The effective rate for a provider: the built-in row for `model_id`, with any
/// configured override merged field-wise over it.
///
/// Called once when a provider is constructed, not once per call. That is what
/// makes the unknown-model warning fire exactly once per run per provider with
/// no global mutable state to leak across a shared test binary — and it keeps
/// `domarinn validate` and `list` silent, since neither builds a provider.
///
/// `None` means the cost of this provider's calls is unknown, and callers must
/// leave `cost_usd` absent rather than substitute anything.
pub fn resolve_rate(
    provider_id: &str,
    model_id: &str,
    override_cfg: Option<&crate::config::PricingCfg>,
) -> Option<ModelRate> {
    let built_in = built_in_rate(model_id);

    let merged = match (built_in, override_cfg) {
        (None, None) => {
            tracing::warn!(
                provider = provider_id,
                model = model_id,
                "no built-in rate for this model, so cost will not be reported; \
                 set `pricing:` on the provider to price it"
            );
            return None;
        }
        (Some(base), None) => base.clone(),
        (base, Some(cfg)) => {
            let base = base.cloned();
            let pick = |over: Option<f64>, from_base: Option<f64>| over.or(from_base);
            let input = pick(cfg.input_per_mtok, base.as_ref().map(|b| b.input));
            let output = pick(cfg.output_per_mtok, base.as_ref().map(|b| b.output));
            let (Some(input), Some(output)) = (input, output) else {
                tracing::warn!(
                    provider = provider_id,
                    model = model_id,
                    "`pricing:` is set but leaves input or output unpriced and there is \
                     no built-in rate to fall back on, so cost will not be reported"
                );
                return None;
            };
            ModelRate {
                input,
                output,
                cache_read: pick(
                    cfg.cache_read_per_mtok,
                    base.as_ref().and_then(|b| b.cache_read),
                ),
                cache_write_5m: pick(
                    cfg.cache_write_per_mtok,
                    base.as_ref().and_then(|b| b.cache_write_5m),
                ),
                cache_write_1h: pick(
                    cfg.cache_write_1h_per_mtok,
                    base.as_ref().and_then(|b| b.cache_write_1h),
                ),
                // A configured rate is verified by whoever configured it.
                as_of: base
                    .as_ref()
                    .map(|b| b.as_of.clone())
                    .unwrap_or_else(|| "configured".to_string()),
            }
        }
    };
    Some(merged)
}

/// The effective rate for an embeddings provider.
///
/// Written out rather than routed through [`resolve_rate`]'s merge because the
/// two disagree on what "unpriced" means: a chat model with no output rate
/// cannot be priced, whereas an embedding model has no output tokens to price,
/// so `input` alone is a complete answer. `cache_*` stay absent for the same
/// reason — the embeddings endpoint reports no cache counters, so a rate for
/// them would price nothing.
pub fn resolve_embedding_rate(
    provider_id: &str,
    model_id: &str,
    override_cfg: Option<&crate::config::PricingCfg>,
) -> Option<ModelRate> {
    let built_in = built_in_embedding_rate(model_id);
    let input = override_cfg
        .and_then(|c| c.input_per_mtok)
        .or_else(|| built_in.map(|r| r.input))?;
    if built_in.is_none() && override_cfg.and_then(|c| c.input_per_mtok).is_none() {
        tracing::warn!(
            provider = provider_id,
            model = model_id,
            "no built-in rate for this embedding model, so similarity grading \
             will not be costed; set `pricing:` on the provider to price it"
        );
    }
    Some(ModelRate {
        input,
        // Not a rate of zero dollars — zero tokens. An embedding response
        // reports none, so this multiplies out to nothing either way.
        output: 0.0,
        cache_read: None,
        cache_write_5m: None,
        cache_write_1h: None,
        as_of: built_in
            .map(|r| r.as_of.clone())
            .unwrap_or_else(|| "configured".to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            output_tokens: output,
            ..Default::default()
        }
    }

    /// The table is `include_str!`'d, so a malformed edit would otherwise only
    /// surface as a panic on the first priced call — in someone's run, not in
    /// CI.
    #[test]
    fn the_table_parses_and_every_row_is_sane() {
        let rows: Vec<(&String, &ModelRate)> =
            TABLE.exact.iter().chain(TABLE.families.iter()).collect();
        assert!(!rows.is_empty(), "the shipped table must not be empty");
        for (id, rate) in rows {
            assert!(rate.input > 0.0, "{id}: input rate must be positive");
            assert!(rate.output > 0.0, "{id}: output rate must be positive");
            for (name, value) in [
                ("cache_read", rate.cache_read),
                ("cache_write_5m", rate.cache_write_5m),
                ("cache_write_1h", rate.cache_write_1h),
            ] {
                if let Some(v) = value {
                    assert!(v >= 0.0, "{id}: {name} must not be negative");
                }
            }
            // The verification marker. A row without a parseable date has not
            // been checked by anyone, which is the one thing the table promises.
            assert!(
                rate.as_of.len() == 10 && rate.as_of.split('-').count() == 3,
                "{id}: as_of must be an ISO date, got {:?}",
                rate.as_of
            );
        }

        assert!(
            !TABLE.embeddings.is_empty(),
            "embedding rows must not vanish"
        );
        for (id, rate) in &TABLE.embeddings {
            assert!(rate.input > 0.0, "{id}: input rate must be positive");
            assert!(
                rate.as_of.len() == 10 && rate.as_of.split('-').count() == 3,
                "{id}: as_of must be an ISO date, got {:?}",
                rate.as_of
            );
        }
    }

    #[test]
    fn an_embedding_id_resolves_and_prices_only_its_input() {
        let rate = resolve_embedding_rate("e", "text-embedding-3-small", None).expect("known id");
        assert_eq!(rate.output, 0.0);
        // 1M input at $0.02, and no output tokens to bill for.
        assert_eq!(
            cost_of(&usage(1_000_000, 0), &rate).unwrap(),
            MicroUsd(20_000)
        );
    }

    /// The two tables are separate namespaces on purpose: an embedding id must
    /// not pick up a chat family's rate, and a chat id must not silently resolve
    /// to an embedding row that prices its output at nothing.
    #[test]
    fn the_chat_and_embedding_tables_do_not_bleed_into_each_other() {
        assert!(built_in_rate("text-embedding-3-small").is_none());
        assert!(built_in_embedding_rate("claude-haiku-4-5").is_none());
    }

    /// An unknown embedding model behaves like an unknown chat model: nothing is
    /// reported, rather than a plausible-looking guess.
    #[test]
    fn an_unknown_embedding_id_prices_nothing_without_an_override() {
        assert!(resolve_embedding_rate("e", "some-embedder-from-2030", None).is_none());
        let cfg = crate::config::PricingCfg {
            input_per_mtok: Some(0.05),
            ..Default::default()
        };
        let rate = resolve_embedding_rate("e", "some-embedder-from-2030", Some(&cfg))
            .expect("an override prices it");
        assert_eq!(rate.input, 0.05);
        assert_eq!(rate.as_of, "configured");
    }

    #[test]
    fn an_exact_id_resolves() {
        assert!(built_in_rate("claude-haiku-4-5").is_some());
    }

    /// The coverage floor. Every mechanism above is tested in isolation, and
    /// all of them were green while the table itself was a model generation
    /// behind — so the ids a run in 2026 actually names resolved to nothing and
    /// every `cost:` budget quietly stopped enforcing. This is the test that
    /// fails when the table goes stale rather than when the resolver breaks:
    /// it names current ids, in each of the shapes a provider hands us.
    #[test]
    fn well_known_current_ids_resolve_to_a_built_in_rate() {
        for id in [
            // Anthropic, exact.
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-fable-5",
            "claude-haiku-4-5",
            // Dated snapshot -> date-strip.
            "claude-opus-5-20260315",
            // Suffixed alias -> longest-prefix `families` fallback.
            "claude-sonnet-5-latest",
            // Bedrock decoration -> prefix normalization.
            "us.anthropic.claude-opus-5",
            // OpenAI, exact.
            "gpt-5",
            "gpt-5-mini",
            "gpt-5-nano",
            "o3",
            "o4-mini",
            // OpenAI's hyphenated snapshot form -> date-strip.
            "gpt-4o-2024-08-06",
        ] {
            assert!(
                built_in_rate(id).is_some(),
                "no built-in rate resolves for {id}"
            );
        }
    }

    #[test]
    fn a_dated_snapshot_falls_back_to_its_family() {
        let dated = built_in_rate("claude-haiku-4-5-20251001").expect("snapshot resolves");
        let base = built_in_rate("claude-haiku-4-5").expect("base resolves");
        assert_eq!(dated, base);
    }

    #[test]
    fn bedrock_and_vertex_decorations_are_stripped() {
        let base = built_in_rate("claude-haiku-4-5");
        assert_eq!(built_in_rate("us.anthropic.claude-haiku-4-5"), base);
        assert_eq!(built_in_rate("anthropic.claude-haiku-4-5"), base);
        assert_eq!(built_in_rate("claude-haiku-4-5@20251001"), base);
    }

    /// The load-bearing negative: an unknown id must produce nothing, so the
    /// `cost` assertion keeps saying "not reported" instead of grading against
    /// an invented number.
    #[test]
    fn an_unknown_id_resolves_to_nothing() {
        assert!(built_in_rate("some-model-from-2030").is_none());
        assert!(built_in_rate("").is_none());
    }

    #[test]
    fn a_non_date_suffix_is_not_treated_as_a_snapshot() {
        assert_eq!(strip_snapshot_date("claude-haiku-4-5"), None);
        assert_eq!(strip_snapshot_date("model-abcdefgh"), None);
        assert_eq!(
            strip_snapshot_date("claude-haiku-4-5-20251001"),
            Some("claude-haiku-4-5")
        );
    }

    /// The two vendors date their snapshots differently, and recognizing only
    /// Anthropic's compact form left every pinned OpenAI id unpriced — so
    /// `cost_usd` was absent and a `cost:` budget silently stopped enforcing.
    #[test]
    fn openai_hyphenated_snapshot_dates_resolve_to_their_base_model() {
        assert_eq!(
            strip_snapshot_date("gpt-4o-2024-11-20"),
            Some("gpt-4o"),
            "the id OpenAI's own docs recommend pinning"
        );
        assert_eq!(built_in_rate("gpt-4o-2024-11-20"), built_in_rate("gpt-4o"));
        assert!(
            built_in_rate("gpt-4o-2024-11-20").is_some(),
            "sanity: the base model must actually be in the table"
        );
    }

    /// Peeled one component at a time, so an id that merely ends in digits is
    /// not mistaken for a date and silently priced as a different model.
    #[test]
    fn a_trailing_number_is_not_mistaken_for_a_hyphenated_date() {
        assert_eq!(strip_snapshot_date("gpt-4o-mini-2"), None);
        assert_eq!(strip_snapshot_date("some-model-11-20"), None);
        assert_eq!(strip_snapshot_date("some-model-24-11-20"), None);
    }

    #[test]
    fn longest_family_prefix_wins_regardless_of_order() {
        let broad = ModelRate {
            input: 1.0,
            output: 1.0,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            as_of: "2026-01-01".into(),
        };
        let narrow = ModelRate {
            input: 9.0,
            ..broad.clone()
        };
        let families: BTreeMap<String, ModelRate> = BTreeMap::from([
            ("fam-".to_string(), broad),
            ("fam-special-".to_string(), narrow),
        ]);
        let picked = families
            .iter()
            .filter(|(p, _)| "fam-special-v2".starts_with(p.as_str()))
            .max_by_key(|(p, _)| p.len())
            .map(|(_, r)| r)
            .unwrap();
        assert_eq!(picked.input, 9.0);
    }

    #[test]
    fn cost_multiplies_rate_by_tokens() {
        let rate = ModelRate {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.30),
            cache_write_5m: Some(3.75),
            cache_write_1h: Some(6.00),
            as_of: "2026-01-01".into(),
        };
        // 1M input at $3 + 1M output at $15.
        let cost = cost_of(&usage(1_000_000, 1_000_000), &rate).unwrap();
        assert_eq!(cost, MicroUsd(18_000_000));
        assert!((cost.to_usd() - 18.0).abs() < 1e-9);
    }

    /// The premium is the whole reason the two write rates are separate: a
    /// blended figure would match neither the invoice nor the API's own split.
    #[test]
    fn cache_writes_split_across_both_ttl_rates() {
        let rate = ModelRate {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.30),
            cache_write_5m: Some(3.75),
            cache_write_1h: Some(6.00),
            as_of: "2026-01-01".into(),
        };
        let usage = TokenUsage {
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: None,
            cache_write_tokens: Some(1_000_000),
            cache_write_1h_tokens: Some(400_000),
        };
        // 600k at $3.75/MTok + 400k at $6.00/MTok = $2.25 + $2.40.
        assert_eq!(cost_of(&usage, &rate).unwrap(), MicroUsd(4_650_000));
    }

    /// Never emit a partial cost as if it were the whole cost.
    #[test]
    fn a_reported_component_with_no_rate_makes_the_whole_cost_unknown() {
        let no_cache_rate = ModelRate {
            input: 3.0,
            output: 15.0,
            cache_read: None,
            cache_write_5m: None,
            cache_write_1h: None,
            as_of: "2026-01-01".into(),
        };
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
            cache_read_tokens: Some(1_000),
            ..Default::default()
        };
        assert!(cost_of(&usage, &no_cache_rate).is_none());

        // A *zero* count is not a reported component — pricing it would need a
        // rate for tokens that do not exist.
        let zeroed = TokenUsage {
            cache_read_tokens: Some(0),
            ..usage.clone()
        };
        assert!(cost_of(&zeroed, &no_cache_rate).is_some());
    }

    /// The direct counter-test to the float accumulator this replaced: summing
    /// many small costs must not depend on the order they were added.
    #[test]
    fn micro_usd_sums_are_order_independent() {
        let costs: Vec<MicroUsd> = (1..=10_000).map(MicroUsd).collect();
        let forward: MicroUsd = costs.iter().copied().sum();
        let backward: MicroUsd = costs.iter().rev().copied().sum();
        // Same values, interleaved differently.
        let interleaved: MicroUsd = costs
            .iter()
            .step_by(2)
            .chain(costs.iter().skip(1).step_by(2))
            .copied()
            .sum();
        assert_eq!(forward, backward);
        assert_eq!(forward, interleaved);
    }

    #[test]
    fn from_usd_clamps_nonsense_instead_of_wrapping() {
        assert_eq!(MicroUsd::from_usd(-1.0), MicroUsd::ZERO);
        assert_eq!(MicroUsd::from_usd(f64::NAN), MicroUsd::ZERO);
        assert_eq!(MicroUsd::from_usd(0.0125), MicroUsd(12_500));
    }
}
