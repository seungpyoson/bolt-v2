//! Bolt-v3 shared realized-volatility estimator.
//!
//! This module is the single home for the naive windowed realized-volatility
//! estimator that strategies feed into the market-family fair-value model as a
//! `realized_vol` read. It depends only on [`crate::bolt_v3_numeric`] for the
//! shared numeric/time primitives, so it sits below the strategy layer and can
//! be imported without introducing a cycle: it pulls in nothing from any
//! strategy module.
//!
//! Strategies own their own TOML deserialization; they project the volatility
//! window/sample knobs into [`RealizedVolConfig`] (a plain runtime view, not a
//! serde type) and stream spot observations in as primitive samples. The
//! estimator never sees a strategy config or quote type.

use std::collections::VecDeque;

use crate::bolt_v3_numeric::{
    MILLIS_PER_SECOND_F64, MILLIS_PER_SECOND_U64, POWER_OF_TWO, SECONDS_PER_YEAR_F64, ZERO_F64,
    is_positive_finite,
};

const MIN_OBSERVATION_COUNT: u64 = 1;
const INITIAL_COUNTER_U64: u64 = 0;
const INITIAL_COUNTER_USIZE: usize = 0;
const COUNTER_INCREMENT: usize = 1;

/// Runtime view of the volatility-window knobs the estimator needs.
///
/// This is a plain runtime config view, deliberately without a serde derive:
/// strategies keep owning the TOML deserialization on their own config structs
/// and project the relevant fields into this struct at the call site. All four
/// fields carry the same seconds-based semantics as the originating TOML
/// values; the estimator performs the seconds-to-milliseconds conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealizedVolConfig {
    pub window_secs: u64,
    pub gap_reset_secs: u64,
    pub min_observations: u64,
    pub bridge_valid_secs: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct VolatilitySample {
    ts_ms: u64,
    price: f64,
}

/// Naive windowed realized-volatility estimator over a single venue's spot
/// observations.
///
/// The estimator retains a rolling window of price samples, computes an
/// annualized realized volatility once enough observations accumulate, and
/// bridges the last ready value forward for a bounded interval. Fields read by
/// the strategy layer (state inspection and the bridge-validity horizon) are
/// public; the sample buffer remains private and is exercised only by this
/// module's own tests.
#[derive(Debug, Clone, PartialEq)]
pub struct RealizedVolEstimator {
    window_ms: u64,
    gap_reset_ms: u64,
    min_observations: u64,
    pub bridge_valid_ms: u64,
    pub active_venue: Option<String>,
    samples: VecDeque<VolatilitySample>,
    pub last_ready_vol: Option<f64>,
    pub last_ready_ts_ms: Option<u64>,
}

impl RealizedVolEstimator {
    pub fn from_config(config: &RealizedVolConfig) -> Self {
        Self {
            window_ms: config.window_secs.saturating_mul(MILLIS_PER_SECOND_U64),
            gap_reset_ms: config.gap_reset_secs.saturating_mul(MILLIS_PER_SECOND_U64),
            min_observations: config.min_observations,
            bridge_valid_ms: config
                .bridge_valid_secs
                .saturating_mul(MILLIS_PER_SECOND_U64),
            active_venue: None,
            samples: VecDeque::new(),
            last_ready_vol: None,
            last_ready_ts_ms: None,
        }
    }

    pub fn empty_like(&self) -> Self {
        Self {
            window_ms: self.window_ms,
            gap_reset_ms: self.gap_reset_ms,
            min_observations: self.min_observations,
            bridge_valid_ms: self.bridge_valid_ms,
            active_venue: None,
            samples: VecDeque::new(),
            last_ready_vol: None,
            last_ready_ts_ms: None,
        }
    }

    fn reset(&mut self) {
        self.active_venue = None;
        self.samples.clear();
        self.last_ready_vol = None;
        self.last_ready_ts_ms = None;
    }

    pub fn observe(&mut self, venue: &str, price: f64, observed_ts_ms: u64) -> Option<f64> {
        if !is_positive_finite(price) {
            return None;
        }

        if self.active_venue.as_deref() != Some(venue) {
            self.reset();
            self.active_venue = Some(venue.to_string());
        }

        if let Some(previous) = self.samples.back() {
            if observed_ts_ms <= previous.ts_ms {
                return self.current_vol_at(observed_ts_ms);
            }
            if observed_ts_ms.saturating_sub(previous.ts_ms) > self.gap_reset_ms {
                self.reset();
                self.active_venue = Some(venue.to_string());
            }
        }

        self.samples.push_back(VolatilitySample {
            ts_ms: observed_ts_ms,
            price,
        });
        self.evict_old_samples(observed_ts_ms);

        if let Some(vol) = self.compute_ready_vol() {
            self.last_ready_vol = Some(vol);
            self.last_ready_ts_ms = Some(observed_ts_ms);
        }

        self.current_vol_at(observed_ts_ms)
    }

    pub fn current_vol_at(&self, now_ms: u64) -> Option<f64> {
        let last_ready_ts_ms = self.last_ready_ts_ms?;
        if now_ms.checked_sub(last_ready_ts_ms)? <= self.bridge_valid_ms {
            self.last_ready_vol
        } else {
            None
        }
    }

    fn evict_old_samples(&mut self, now_ms: u64) {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        while self.samples.len() > 1
            && self
                .samples
                .front()
                .is_some_and(|sample| sample.ts_ms < cutoff_ms)
        {
            let _ = self.samples.pop_front();
        }
    }

    fn compute_ready_vol(&self) -> Option<f64> {
        let min_observations = self.min_observations.max(MIN_OBSERVATION_COUNT) as usize;
        let mut observation_count = INITIAL_COUNTER_USIZE;
        let mut elapsed_ms = INITIAL_COUNTER_U64;
        let mut sum_squared_returns = ZERO_F64;

        let mut iter = self.samples.iter();
        let mut previous = iter.next()?;
        for current in iter {
            let dt_ms = current.ts_ms.saturating_sub(previous.ts_ms);
            if dt_ms == 0 {
                previous = current;
                continue;
            }
            if !is_positive_finite(current.price) || !is_positive_finite(previous.price) {
                return None;
            }

            let log_return = (current.price / previous.price).ln();
            if !log_return.is_finite() {
                return None;
            }

            sum_squared_returns += log_return.powi(POWER_OF_TWO);
            elapsed_ms = elapsed_ms.saturating_add(dt_ms);
            observation_count += COUNTER_INCREMENT;
            previous = current;
        }

        if observation_count < min_observations || elapsed_ms == 0 {
            return None;
        }

        let elapsed_secs = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
        let annualized_variance = (sum_squared_returns / elapsed_secs) * SECONDS_PER_YEAR_F64;
        let vol = annualized_variance.sqrt();
        if is_positive_finite(vol) {
            Some(vol)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_WINDOW_SECS: u64 = 60;
    const TEST_GAP_RESET_SECS: u64 = 10;
    const TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES: u64 = 1;
    const TEST_MIN_OBSERVATIONS_READY_AFTER_FOUR_SAMPLES: u64 = 3;
    const TEST_BRIDGE_VALID_SECS: u64 = 10;
    const TEST_INITIAL_PRICE: f64 = 3_100.0;
    const TEST_SECOND_PRICE: f64 = 3_101.0;
    const TEST_THIRD_PRICE: f64 = 3_099.5;
    const TEST_FOURTH_PRICE: f64 = 3_102.0;
    const TEST_AFTER_GAP_PRICE: f64 = 3_103.0;
    const TEST_OUT_OF_ORDER_PRICE: f64 = 3_200.0;
    const TEST_INITIAL_TS_MS: u64 = 0;
    const TEST_FIRST_READY_INPUT_TS_MS: u64 = 1_000;
    const TEST_BACKWARD_QUERY_TS_MS: u64 = TEST_FIRST_READY_INPUT_TS_MS;
    const TEST_SECOND_READY_INPUT_TS_MS: u64 = 2_000;
    const TEST_READY_TS_MS: u64 = 3_000;
    const TEST_BRIDGED_QUERY_TS_MS: u64 = 12_000;
    const TEST_EXPIRED_BRIDGE_QUERY_TS_MS: u64 = 13_001;
    const TEST_AFTER_GAP_TS_MS: u64 = 20_000;
    const TEST_OUT_OF_ORDER_TS_MS: u64 = 1_500;

    fn estimator(
        window_secs: u64,
        gap_reset_secs: u64,
        min_observations: u64,
        bridge_valid_secs: u64,
    ) -> RealizedVolEstimator {
        RealizedVolEstimator::from_config(&RealizedVolConfig {
            window_secs,
            gap_reset_secs,
            min_observations,
            bridge_valid_secs,
        })
    }

    fn fixture_venue() -> &'static str {
        std::any::type_name::<RealizedVolEstimator>()
    }

    #[test]
    fn realized_vol_estimator_warms_bridges_and_resets_after_gap() {
        let mut estimator = estimator(
            TEST_WINDOW_SECS,
            TEST_GAP_RESET_SECS,
            TEST_MIN_OBSERVATIONS_READY_AFTER_FOUR_SAMPLES,
            TEST_BRIDGE_VALID_SECS,
        );

        assert!(
            estimator
                .observe(fixture_venue(), TEST_INITIAL_PRICE, TEST_INITIAL_TS_MS)
                .is_none()
        );
        assert!(
            estimator
                .observe(
                    fixture_venue(),
                    TEST_SECOND_PRICE,
                    TEST_FIRST_READY_INPUT_TS_MS,
                )
                .is_none()
        );
        assert!(
            estimator
                .observe(
                    fixture_venue(),
                    TEST_THIRD_PRICE,
                    TEST_SECOND_READY_INPUT_TS_MS,
                )
                .is_none()
        );
        let ready_vol = estimator
            .observe(fixture_venue(), TEST_FOURTH_PRICE, TEST_READY_TS_MS)
            .expect("vol should be ready after min observations");
        assert!(ready_vol > 0.0);
        assert_eq!(
            estimator.current_vol_at(TEST_BRIDGED_QUERY_TS_MS),
            Some(ready_vol)
        );
        assert!(
            estimator
                .current_vol_at(TEST_EXPIRED_BRIDGE_QUERY_TS_MS)
                .is_none()
        );

        assert!(
            estimator
                .observe(fixture_venue(), TEST_AFTER_GAP_PRICE, TEST_AFTER_GAP_TS_MS)
                .is_none()
        );
        assert_eq!(estimator.samples.len(), 1);
        assert!(estimator.last_ready_vol.is_none());
    }

    #[test]
    fn realized_vol_estimator_does_not_bridge_backwards_in_time() {
        let mut estimator = estimator(
            TEST_WINDOW_SECS,
            TEST_GAP_RESET_SECS,
            TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES,
            TEST_BRIDGE_VALID_SECS,
        );

        assert!(
            estimator
                .observe(
                    fixture_venue(),
                    TEST_INITIAL_PRICE,
                    TEST_FIRST_READY_INPUT_TS_MS,
                )
                .is_none()
        );
        let ready_vol = estimator
            .observe(
                fixture_venue(),
                TEST_SECOND_PRICE,
                TEST_SECOND_READY_INPUT_TS_MS,
            )
            .expect("vol should be ready after min observations");

        assert_eq!(
            estimator.current_vol_at(TEST_SECOND_READY_INPUT_TS_MS),
            Some(ready_vol)
        );
        assert_eq!(estimator.current_vol_at(TEST_BACKWARD_QUERY_TS_MS), None);
    }

    #[test]
    fn realized_vol_estimator_ignores_non_monotonic_samples_within_same_venue() {
        let mut estimator = estimator(
            TEST_WINDOW_SECS,
            TEST_GAP_RESET_SECS,
            TEST_MIN_OBSERVATIONS_READY_AFTER_TWO_SAMPLES,
            TEST_BRIDGE_VALID_SECS,
        );

        assert!(
            estimator
                .observe(
                    fixture_venue(),
                    TEST_INITIAL_PRICE,
                    TEST_FIRST_READY_INPUT_TS_MS,
                )
                .is_none()
        );
        let _ready_vol = estimator
            .observe(
                fixture_venue(),
                TEST_SECOND_PRICE,
                TEST_SECOND_READY_INPUT_TS_MS,
            )
            .expect("vol should be ready after min observations");
        let sample_count = estimator.samples.len();

        assert_eq!(
            estimator.observe(
                fixture_venue(),
                TEST_OUT_OF_ORDER_PRICE,
                TEST_OUT_OF_ORDER_TS_MS,
            ),
            None
        );
        assert_eq!(estimator.samples.len(), sample_count);
        assert_eq!(
            estimator.samples.back().map(|sample| sample.ts_ms),
            Some(TEST_SECOND_READY_INPUT_TS_MS)
        );
        assert_eq!(
            estimator.last_ready_ts_ms,
            Some(TEST_SECOND_READY_INPUT_TS_MS)
        );
    }
}
