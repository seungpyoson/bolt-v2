//! Pure market-selection scorer for the binary-oracle maker portfolio layer
//! (W5 — FR-041: rank candidate binary markets so only the most attractive
//! ones inside the portfolio budget are quoted, fail-closed).
//!
//! FR-041 splits one bankroll across N concurrent thin binary markets. Before
//! any capital is allocated, the portfolio must decide **which** markets are
//! worth quoting at all. This module is the "select markets" half: it turns the
//! per-market numeric signals the shell already computes (captured spread, top-
//! of-book liquidity, time to resolution) into a single comparable
//! attractiveness score, drops any market that is poisoned or too close to
//! expiry, and ranks the survivors descending. The resulting [`MarketScore`]
//! list is the single input the capital allocator ([`crate::strategies::
//! portfolio_allocator`]) and, transitively, the portfolio-risk aggregator
//! consume — this module is the root of the FR-041 pure pipeline.
//!
//! ## Why these three signals, and why higher is better
//!
//! A thin binary market is more attractive to a maker when (a) the captured
//! spread is wider (more edge per round-trip), (b) the resting book has more
//! top-of-book liquidity to lean on, and (c) there is more time to resolution
//! (more requote cycles before the oracle settles and the position becomes a
//! coin-flip). All three are monotonically "more is better", so the score is a
//! plain non-negative weighted sum of the raw features. The caller owns the
//! relative importance via [`SelectionWeights`]; the scorer never invents a
//! threshold or a weight (NO HARDCODES — every tuning value is config-supplied).
//!
//! ## Fail-closed selection
//!
//! A market that cannot be scored is **excluded**, never quoted on a guess:
//! [`SelectionWeights::score`] returns `None` on any non-finite feature and on a
//! market at/below the zero time-to-resolution floor (too close to expiry to
//! quote safely — the same tau-floor rationale the maker governor uses to kill
//! a near-expiry market). [`SelectionWeights::rank`] silently drops every
//! excluded candidate, so a poisoned feed can never enter the allocation. An
//! all-zero or non-finite weight set cannot even be constructed, so a
//! meaningless ranking is structurally impossible.
//!
//! Pure: no NautilusTrader type, no async, no I/O. No `Default` (an empty weight
//! set is not a meaningful identity — every weight must be chosen). All numeric
//! invariants come from [`crate::bolt_v3_numeric`]; no inline runtime literal on
//! the production path.

use crate::bolt_v3_numeric::{ZERO_F64, is_positive_finite};

/// A market's stable identity within the portfolio. A newtype over `String` so
/// the rest of the pipeline keys on an opaque, caller-supplied id rather than a
/// bare string (NO HARDCODES — the id is never literal in code).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MarketKey(String);

impl MarketKey {
    /// Wrap a caller-supplied market id. Total: the id's provenance (config,
    /// venue feed) is the caller's concern; this layer only carries it.
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// Borrow the underlying id for keying/logging by the shell.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A market's computed attractiveness score, the shared currency of the FR-041
/// pure pipeline: produced here, consumed by the capital allocator.
///
/// `score` is always a finite, non-negative weighted sum (the scorer rejects any
/// path that could produce a non-finite or negative value), so downstream
/// proportional-weight math never divides by or sums a degenerate score.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketScore {
    pub market: MarketKey,
    pub score: f64,
}

/// The raw per-market features the shell measures each tick. All are
/// venue-agnostic numerics: `captured_spread` is the quote band half-width the
/// maker would earn, `top_of_book_liquidity` is the touch size to lean on, and
/// `seconds_to_resolution` is the time remaining before the oracle settles.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketCandidate {
    pub market: MarketKey,
    pub captured_spread: f64,
    pub top_of_book_liquidity: f64,
    pub seconds_to_resolution: f64,
}

/// Caller-supplied relative importance of each selection feature. Constructed
/// only through [`SelectionWeights::new`], which rejects any non-finite or
/// negative weight and any all-zero set, so a meaningless ranking can never be
/// built.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionWeights {
    spread_weight: f64,
    liquidity_weight: f64,
    tau_weight: f64,
}

impl SelectionWeights {
    /// Validate-at-construction (mirrors the bolt-v3 `new() -> Option` fence; no
    /// `Default`). Returns `None` (fail-closed) unless every weight is finite and
    /// `>= ZERO_F64`, and at least one weight is strictly positive — an all-zero
    /// weight set would score every market identically and rank meaninglessly.
    pub fn new(spread_weight: f64, liquidity_weight: f64, tau_weight: f64) -> Option<Self> {
        let weights = [spread_weight, liquidity_weight, tau_weight];
        if !weights.iter().all(|w| w.is_finite() && *w >= ZERO_F64) {
            return None;
        }
        if !weights.iter().any(|w| is_positive_finite(*w)) {
            return None;
        }
        Some(Self {
            spread_weight,
            liquidity_weight,
            tau_weight,
        })
    }

    /// Score one candidate, or `None` (excluded from selection) when the market
    /// cannot be quoted safely: any non-finite feature, or a non-positive
    /// `seconds_to_resolution` (at/below the zero time floor → too close to
    /// expiry to rest quotes). Otherwise a finite, non-negative weighted sum
    /// where higher = more attractive (wider spread, deeper book, more time).
    ///
    /// A non-finite weighted sum (overflow to infinity) also yields `None` so a
    /// degenerate score never enters the ranking.
    pub fn score(&self, candidate: &MarketCandidate) -> Option<f64> {
        if !(candidate.captured_spread.is_finite()
            && candidate.top_of_book_liquidity.is_finite()
            && is_positive_finite(candidate.seconds_to_resolution))
        {
            return None;
        }
        // Negative spread/liquidity are nonsensical inputs for a maker; treat
        // them as exclusion rather than letting a negative term silently lower
        // an otherwise-attractive score.
        if candidate.captured_spread < ZERO_F64 || candidate.top_of_book_liquidity < ZERO_F64 {
            return None;
        }
        let weighted = self.spread_weight * candidate.captured_spread
            + self.liquidity_weight * candidate.top_of_book_liquidity
            + self.tau_weight * candidate.seconds_to_resolution;
        if weighted.is_finite() && weighted >= ZERO_F64 {
            Some(weighted)
        } else {
            None
        }
    }

    /// Score every candidate, drop the excluded ones, and return the survivors
    /// sorted by descending score. An empty input (or all-excluded input) ranks
    /// to an empty `Vec` — nothing selected, quote nothing (fail-closed).
    pub fn rank(&self, candidates: &[MarketCandidate]) -> Vec<MarketScore> {
        let mut scored: Vec<MarketScore> = candidates
            .iter()
            .filter_map(|candidate| {
                self.score(candidate).map(|score| MarketScore {
                    market: candidate.market.clone(),
                    score,
                })
            })
            .collect();
        // Scores are finite by construction in `score`, so `total_cmp` gives a
        // stable total order; descending = most attractive first.
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> MarketKey {
        MarketKey::new(id.to_string())
    }

    fn candidate(id: &str, spread: f64, liquidity: f64, tau: f64) -> MarketCandidate {
        MarketCandidate {
            market: key(id),
            captured_spread: spread,
            top_of_book_liquidity: liquidity,
            seconds_to_resolution: tau,
        }
    }

    #[test]
    fn weights_reject_non_finite_negative_and_all_zero() {
        assert!(SelectionWeights::new(f64::NAN, 1.0, 1.0).is_none());
        assert!(SelectionWeights::new(-0.1, 1.0, 1.0).is_none());
        assert!(SelectionWeights::new(0.0, 0.0, 0.0).is_none());
        // At least one strictly positive is enough.
        assert!(SelectionWeights::new(0.0, 0.0, 1.0).is_some());
    }

    #[test]
    fn score_is_the_weighted_sum() {
        let weights = SelectionWeights::new(2.0, 3.0, 0.5).unwrap();
        let scored = weights.score(&candidate("m", 0.10, 4.0, 60.0)).unwrap();
        assert_eq!(scored, 2.0 * 0.10 + 3.0 * 4.0 + 0.5 * 60.0);
    }

    #[test]
    fn score_excludes_non_finite_feature() {
        let weights = SelectionWeights::new(1.0, 1.0, 1.0).unwrap();
        assert!(
            weights
                .score(&candidate("m", f64::NAN, 4.0, 60.0))
                .is_none()
        );
        assert!(
            weights
                .score(&candidate("m", 0.10, f64::INFINITY, 60.0))
                .is_none()
        );
    }

    #[test]
    fn score_excludes_market_at_or_below_zero_time_floor() {
        let weights = SelectionWeights::new(1.0, 1.0, 1.0).unwrap();
        assert!(weights.score(&candidate("m", 0.10, 4.0, 0.0)).is_none());
        assert!(weights.score(&candidate("m", 0.10, 4.0, -5.0)).is_none());
    }

    #[test]
    fn score_excludes_negative_spread_or_liquidity() {
        let weights = SelectionWeights::new(1.0, 1.0, 1.0).unwrap();
        assert!(weights.score(&candidate("m", -0.01, 4.0, 60.0)).is_none());
        assert!(weights.score(&candidate("m", 0.10, -1.0, 60.0)).is_none());
    }

    #[test]
    fn rank_sorts_descending_and_drops_excluded() {
        let weights = SelectionWeights::new(1.0, 0.0, 0.0).unwrap();
        let candidates = [
            candidate("low", 0.05, 1.0, 60.0),
            candidate("dead", 0.20, 1.0, 0.0), // excluded: at expiry floor
            candidate("high", 0.30, 1.0, 60.0),
            candidate("mid", 0.10, 1.0, 60.0),
        ];
        let ranked = weights.rank(&candidates);
        let ids: Vec<&str> = ranked.iter().map(|s| s.market.as_str()).collect();
        assert_eq!(ids, vec!["high", "mid", "low"]);
    }

    #[test]
    fn rank_of_empty_or_all_excluded_is_empty() {
        let weights = SelectionWeights::new(1.0, 1.0, 1.0).unwrap();
        assert!(weights.rank(&[]).is_empty());
        let all_dead = [
            candidate("a", f64::NAN, 1.0, 60.0),
            candidate("b", 0.10, 1.0, 0.0),
        ];
        assert!(weights.rank(&all_dead).is_empty());
    }

    #[test]
    fn higher_spread_outranks_lower_when_only_spread_weighted() {
        let weights = SelectionWeights::new(1.0, 0.0, 0.0).unwrap();
        let a = weights.score(&candidate("a", 0.10, 99.0, 1.0)).unwrap();
        let b = weights.score(&candidate("b", 0.30, 1.0, 99.0)).unwrap();
        assert!(b > a, "spread-only weighting ranks wider spread higher");
    }
}
