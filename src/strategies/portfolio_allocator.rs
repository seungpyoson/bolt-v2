//! Pure capital allocator for the binary-oracle maker portfolio layer
//! (W5 — FR-041: split one bankroll into per-market budgets from per-market
//! selection scores, fail-closed, over-allocation structurally impossible).
//!
//! This is the "split capital" half of FR-041. It consumes the ranked
//! [`MarketScore`] list produced by [`crate::strategies::portfolio_selection`]
//! and a single total bankroll, and emits a per-market budget for the top
//! markets. Each per-market budget becomes that market's reservation ceiling:
//! the FR-040 per-market reserved-collateral gate
//! ([`crate::strategies::maker_reservation`]) treats it as the
//! `available_collateral` for that market, and the portfolio-risk aggregator
//! sums the resulting reservations against the portfolio cap. So this module
//! sits between selection and reservation in the FR-041 pipeline.
//!
//! ## Allocation rule, and why it can never over-allocate
//!
//! 1. Drop any score that is not positive-finite (a non-positive or non-finite
//!    signal earns no capital — fail-closed, no allocation to a garbage score).
//! 2. Take the top `max_markets` by score (concentration by count).
//! 3. Split `total_bankroll` proportionally to each selected market's share of
//!    the selected-score sum.
//! 4. Clamp every per-market budget to `total_bankroll * per_market_cap_fraction`
//!    (concentration by size — no single market may exceed this fraction).
//!
//! Both the top-K cut and the per-market clamp can only *shrink* an allocation,
//! never grow it, and a proportional split of a fixed pool sums to at most the
//! pool. Therefore `sum(budgets) <= total_bankroll` holds by construction — over-
//! allocation is impossible, not merely checked. Any capital left by the clamp
//! is deliberately left unallocated rather than redistributed, so a single tight
//! market can never pull the portfolio over budget through a rebalancing loop.
//!
//! ## Fail-closed
//!
//! [`AllocatorConfig::new`] returns the full list of every out-of-domain field
//! (mirroring the maker-config validation pattern — surface all problems at
//! once, not one per run) so a degenerate config can never construct a
//! permissive allocator. [`AllocatorConfig::allocate`] returns an empty `Vec`
//! when no score is positive-finite or the selected-score sum is zero — nothing
//! allocated, quote nothing.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default`. All numeric
//! invariants come from [`crate::bolt_v3_numeric`]; no inline runtime literal on
//! the production path.

use crate::bolt_v3_numeric::{UNIT_F64, is_positive_finite};
use crate::strategies::portfolio_selection::{MarketKey, MarketScore};

/// One market's allocated capital budget. `budget` is a positive-finite USDC
/// figure (the allocator never emits a zero/non-finite budget) and is the
/// reservation ceiling the FR-040 gate enforces for that market.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketAllocation {
    pub market: MarketKey,
    pub budget: f64,
}

/// Validated allocator configuration. Constructed only through
/// [`AllocatorConfig::new`]; no `Default` (a bankroll of zero is not a
/// meaningful identity).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AllocatorConfig {
    total_bankroll: f64,
    max_markets: u32,
    per_market_cap_fraction: f64,
}

impl AllocatorConfig {
    /// Validate every field, accumulating all problems into the error list
    /// (mirrors `ValidatedMakerConfig::validate` — report all at once). Rejects:
    /// non-positive/non-finite `total_bankroll`; `max_markets == 0` (no market
    /// could ever be funded); `per_market_cap_fraction` outside `(0, 1]` (a
    /// fraction of zero funds nothing; above one is not a fraction — the
    /// concentration guard would be meaningless).
    pub fn new(
        total_bankroll: f64,
        max_markets: u32,
        per_market_cap_fraction: f64,
    ) -> Result<Self, Vec<String>> {
        let mut errors: Vec<String> = Vec::new();
        if !is_positive_finite(total_bankroll) {
            errors.push("total_bankroll must be positive and finite".to_string());
        }
        if max_markets == 0 {
            errors.push("max_markets must be at least 1".to_string());
        }
        if !(is_positive_finite(per_market_cap_fraction) && per_market_cap_fraction <= UNIT_F64) {
            errors.push("per_market_cap_fraction must lie in (0, 1]".to_string());
        }
        if errors.is_empty() {
            Ok(Self {
                total_bankroll,
                max_markets,
                per_market_cap_fraction,
            })
        } else {
            Err(errors)
        }
    }

    /// Split the bankroll across the top markets by score. Returns an empty
    /// `Vec` (fail-closed) when no score is positive-finite or the selected-score
    /// sum is zero. The returned budgets satisfy `sum(budgets) <= total_bankroll`
    /// and each `budget <= total_bankroll * per_market_cap_fraction` by
    /// construction (see module docs).
    pub fn allocate(&self, scores: &[MarketScore]) -> Vec<MarketAllocation> {
        // 1. Drop garbage signals — only a positive-finite score earns capital.
        let mut selected: Vec<&MarketScore> = scores
            .iter()
            .filter(|s| is_positive_finite(s.score))
            .collect();
        if selected.is_empty() {
            return Vec::new();
        }

        // 2. Top-K by score (descending); the input may already be ranked, but
        // re-sort here so the allocator does not depend on caller ordering.
        selected.sort_by(|a, b| b.score.total_cmp(&a.score));
        let keep = (self.max_markets as usize).min(selected.len());
        let selected = &selected[..keep];

        // 3. Proportional-weight denominator. If non-finite or non-positive,
        // nothing can be split — fail closed to empty.
        let score_sum: f64 = selected.iter().map(|s| s.score).sum();
        if !is_positive_finite(score_sum) {
            return Vec::new();
        }

        // 4. Proportional split, each clamped to the per-market concentration
        // cap. Both operations only shrink, so the total never exceeds bankroll.
        let per_market_cap = self.total_bankroll * self.per_market_cap_fraction;
        selected
            .iter()
            .filter_map(|s| {
                let share = s.score / score_sum;
                let raw = self.total_bankroll * share;
                let budget = raw.min(per_market_cap);
                // A clamp/split that produced a non-positive-finite budget funds
                // nothing — drop it rather than emit a degenerate allocation.
                if is_positive_finite(budget) {
                    Some(MarketAllocation {
                        market: s.market.clone(),
                        budget,
                    })
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(id: &str, score: f64) -> MarketScore {
        MarketScore {
            market: MarketKey::new(id.to_string()),
            score,
        }
    }

    #[test]
    fn config_rejects_every_out_of_domain_field_at_once() {
        let errors = AllocatorConfig::new(0.0, 0, 2.0).unwrap_err();
        assert_eq!(errors.len(), 3, "all three problems reported together");
    }

    #[test]
    fn config_accepts_valid_inputs() {
        assert!(AllocatorConfig::new(1_000.0, 3, 0.5).is_ok());
        assert!(AllocatorConfig::new(1_000.0, 1, 1.0).is_ok());
    }

    #[test]
    fn proportional_split_matches_score_weight() {
        let config = AllocatorConfig::new(100.0, 10, 1.0).unwrap();
        let allocations = config.allocate(&[score("a", 3.0), score("b", 1.0)]);
        let a = allocations
            .iter()
            .find(|m| m.market.as_str() == "a")
            .unwrap();
        let b = allocations
            .iter()
            .find(|m| m.market.as_str() == "b")
            .unwrap();
        assert_eq!(a.budget, 100.0 * (3.0 / 4.0));
        assert_eq!(b.budget, 100.0 * (1.0 / 4.0));
    }

    #[test]
    fn sum_of_budgets_never_exceeds_bankroll() {
        let config = AllocatorConfig::new(50.0, 10, 1.0).unwrap();
        let allocations = config.allocate(&[score("a", 5.0), score("b", 3.0), score("c", 2.0)]);
        let total: f64 = allocations.iter().map(|m| m.budget).sum();
        assert!(total <= 50.0 + f64::EPSILON);
    }

    #[test]
    fn per_market_cap_clamps_concentration() {
        // One dominant market would take ~91% by weight; the 0.5 cap clamps it.
        let config = AllocatorConfig::new(100.0, 10, 0.5).unwrap();
        let allocations = config.allocate(&[score("whale", 10.0), score("minnow", 1.0)]);
        let whale = allocations
            .iter()
            .find(|m| m.market.as_str() == "whale")
            .unwrap();
        assert_eq!(whale.budget, 100.0 * 0.5);
    }

    #[test]
    fn top_k_keeps_only_highest_scores() {
        let config = AllocatorConfig::new(100.0, 2, 1.0).unwrap();
        let allocations = config.allocate(&[score("a", 1.0), score("b", 5.0), score("c", 3.0)]);
        let ids: Vec<&str> = allocations.iter().map(|m| m.market.as_str()).collect();
        assert_eq!(allocations.len(), 2);
        assert!(ids.contains(&"b") && ids.contains(&"c") && !ids.contains(&"a"));
    }

    #[test]
    fn non_positive_or_non_finite_scores_are_dropped() {
        let config = AllocatorConfig::new(100.0, 10, 1.0).unwrap();
        let allocations = config.allocate(&[
            score("good", 4.0),
            score("zero", 0.0),
            score("neg", -1.0),
            score("nan", f64::NAN),
        ]);
        assert_eq!(allocations.len(), 1);
        assert_eq!(allocations[0].market.as_str(), "good");
        // The lone funded market takes the whole bankroll (it is the only weight).
        assert_eq!(allocations[0].budget, 100.0);
    }

    #[test]
    fn empty_or_all_garbage_allocates_nothing() {
        let config = AllocatorConfig::new(100.0, 10, 1.0).unwrap();
        assert!(config.allocate(&[]).is_empty());
        assert!(
            config
                .allocate(&[score("a", -1.0), score("b", f64::INFINITY)])
                .is_empty()
        );
    }
}
