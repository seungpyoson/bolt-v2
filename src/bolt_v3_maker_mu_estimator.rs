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
use crate::bolt_v3_trade_flow::{SignedTrade, SignedTradeFlow};
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

/// A μ (informed-fraction) value that has already cleared the fail-closed health
/// gate. The inner `f64` is **private**: the only way to obtain a `UsableMu` is
/// from the per-instrument gate read
/// (`crate::strategies::binary_oracle_maker::mu::MakerMuState::usable_mu_for`),
/// which returns it solely when the gate passes. The consuming quote planner
/// ([`crate::bolt_v3_maker_quote_plan::MakerQuotePlanInputs`]) takes a `UsableMu`
/// — not a bare `f64` — for its informed-fraction input and reads the value with
/// [`get`](Self::get) only at the single `gm_binary_quote` call, so "a raw,
/// ungated μ reached `gm_binary_quote`" is a compile error: a bare `f64` cannot
/// be coerced into this type and the field cannot be constructed outside this
/// module.
///
/// It lives in this shared estimator module (not the strategy `mu` module) so the
/// shared `bolt_v3_*` quote planner can name it without referencing
/// `crate::strategies::*`, keeping the dependency-direction fence green.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsableMu(f64);

impl UsableMu {
    /// Wrap a gate-cleared μ. **Module-private** (not `pub(crate)`): the only
    /// in-crate caller is [`mint_usable_mu`] in this module, which runs the
    /// fail-closed health gate before constructing. Module privacy is enforced by
    /// the compiler regardless of call syntax — a `UsableMu::new`, UFCS
    /// `<UsableMu>::new`, alias, or macro mint in any *other* module is a compile
    /// error (E0603) — so the sole-mint property is structural, not a convention a
    /// text fence approximates.
    fn new(value: f64) -> Self {
        Self(value)
    }

    /// Test-only constructor for cross-module unit tests that need a `UsableMu`
    /// without standing up a full estimator + flow (e.g. the quote-planner unit
    /// tests). `#[cfg(test)]` means it does not exist in the production binary, so
    /// production sole-mint stays compiler-guaranteed; it is `pub(crate)` only so
    /// sibling-module test code can reach it.
    #[cfg(test)]
    pub(crate) fn for_test(value: f64) -> Self {
        Self(value)
    }

    /// The gate-cleared μ value, read at the single point that needs the raw
    /// number (`gm_binary_quote`). Kept deliberately minimal so no path can pull
    /// the value out earlier and route it around the gate.
    pub fn get(self) -> f64 {
        self.0
    }
}

/// Mint a [`UsableMu`] — the ONLY in-crate path to one — by running the
/// fail-closed health gate over the raw inputs and constructing only on a pass.
///
/// This co-locates the mint authorization with the type, in the shared estimator
/// module (so the `bolt_v3_*` quote planner can name `UsableMu` without a
/// `crate::strategies::*` dependency, keeping the dependency-direction fence
/// green). The seam takes the RAW `flow` + configs and computes μ, *derives* the
/// staleness anchor, *and* runs [`evaluate_mu_health`] itself, so a caller cannot
/// fabricate a "cleared" verdict to slip an ungated μ through — any `UsableMu`
/// this returns provably cleared the same health check `usable_mu_for` applies.
///
/// The staleness anchor is **derived here, not accepted from the caller**, and it
/// is the newest *classified* sample inside the retention window as of `now_ms`
/// (`classified_samples_within(flow, now_ms).last()`) — the exact sample set
/// [`estimate_informed_fraction`] reduces μ over, via the shared
/// [`classified_samples_within`] definition. Anchoring on the same set μ is
/// computed from is load-bearing: a fresh unclassified (`NoAggressor`) tick is
/// excluded from both, so it cannot read as fresh and mask stale classified data
/// (the fail-open this would otherwise have). Because the anchor is computed
/// internally from `flow` + `now_ms` rather than passed in, no same-crate caller
/// can forge a fresh staleness reference (e.g. `Some(now_ms)`) to mint a μ over
/// stale-but-in-window flow that the gate would otherwise reject as `Stale`.
///
/// Returns `Err(reason)` on a blocked gate and falls back to `Absent` rather than
/// unwrap so the mint stays fail-closed even if the μ and health views ever diverge.
pub(crate) fn mint_usable_mu(
    flow: &SignedTradeFlow,
    now_ms: u64,
    estimator: &MuEstimatorConfig,
    health: &MuHealthConfig,
) -> Result<UsableMu, MuHealthReason> {
    let mu = estimate_informed_fraction(flow, now_ms, estimator);
    // Anchor staleness on the newest sample that actually feeds μ — the newest
    // *classified* sample in-window — not the raw newest in-window sample. Using
    // the same `classified_samples_within` definition as the μ computation means a
    // fresh unclassified (`NoAggressor`) tick cannot read as fresh and mask stale
    // classified data (the fail-open this closes). `samples_within` is oldest-first
    // so `last()` is the newest classified sample; no classified samples → `None`
    // → `Absent`, matching the `mu == None` verdict.
    let last_trade_ms = classified_samples_within(flow, now_ms)
        .last()
        .map(|sample| sample.ts_ms);
    match evaluate_mu_health(mu, last_trade_ms, now_ms, health) {
        Some(reason) => Err(reason),
        None => mu.map(UsableMu::new).ok_or(MuHealthReason::Absent),
    }
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

/// The single definition of "a sample that contributes to μ": a trade inside the
/// retention window as of `now_ms` whose aggressor is classified (`Buyer` or
/// `Seller`). `NoAggressor` (the NT default, emitted for default-constructed or
/// replay ticks) is excluded — it is never treated as net-zero flow, as a side,
/// or as a freshness signal.
///
/// This is the ONE place that decides what feeds μ, and it is used by BOTH the μ
/// computation ([`estimate_informed_fraction`]) and the staleness anchor
/// derivation ([`mint_usable_mu`]). Sharing it is load-bearing: if the μ inputs
/// and the freshness anchor used different sample sets, a fresh unclassified tick
/// could mask stale classified data and read the gate as fresh (fail-open). They
/// cannot drift because there is only one predicate. `samples_within` yields
/// oldest-first, so the iterator's `last()` is the newest classified sample.
fn classified_samples_within(
    flow: &SignedTradeFlow,
    now_ms: u64,
) -> impl Iterator<Item = &SignedTrade> {
    flow.samples_within(now_ms)
        .filter(|sample| matches!(sample.aggressor, AggressorSide::Buy | AggressorSide::Sell))
}

/// Estimate the informed-fraction μ ∈ [0, 1] from the signed flow inside the
/// retention window as of `now_ms`.
///
/// Only `Buyer`/`Seller` aggressors are counted (via
/// [`classified_samples_within`]); `NoAggressor` (the NT default, emitted for
/// default-constructed or replay ticks) is excluded from both the volume sums and
/// the classified-sample count — an unclassified trade is never treated as
/// net-zero flow or as a side. Returns `None` (fail-closed) when the
/// classified-sample count is below `cfg.min_classified_samples`, when the total
/// classified volume is not strictly positive, or when the result is non-finite.
pub fn estimate_informed_fraction(
    flow: &SignedTradeFlow,
    now_ms: u64,
    cfg: &MuEstimatorConfig,
) -> Option<f64> {
    let classified_count = classified_samples_within(flow, now_ms).count() as u64;
    if classified_count < cfg.min_classified_samples {
        return None;
    }

    let buy_volume: f64 = classified_samples_within(flow, now_ms)
        .filter(|sample| matches!(sample.aggressor, AggressorSide::Buy))
        .map(|sample| sample.size)
        .sum();
    let sell_volume: f64 = classified_samples_within(flow, now_ms)
        .filter(|sample| matches!(sample.aggressor, AggressorSide::Sell))
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

    /// Build a flow by observing `(aggressor, size, ts_ms)` triples at the exact
    /// timestamps given (caller must supply them non-decreasing so none are dropped
    /// by the buffer's non-monotonic guard). Used where a test needs explicit gaps
    /// — e.g. stale classified data plus a fresh unclassified tick at `now_ms`.
    fn flow_with_ts(samples: &[(AggressorSide, f64, u64)]) -> SignedTradeFlow {
        let instrument_id = estimator_instrument_id();
        let mut flow = SignedTradeFlow::from_config(&SignedTradeFlowConfig {
            window_secs: TEST_WINDOW_SECS,
            max_samples: TEST_MAX_SAMPLES,
        });
        for (aggressor, size, ts_ms) in samples {
            flow.observe(&trade_tick(
                instrument_id.as_str(),
                *size,
                *aggressor,
                *ts_ms,
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
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Sell, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Sell, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_BALANCED_MU)
        );
    }

    #[test]
    fn one_sided_flow_yields_one() {
        let flow = flow_with(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_ONE_SIDED_MU)
        );
    }

    #[test]
    fn skewed_flow_yields_imbalance_magnitude() {
        let flow = flow_with(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Sell, TEST_UNIT_SIZE),
        ]);
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_SKEWED_MU)
        );
    }

    #[test]
    fn below_minimum_classified_samples_is_none() {
        let flow = flow_with(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Sell, TEST_UNIT_SIZE),
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
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::NoAggressor, TEST_UNIT_SIZE),
            (AggressorSide::Sell, TEST_UNIT_SIZE),
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
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
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
    fn mint_derives_staleness_anchor_and_blocks_stale_in_window_flow() {
        // X1 differential: a one-sided flow whose newest in-window sample is 46s
        // old (inside the 600s retention window so μ is producible, but past the 5s
        // stale window) must mint `Err(Stale)`. `mint_usable_mu` derives the
        // staleness anchor internally from `flow` + `now_ms`, so there is no
        // parameter through which a caller could forge a fresh reference
        // (e.g. `Some(TEST_NOW_MS)`) to slip this stale μ past the gate.
        //
        // Pre-fix `mint_usable_mu` accepted `last_trade_ms: Option<u64>`; a caller
        // passing `Some(TEST_NOW_MS)` here made `now - last == 0 <= stale_window`,
        // so the gate read fresh and the mint returned `Ok(UsableMu(1.0))` over
        // provably stale data. With the parameter removed and the anchor derived as
        // the newest in-window sample (ts 4_000, age 46_000ms), the gate now
        // correctly reports `Stale` and no caller-supplied timestamp can override it.
        let flow = flow_with(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
        ]);
        // Sanity: μ is producible for this flow at TEST_NOW_MS (one-sided → 1.0),
        // so the only thing blocking the mint is the derived staleness anchor.
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_NOW_MS, &estimator_config()),
            Some(TEST_ONE_SIDED_MU)
        );
        assert_eq!(
            mint_usable_mu(&flow, TEST_NOW_MS, &estimator_config(), &health_config()),
            Err(MuHealthReason::Stale)
        );
    }

    // A fresh unclassified tick must NOT refresh the staleness anchor: the anchor
    // is the newest *classified* sample, the same set μ is computed from.
    const TEST_STALE_CLASSIFIED_NOW_MS: u64 = 100_000;
    const TEST_FRESH_NOAGGRESSOR_TS_MS: u64 = 100_000;

    #[test]
    fn fresh_unclassified_tick_does_not_mask_stale_classified_data() {
        // X1 fail-open regression: μ is computed from CLASSIFIED samples, so the
        // staleness anchor must track the newest CLASSIFIED sample too. Four stale
        // buys (newest ts 4_000, 96s before now, inside the 600s retention window so
        // μ=1.0 is still producible) plus one FRESH `NoAggressor` tick at `now_ms`.
        // NoAggressor is the NT default for default-constructed/replay ticks, so this
        // is realistic. The fresh unclassified tick is the raw newest in-window
        // sample but contributes nothing to μ; anchoring on it would read fresh and
        // mint a `UsableMu` over provably stale classified data (fail-open).
        //
        // PRE-FIX (raw `samples_within(now).last()` anchor): the anchor is the
        // NoAggressor@100_000 → now-last == 0 ≤ stale_window → gate reads fresh →
        // mint returns `Ok(UsableMu(1.0))` over 96s-stale flow (the bug).
        // POST-FIX (shared `classified_samples_within(...).last()` anchor): the
        // anchor is the newest classified buy@4_000 → now-last == 96_000 >
        // stale_window → `Err(Stale)`. μ and the anchor share one classified
        // definition, so they cannot drift.
        let flow = flow_with_ts(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE, TEST_FIRST_TRADE_TS_MS),
            (
                AggressorSide::Buy,
                TEST_UNIT_SIZE,
                TEST_FIRST_TRADE_TS_MS + TEST_TRADE_TS_STEP_MS,
            ),
            (
                AggressorSide::Buy,
                TEST_UNIT_SIZE,
                TEST_FIRST_TRADE_TS_MS + 2 * TEST_TRADE_TS_STEP_MS,
            ),
            (
                AggressorSide::Buy,
                TEST_UNIT_SIZE,
                TEST_FIRST_TRADE_TS_MS + 3 * TEST_TRADE_TS_STEP_MS,
            ),
            (
                AggressorSide::NoAggressor,
                TEST_UNIT_SIZE,
                TEST_FRESH_NOAGGRESSOR_TS_MS,
            ),
        ]);
        // Sanity: μ is still producible (one-sided buys → 1.0); the unclassified
        // tick is excluded from μ, so only the staleness anchor decides the gate.
        assert_eq!(
            estimate_informed_fraction(&flow, TEST_STALE_CLASSIFIED_NOW_MS, &estimator_config()),
            Some(TEST_ONE_SIDED_MU)
        );
        assert_eq!(
            mint_usable_mu(
                &flow,
                TEST_STALE_CLASSIFIED_NOW_MS,
                &estimator_config(),
                &health_config()
            ),
            Err(MuHealthReason::Stale)
        );
    }

    #[test]
    fn mint_returns_gate_cleared_mu_for_fresh_in_window_flow() {
        // The legitimate production path is unchanged: a one-sided flow whose newest
        // in-window sample is fresh (age within the stale window) mints the
        // gate-cleared μ. `now_ms` is one stale-window past the newest sample
        // (ts 4_000) so the derived anchor reads fresh and the gate passes.
        let flow = flow_with(&[
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
            (AggressorSide::Buy, TEST_UNIT_SIZE),
        ]);
        let newest_ts_ms = TEST_FIRST_TRADE_TS_MS + 3 * TEST_TRADE_TS_STEP_MS;
        let fresh_now_ms = newest_ts_ms + TEST_STALE_WINDOW_MS;
        assert_eq!(
            mint_usable_mu(&flow, fresh_now_ms, &estimator_config(), &health_config())
                .map(UsableMu::get),
            Ok(TEST_ONE_SIDED_MU)
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
