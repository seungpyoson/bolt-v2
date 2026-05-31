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
        // Look-ahead guard: a query for a time *before* the vol was computed must
        // not surface the future-derived value. `checked_sub` yields None when
        // `now_ms < last_ready_ts_ms`; the bridge only holds a ready vol forward,
        // never backward into the past.
        let age_ms = now_ms.checked_sub(last_ready_ts_ms)?;
        if age_ms <= self.bridge_valid_ms {
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

    #[test]
    fn realized_vol_estimator_warms_bridges_and_resets_after_gap() {
        let mut estimator = estimator(60, 10, 3, 10);

        assert!(estimator.observe("bybit", 3_100.0, 0).is_none());
        assert!(estimator.observe("bybit", 3_101.0, 1_000).is_none());
        assert!(estimator.observe("bybit", 3_099.5, 2_000).is_none());
        let ready_vol = estimator
            .observe("bybit", 3_102.0, 3_000)
            .expect("vol should be ready after min observations");
        assert!(ready_vol > 0.0);
        assert_eq!(estimator.current_vol_at(12_000), Some(ready_vol));
        assert!(estimator.current_vol_at(13_001).is_none());

        assert!(estimator.observe("bybit", 3_103.0, 20_000).is_none());
        assert_eq!(estimator.samples.len(), 1);
        assert!(estimator.last_ready_vol.is_none());
    }

    #[test]
    fn realized_vol_estimator_ignores_non_monotonic_samples_within_same_venue() {
        let mut estimator = estimator(60, 10, 1, 10);

        assert!(estimator.observe("bybit", 3_100.0, 1_000).is_none());
        assert!(
            estimator.observe("bybit", 3_101.0, 2_000).is_some(),
            "vol should be ready after min observations"
        );
        let sample_count = estimator.samples.len();

        // The non-monotonic sample is ignored, and the read for its earlier
        // timestamp must not leak the future-derived ready vol back into the past.
        assert!(estimator.observe("bybit", 3_200.0, 1_500).is_none());
        assert_eq!(estimator.samples.len(), sample_count);
        assert_eq!(
            estimator.samples.back().map(|sample| sample.ts_ms),
            Some(2_000)
        );
        assert_eq!(estimator.last_ready_ts_ms, Some(2_000));
    }

    #[test]
    fn current_vol_at_returns_none_for_query_before_last_ready_ts() {
        let mut estimator = estimator(60, 10, 1, 10);

        assert!(estimator.observe("bybit", 3_100.0, 1_000).is_none());
        let ready_vol = estimator
            .observe("bybit", 3_101.0, 2_000)
            .expect("vol should be ready after min observations");

        // Forward query within the bridge horizon still returns the ready vol.
        assert_eq!(estimator.current_vol_at(2_500), Some(ready_vol));
        // A query for a time *before* the vol was computed must not surface the
        // future-derived value — that would be look-ahead and would inflate the
        // backtest edge. The bridge only holds forward.
        assert!(estimator.current_vol_at(1_999).is_none());
    }
}
