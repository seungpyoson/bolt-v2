//! Net-new informed-fraction (μ) estimator and fail-closed health gate over the
//! shared signed trade-flow buffer (Slice 2, #488).
//!
//! NautilusTrader provides the signed-trade input (`TradeTick.aggressor_side`)
//! and the retention buffer ([`crate::bolt_v3_trade_flow::SignedTradeFlow`]) but
//! **no** production order-flow-toxicity / VPIN / informed-fraction estimator
//! (its `signed_vpin` exists only inside a feature-gated example strategy). This
//! module is that genuine residue.
//!
//! [`estimate_informed_fraction`] reduces the signed flow inside the retention
//! window to a single VPIN-style **order-flow-imbalance magnitude**
//! `μ = |buy_volume − sell_volume| / (buy_volume + sell_volume) ∈ [0, 1]`:
//! `0` is perfectly balanced flow (no directional information), `1` is fully
//! one-sided (maximally toxic). This μ is the `informed_fraction` consumed by
//! [`crate::bolt_v3_maker_model::gm_binary_quote`] (wired in Slice 3).
//!
//! [`evaluate_mu_health`] is the fail-closed gate: an absent, stale, non-finite,
//! or degenerate (below-floor / constant-0) μ blocks quoting and go-live, because
//! `gm_binary_quote` accepts `μ = 0` and collapses the spread to `bid = ask =
//! fair` — a zero-spread quote that earns no compensation for pick-off risk.
//! Every threshold is supplied by the caller from TOML; nothing defaults.

use crate::bolt_v3_numeric::{is_positive_finite, sanitize_probability};
use crate::bolt_v3_trade_flow::SignedTradeFlow;
use nautilus_model::enums::AggressorSide;

/// Runtime view of the μ-estimator knobs, projected from strategy TOML at the
/// call site so this module never depends on a strategy config type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuEstimatorConfig {
    /// Minimum number of classified (`Buyer`/`Seller`) samples inside the window
    /// required before a μ is produced; below this the estimator is warming up
    /// and returns `None` (fail-closed).
    pub min_classified_samples: u64,
}

/// Runtime view of the μ-health-gate knobs, projected from strategy TOML.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MuHealthConfig {
    /// Maximum age (ms) of the most recent trade before μ is considered stale.
    pub stale_window_ms: u64,
    /// Lower bound (exclusive of degenerate values below it) μ must reach to be
    /// healthy; a μ below this floor is treated as constant-0/degenerate and
    /// blocks quoting and go-live (spec §15: μ=0 collapses the GM spread).
    pub mu_min_floor: f64,
}

/// Why a μ reading blocks quoting / go-live. `None` from
/// [`evaluate_mu_health`] means healthy; `Some(reason)` blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuHealthReason {
    /// No trade has been observed, or the window holds no producible μ.
    Absent,
    /// The most recent trade is older than the configured stale window.
    Stale,
    /// μ is NaN or infinite.
    NotFinite,
    /// μ is below the configured floor (degenerate / constant-0).
    BelowFloor,
}

/// Estimate the informed-fraction μ ∈ [0, 1] from the signed flow inside the
/// retention window as of `now_ms`.
///
/// Only `Buyer`/`Seller` aggressors are counted; `NoAggressor` (the NT default,
/// emitted for default-constructed or replay ticks) is excluded from both the
/// volume sums and the classified-sample count — an unclassified trade is never
/// treated as net-zero flow or as a side. Returns `None` (fail-closed) when the
/// classified-sample count is below `cfg.min_classified_samples`, when the total
/// classified volume is not strictly positive, or when the result is non-finite.
pub fn estimate_informed_fraction(
    flow: &SignedTradeFlow,
    now_ms: u64,
    cfg: &MuEstimatorConfig,
) -> Option<f64> {
    let classified_count = flow
        .samples_within(now_ms)
        .filter(|sample| {
            matches!(
                sample.aggressor,
                AggressorSide::Buyer | AggressorSide::Seller
            )
        })
        .count() as u64;
    if classified_count < cfg.min_classified_samples {
        return None;
    }

    let buy_volume: f64 = flow
        .samples_within(now_ms)
        .filter(|sample| matches!(sample.aggressor, AggressorSide::Buyer))
        .map(|sample| sample.size)
        .sum();
    let sell_volume: f64 = flow
        .samples_within(now_ms)
        .filter(|sample| matches!(sample.aggressor, AggressorSide::Seller))
        .map(|sample| sample.size)
        .sum();

    let total_volume = buy_volume + sell_volume;
    if !is_positive_finite(total_volume) {
        return None;
    }

    // |buy − sell| / total ∈ [0, 1] when total > 0 (since |buy − sell| ≤ buy +
    // sell); sanitize_probability returns it as Some, and fails closed to None on
    // any non-finite slip.
    sanitize_probability((buy_volume - sell_volume).abs() / total_volume)
}

/// Fail-closed μ-health gate. Returns `None` when μ is healthy (quoting/go-live
/// permitted) and `Some(reason)` when it must block. Checks apply in order, so
/// the first failure wins: absent data → stale data → absent μ → non-finite μ →
/// below-floor μ.
pub fn evaluate_mu_health(
    mu: Option<f64>,
    last_trade_ms: Option<u64>,
    now_ms: u64,
    cfg: &MuHealthConfig,
) -> Option<MuHealthReason> {
    match last_trade_ms {
        None => return Some(MuHealthReason::Absent),
        Some(last) => {
            if now_ms.saturating_sub(last) > cfg.stale_window_ms {
                return Some(MuHealthReason::Stale);
            }
        }
    }

    match mu {
        None => Some(MuHealthReason::Absent),
        Some(value) if !value.is_finite() => Some(MuHealthReason::NotFinite),
        Some(value) if value < cfg.mu_min_floor => Some(MuHealthReason::BelowFloor),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_numeric::NANOS_PER_MILLI_U64;
    use crate::bolt_v3_trade_flow::SignedTradeFlowConfig;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        data::TradeTick,
        identifiers::{InstrumentId, TradeId},
        types::{Price, Quantity},
    };

    const TEST_IDENTIFIER_TOKEN_LIMIT: usize = 16;
    const TEST_TRADE_PRICE_PRECISION: u8 = 2;
    const TEST_TRADE_SIZE_PRECISION: u8 = u8::MIN;
    const TEST_WINDOW_SECS: u64 = 600;
    const TEST_MAX_SAMPLES: u64 = 1_000;
    const TEST_MIN_CLASSIFIED: u64 = 4;
    const TEST_TRADE_PRICE: f64 = 0.50;
    const TEST_UNIT_SIZE: f64 = 1.0;
    const TEST_FIRST_TRADE_TS_MS: u64 = 1_000;
    const TEST_TRADE_TS_STEP_MS: u64 = 1_000;
    const TEST_NOW_MS: u64 = 50_000;
    const TEST_AGED_OUT_NOW_MS: u64 = 10_000_000;
    const TEST_BALANCED_MU: f64 = 0.0;
    const TEST_ONE_SIDED_MU: f64 = 1.0;
    const TEST_SKEWED_MU: f64 = 0.5;
    const TEST_STALE_WINDOW_MS: u64 = 5_000;
    const TEST_MU_MIN_FLOOR: f64 = 0.05;
    const TEST_HEALTHY_MU: f64 = 0.40;
    const TEST_FRESH_LAST_TRADE_MS: u64 = 48_000;
    const TEST_STALE_LAST_TRADE_MS: u64 = 40_000;
    const TEST_HEALTH_NOW_MS: u64 = 50_000;

    fn token(raw: &str) -> String {
        raw.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .take(TEST_IDENTIFIER_TOKEN_LIMIT)
            .collect()
    }

    fn estimator_instrument_id() -> String {
        format!(
            "{}.{}",
            token(std::any::type_name::<MuEstimatorConfig>()).to_ascii_uppercase(),
            token(std::any::type_name::<MuHealthConfig>()).to_ascii_uppercase(),
        )
    }

    fn estimator_config() -> MuEstimatorConfig {
        MuEstimatorConfig {
            min_classified_samples: TEST_MIN_CLASSIFIED,
        }
    }

    fn trade_tick(
        instrument_id: &str,
        size: f64,
        aggressor: AggressorSide,
        ts_ms: u64,
    ) -> TradeTick {
        let ts_ns = ts_ms.saturating_mul(NANOS_PER_MILLI_U64);
        let trade_id = format!("{}{ts_ns}", token(std::any::type_name::<TradeTick>()));
        TradeTick::new_checked(
            InstrumentId::from(instrument_id),
            Price::new(TEST_TRADE_PRICE, TEST_TRADE_PRICE_PRECISION),
            Quantity::new(size, TEST_TRADE_SIZE_PRECISION),
            aggressor,
            TradeId::from(trade_id.as_str()),
            UnixNanos::from(ts_ns),
            UnixNanos::from(ts_ns),
        )
        .expect("test trade tick should be valid")
    }

    /// Build a flow by observing `(aggressor, size)` pairs at monotonically
    /// increasing timestamps so none are dropped by the buffer's non-monotonic
    /// guard.
    fn flow_with(samples: &[(AggressorSide, f64)]) -> SignedTradeFlow {
        let instrument_id = estimator_instrument_id();
        let mut flow = SignedTradeFlow::from_config(&SignedTradeFlowConfig {
            window_secs: TEST_WINDOW_SECS,
            max_samples: TEST_MAX_SAMPLES,
        });
        for (index, (aggressor, size)) in samples.iter().enumerate() {
            let ts_ms = TEST_FIRST_TRADE_TS_MS + (index as u64) * TEST_TRADE_TS_STEP_MS;
            flow.observe(&trade_tick(
                instrument_id.as_str(),
                *size,
                *aggressor,
                ts_ms,
            ));
        }
        flow
    }

    fn health_config() -> MuHealthConfig {
        MuHealthConfig {
            stale_window_ms: TEST_STALE_WINDOW_MS,
            mu_min_floor: TEST_MU_MIN_FLOOR,
        }
    }

    #[test]
    fn balanced_flow_yields_zero() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Seller, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Seller, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_BALANCED_MU)
        );
    }

    #[test]
    fn one_sided_flow_yields_one() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_ONE_SIDED_MU)
        );
    }

    #[test]
    fn skewed_flow_yields_imbalance_magnitude() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Seller, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_SKEWED_MU)
        );
    }

    #[test]
    fn below_minimum_classified_samples_is_none() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Seller, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            None
        );
    }

    #[test]
    fn no_aggressor_samples_are_excluded_and_yield_none() {
        let flow = flow_with(&[
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            None
        );
    }

    #[test]
    fn no_aggressor_does_not_change_classified_result() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::Seller, TEST_UNIT_SIZE),
        ]);
        // Four classified (3 Buyer, 1 Seller), two NoAggressor excluded → 0.5.
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_SKEWED_MU)
        );
    }

    #[test]
    fn aged_out_samples_yield_none() {
        let flow = flow_with(&[
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
            (AggressorSide::Buyer, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_AGED_OUT_NOW_MS, &estimator_config()),
            None
        );
    }

    #[test]
    fn absent_last_trade_blocks_even_with_healthy_mu() {
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_HEALTHY_MU),
                None,
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::Absent)
        );
    }

    #[test]
    fn absent_mu_with_fresh_data_blocks() {
        assert_eq!(
            evaluate_mu_health(
                None,
                Some(TEST_FRESH_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::Absent)
        );
    }

    #[test]
    fn stale_data_blocks_at_boundary_plus_one() {
        // now - last == stale_window is the healthy boundary; strictly greater is stale.
        let boundary_last = TEST_HEALTH_NOW_MS - TEST_STALE_WINDOW_MS;
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_HEALTHY_MU),
                Some(boundary_last),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            None
        );
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_HEALTHY_MU),
                Some(boundary_last - 1),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::Stale)
        );
    }

    #[test]
    fn stale_takes_precedence_over_below_floor() {
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_BALANCED_MU),
                Some(TEST_STALE_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::Stale)
        );
    }

    #[test]
    fn non_finite_mu_blocks() {
        assert_eq!(
            evaluate_mu_health(
                Some(f64::NAN),
                Some(TEST_FRESH_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::NotFinite)
        );
    }

    #[test]
    fn below_floor_mu_blocks() {
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_BALANCED_MU),
                Some(TEST_FRESH_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            Some(MuHealthReason::BelowFloor)
        );
    }

    #[test]
    fn at_floor_and_above_is_healthy() {
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_MU_MIN_FLOOR),
                Some(TEST_FRESH_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            None
        );
        assert_eq!(
            evaluate_mu_health(
                Some(TEST_HEALTHY_MU),
                Some(TEST_FRESH_LAST_TRADE_MS),
                TEST_HEALTH_NOW_MS,
                &health_config()
            ),
            None
        );
    }
}
