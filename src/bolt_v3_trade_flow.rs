//! Bounded signed-trade-flow buffer shared across binary strategies.
//!
//! Hoisted out of [`crate::strategies::binary_oracle_edge_taker`] so the taker
//! and the binary maker reuse one buffer instead of each owning a copy (NO DUAL
//! PATHS). The taker re-exports these types from their former definition site so
//! existing references resolve unchanged; the W3 Glosten-Milgrom / VPIN stage is
//! the buffer's read consumer.

use std::collections::VecDeque;

use nautilus_model::data::TradeTick;
use nautilus_model::enums::AggressorSide;

use crate::bolt_v3_numeric::{MILLIS_PER_SECOND_U64, NANOS_PER_MILLI_U64};

/// A single signed trade retained for downstream adverse-selection / VPIN analysis.
///
/// This is the per-trade element stored by [`SignedTradeFlow`]; the signed
/// aggressor side, price, and size are the inputs the W3 Glosten-Milgrom / VPIN
/// stage will read. Fields are public because this struct is the read seam for
/// that later stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignedTrade {
    pub ts_ms: u64,
    pub aggressor: AggressorSide,
    pub price: f64,
    pub size: f64,
}

/// Runtime view of the trade-flow buffer's window/cap knobs.
///
/// Plain runtime config view without a serde derive: the strategy owns TOML
/// deserialization and projects the relevant fields here at the call site
/// (mirroring [`RealizedVolConfig`](crate::bolt_v3_volatility::RealizedVolConfig)
/// for the volatility estimator), so the buffer never depends on the strategy's
/// config type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedTradeFlowConfig {
    pub window_secs: u64,
    pub max_samples: u64,
}

/// Bounded rolling buffer of signed trades for a single quoted instrument.
///
/// Mirrors
/// [`RealizedVolEstimator`](crate::bolt_v3_volatility::RealizedVolEstimator)'s
/// config-driven rolling-window shape: the retention window and hard sample cap
/// both come from a projected [`SignedTradeFlowConfig`], and the buffer is
/// bounded by time (`window_ms`) and count (`max_samples`). It only retains
/// signed trade flow; the W3 stage reads it to compute adverse-selection signals.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedTradeFlow {
    window_ms: u64,
    max_samples: usize,
    samples: VecDeque<SignedTrade>,
}

impl SignedTradeFlow {
    pub(crate) fn from_config(config: &SignedTradeFlowConfig) -> Self {
        Self {
            window_ms: config.window_secs.saturating_mul(MILLIS_PER_SECOND_U64),
            max_samples: config.max_samples as usize,
            samples: VecDeque::new(),
        }
    }

    pub(crate) fn observe(&mut self, trade: &TradeTick) {
        let ts_ms = trade.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        // Reject out-of-order observations, mirroring `RealizedVolEstimator`: a
        // timestamp before the latest retained sample would break the
        // oldest-first ordering and the time-window eviction cutoff.
        if self
            .samples
            .back()
            .is_some_and(|previous| ts_ms < previous.ts_ms)
        {
            return;
        }
        self.samples.push_back(SignedTrade {
            ts_ms,
            aggressor: trade.aggressor_side,
            price: trade.price.as_f64(),
            size: trade.size.as_f64(),
        });
        self.evict(ts_ms);
    }

    fn evict(&mut self, now_ms: u64) {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        while self
            .samples
            .front()
            .is_some_and(|trade| trade.ts_ms < cutoff_ms)
        {
            let _ = self.samples.pop_front();
        }
        while self.samples.len() > self.max_samples {
            let _ = self.samples.pop_front();
        }
    }

    /// Number of signed trades currently retained. Read seam for the W3 stage.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the buffer currently holds no retained trades.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Retained signed trades, oldest first. Read seam for the W3 stage.
    ///
    /// These are evicted only as of the last [`observe`](Self::observe); in a
    /// quiet market some may have aged out of the window. A point-in-time
    /// consumer should read through [`samples_within`](Self::samples_within).
    pub fn samples(&self) -> &VecDeque<SignedTrade> {
        &self.samples
    }

    /// Signed trades still inside the retention window as of `now_ms`, oldest
    /// first. Unlike [`samples`](Self::samples) this filters against the caller's
    /// clock rather than the last `observe` timestamp, so a quiet market does not
    /// surface trades that have aged out of the window. Read seam for the W3
    /// stage's point-in-time adverse-selection reads.
    pub fn samples_within(&self, now_ms: u64) -> impl Iterator<Item = &SignedTrade> {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        self.samples
            .iter()
            .filter(move |trade| trade.ts_ms >= cutoff_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        identifiers::{InstrumentId, TradeId},
        types::{Price, Quantity},
    };

    #[test]
    fn retains_same_millisecond_trades_but_rejects_older_observations() {
        let mut flow = SignedTradeFlow::from_config(&SignedTradeFlowConfig {
            window_secs: 10,
            max_samples: 10,
        });

        flow.observe(&trade_tick("TRADE-001", 1_000, AggressorSide::Buyer));
        flow.observe(&trade_tick("TRADE-002", 1_000, AggressorSide::Seller));
        flow.observe(&trade_tick("TRADE-003", 999, AggressorSide::NoAggressor));

        let samples = flow.samples().iter().collect::<Vec<_>>();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].aggressor, AggressorSide::Buyer);
        assert_eq!(samples[1].aggressor, AggressorSide::Seller);
    }

    fn trade_tick(trade_id: &str, ts_ms: u64, aggressor: AggressorSide) -> TradeTick {
        TradeTick::new_checked(
            InstrumentId::from("SAME-MS.BOLT"),
            Price::new(1.0, 2),
            Quantity::new(1.0, 0),
            aggressor,
            TradeId::from(trade_id),
            UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
            UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        )
        .expect("test trade tick should be valid")
    }
}
