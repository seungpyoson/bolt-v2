use std::collections::VecDeque;

use nautilus_model::{data::TradeTick, enums::AggressorSide};

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
/// Plain runtime config view without a serde derive: strategy modules own TOML
/// deserialization and project the relevant fields here at the call site, so
/// the buffer never depends on a strategy config type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedTradeFlowConfig {
    pub window_secs: u64,
    pub max_samples: u64,
}

impl SignedTradeFlowConfig {
    /// The retention window in milliseconds, or `None` when `window_secs × 1000`
    /// overflows `u64`. The single home of the seconds→milliseconds conversion:
    /// [`SignedTradeFlow::from_config`] builds the buffer window from it, and a
    /// strategy's go-live gate calls it to reject (fail loud) a `window_secs` so
    /// large the window would silently saturate instead of meaning the configured
    /// value.
    pub fn window_ms(&self) -> Option<u64> {
        self.window_secs.checked_mul(MILLIS_PER_SECOND_U64)
    }
}

/// Bounded rolling buffer of signed trades for a single quoted instrument.
///
/// Mirrors the config-driven rolling-window shape used by other shared V3
/// helpers: the retention window and hard sample cap both come from a projected
/// [`SignedTradeFlowConfig`], and the buffer is bounded by time (`window_ms`) and
/// count (`max_samples`). It only retains signed trade flow; downstream adverse
/// selection stages read it to compute signal state.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedTradeFlow {
    window_ms: u64,
    max_samples: usize,
    last_observed_ns: Option<u64>,
    samples: VecDeque<SignedTrade>,
}

impl SignedTradeFlow {
    pub(crate) fn from_config(config: &SignedTradeFlowConfig) -> Self {
        Self {
            // Overflow is rejected loud upstream at the strategy go-live gate via
            // `SignedTradeFlowConfig::window_ms`, so for any validated config this
            // is the exact configured window; the `u64::MAX` floor only guards a
            // direct, unvalidated construction and never silently truncates one.
            window_ms: config.window_ms().unwrap_or(u64::MAX),
            max_samples: config.max_samples as usize,
            last_observed_ns: None,
            samples: VecDeque::new(),
        }
    }

    pub(crate) fn observe(&mut self, trade: &TradeTick) {
        let ts_ns = trade.ts_event.as_u64();
        let ts_ms = ts_ns / NANOS_PER_MILLI_U64;
        // Reject non-monotonic observations at nanosecond resolution so
        // distinct trades within one millisecond are retained without allowing
        // equal or older observations to corrupt oldest-first ordering.
        if self
            .last_observed_ns
            .is_some_and(|previous| ts_ns <= previous)
        {
            return;
        }
        self.last_observed_ns = Some(ts_ns);
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
            .filter(move |trade| trade.ts_ms >= cutoff_ms && trade.ts_ms <= now_ms)
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

    const TEST_IDENTIFIER_TOKEN_LIMIT: usize = 16;
    const TEST_TRADE_PRICE_PRECISION: u8 = 2;
    const TEST_TRADE_SIZE_PRECISION: u8 = u8::MIN;
    const TEST_WIDE_TRADE_FLOW_WINDOW_SECS: u64 = 600;
    const TEST_NARROW_TRADE_FLOW_WINDOW_SECS: u64 = 10;
    const TEST_TRADE_FLOW_MAX_SAMPLES: u64 = 100;
    const TEST_TRADE_SIZE: f64 = 1.0;
    const TEST_FIRST_APPENDED_TRADE_SIZE: f64 = 3.0;
    const TEST_SECOND_APPENDED_TRADE_SIZE: f64 = 2.0;
    const TEST_FIRST_APPENDED_TRADE_PRICE: f64 = 0.42;
    const TEST_SECOND_APPENDED_TRADE_PRICE: f64 = 0.41;
    const TEST_THIRD_APPENDED_TRADE_PRICE: f64 = 0.40;
    const TEST_FIRST_SAME_MS_TRADE_PRICE: f64 = 0.50;
    const TEST_SECOND_SAME_MS_TRADE_PRICE: f64 = 0.51;
    const TEST_REJECTED_EQUAL_NS_PRICE: f64 = 0.52;
    const TEST_REJECTED_OLDER_NS_PRICE: f64 = 0.53;
    const TEST_FUTURE_WINDOW_PAST_PRICE: f64 = 0.54;
    const TEST_FUTURE_WINDOW_FUTURE_PRICE: f64 = 0.55;
    const TEST_DISABLED_WINDOW_RETAINED_PRICE: f64 = 0.56;
    const TEST_DISABLED_WINDOW_REPLACEMENT_PRICE: f64 = 0.57;
    const TEST_DISABLED_SAMPLE_PRICE: f64 = 0.58;
    const TEST_FIRST_SAME_MS_TRADE_NS: u64 = NANOS_PER_MILLI_U64 + 1;
    const TEST_SECOND_SAME_MS_TRADE_NS: u64 = TEST_FIRST_SAME_MS_TRADE_NS + 1;
    const TEST_EQUAL_NS_WATERMARK: u64 = TEST_SECOND_SAME_MS_TRADE_NS;
    const TEST_OLDER_NS_THAN_WATERMARK: u64 = TEST_EQUAL_NS_WATERMARK - 1;
    const TEST_SAME_MS_SAMPLE_TS_MS: u64 = TEST_FIRST_SAME_MS_TRADE_NS / NANOS_PER_MILLI_U64;
    const TEST_PAST_SAMPLE_TS_MS: u64 = 5_000;
    const TEST_NOW_TS_MS: u64 = 12_000;
    const TEST_FUTURE_SAMPLE_TS_MS: u64 = 15_000;
    const TEST_DISABLED_CONFIG_VALUE: u64 = u64::MIN;
    const TEST_DISABLED_WINDOW_FIRST_TS_MS: u64 = 1_000;
    const TEST_DISABLED_WINDOW_SECOND_TS_MS: u64 = TEST_DISABLED_WINDOW_FIRST_TS_MS + 1;
    const TEST_DISABLED_SAMPLE_TS_MS: u64 = 2_000;
    const TEST_FIRST_APPENDED_TRADE_TS_MS: u64 = 1_000;
    const TEST_SECOND_APPENDED_TRADE_TS_MS: u64 = 1_500;
    const TEST_THIRD_APPENDED_TRADE_TS_MS: u64 = 2_000;
    const TEST_OUT_OF_ORDER_TS_MS: u64 = 1_500;
    const TEST_DUPLICATE_TS_MS: u64 = 2_000;
    const TEST_WINDOW_FIRST_TS_MS: u64 = 1_000;
    const TEST_WINDOW_SECOND_TS_MS: u64 = 5_000;
    const TEST_WINDOW_LATEST_TS_MS: u64 = 15_000;
    const TEST_AGED_OUT_QUERY_TS_MS: u64 = 20_000;
    const TEST_CAP_MAX_SAMPLES: u64 = 2;
    const TEST_CAP_SOURCE_SAMPLE_COUNT: u64 = 4;
    const TEST_CAP_FIRST_TS_MS: u64 = 1_000;
    const TEST_CAP_FIRST_RETAINED_OFFSET: u64 = 2;
    const TEST_CAP_SECOND_RETAINED_OFFSET: u64 = 3;

    fn signed_trade_flow_config(window_secs: u64, max_samples: u64) -> SignedTradeFlowConfig {
        SignedTradeFlowConfig {
            window_secs,
            max_samples,
        }
    }

    fn generated_trade_flow_instrument_id() -> String {
        format!(
            "{}.{}",
            test_identifier_token(std::any::type_name::<SignedTradeFlow>()).to_ascii_uppercase(),
            test_identifier_token(std::any::type_name::<SignedTradeFlowConfig>())
                .to_ascii_uppercase(),
        )
    }

    fn trade_tick_with_aggressor(
        instrument_id: &str,
        price: f64,
        size: f64,
        aggressor: AggressorSide,
        ts_ms: u64,
    ) -> TradeTick {
        trade_tick_with_aggressor_ns(
            instrument_id,
            price,
            size,
            aggressor,
            ts_ms.saturating_mul(NANOS_PER_MILLI_U64),
        )
    }

    fn trade_tick_with_aggressor_ns(
        instrument_id: &str,
        price: f64,
        size: f64,
        aggressor: AggressorSide,
        ts_ns: u64,
    ) -> TradeTick {
        let trade_id = format!(
            "{}{ts_ns}",
            test_identifier_token(std::any::type_name::<TradeTick>())
        );
        TradeTick::new_checked(
            InstrumentId::from(instrument_id),
            Price::new(price, TEST_TRADE_PRICE_PRECISION),
            Quantity::new(size, TEST_TRADE_SIZE_PRECISION),
            aggressor,
            TradeId::from(trade_id.as_str()),
            UnixNanos::from(ts_ns),
            UnixNanos::from(ts_ns),
        )
        .expect("test trade tick should be valid")
    }

    fn test_identifier_token(raw: &str) -> String {
        raw.chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .take(TEST_IDENTIFIER_TOKEN_LIMIT)
            .collect()
    }

    #[test]
    fn signed_trade_flow_observe_appends_signed_price_and_size() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_NARROW_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FIRST_APPENDED_TRADE_PRICE,
            TEST_FIRST_APPENDED_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_FIRST_APPENDED_TRADE_TS_MS,
        ));
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_SECOND_APPENDED_TRADE_PRICE,
            TEST_SECOND_APPENDED_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_SECOND_APPENDED_TRADE_TS_MS,
        ));
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_THIRD_APPENDED_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::NoAggressor,
            TEST_THIRD_APPENDED_TRADE_TS_MS,
        ));

        assert_eq!(flow.len(), 3);
        assert!(!flow.is_empty());
        let samples: Vec<SignedTrade> = flow.samples().iter().copied().collect();
        // Prices/sizes are compared through the same fixed-point round-trip the
        // production path uses, so the test does not depend on literal f64 bits.
        assert_eq!(
            samples,
            vec![
                SignedTrade {
                    ts_ms: TEST_FIRST_APPENDED_TRADE_TS_MS,
                    aggressor: AggressorSide::Buyer,
                    price: Price::new(TEST_FIRST_APPENDED_TRADE_PRICE, TEST_TRADE_PRICE_PRECISION)
                        .as_f64(),
                    size: Quantity::new(TEST_FIRST_APPENDED_TRADE_SIZE, TEST_TRADE_SIZE_PRECISION)
                        .as_f64(),
                },
                SignedTrade {
                    ts_ms: TEST_SECOND_APPENDED_TRADE_TS_MS,
                    aggressor: AggressorSide::Seller,
                    price: Price::new(TEST_SECOND_APPENDED_TRADE_PRICE, TEST_TRADE_PRICE_PRECISION)
                        .as_f64(),
                    size: Quantity::new(TEST_SECOND_APPENDED_TRADE_SIZE, TEST_TRADE_SIZE_PRECISION)
                        .as_f64(),
                },
                SignedTrade {
                    ts_ms: TEST_THIRD_APPENDED_TRADE_TS_MS,
                    aggressor: AggressorSide::NoAggressor,
                    price: Price::new(TEST_THIRD_APPENDED_TRADE_PRICE, TEST_TRADE_PRICE_PRECISION)
                        .as_f64(),
                    size: Quantity::new(TEST_TRADE_SIZE, TEST_TRADE_SIZE_PRECISION).as_f64(),
                },
            ]
        );
    }

    #[test]
    fn signed_trade_flow_drops_out_of_order_and_duplicate_timestamps() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_WIDE_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FIRST_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_FIRST_APPENDED_TRADE_TS_MS,
        ));
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_SECOND_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_THIRD_APPENDED_TRADE_TS_MS,
        ));
        // Out-of-order: an earlier timestamp than the latest retained sample.
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_REJECTED_EQUAL_NS_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_OUT_OF_ORDER_TS_MS,
        ));
        // Duplicate: equal to the latest retained timestamp.
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_REJECTED_OLDER_NS_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_DUPLICATE_TS_MS,
        ));

        assert_eq!(flow.len(), 2);
        assert_eq!(
            flow.samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![
                TEST_FIRST_APPENDED_TRADE_TS_MS,
                TEST_THIRD_APPENDED_TRADE_TS_MS
            ],
            "out-of-order and duplicate-timestamp trades must be dropped to keep \
             the buffer monotonic"
        );
    }

    #[test]
    fn signed_trade_flow_retains_same_millisecond_distinct_trades() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_WIDE_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor_ns(
            instrument_id.as_str(),
            TEST_FIRST_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_FIRST_SAME_MS_TRADE_NS,
        ));
        flow.observe(&trade_tick_with_aggressor_ns(
            instrument_id.as_str(),
            TEST_SECOND_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_SECOND_SAME_MS_TRADE_NS,
        ));

        assert_eq!(flow.len(), 2);
        assert_eq!(
            flow.samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![TEST_SAME_MS_SAMPLE_TS_MS, TEST_SAME_MS_SAMPLE_TS_MS]
        );
    }

    #[test]
    fn signed_trade_flow_rejects_equal_and_older_nanosecond_observations() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_WIDE_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor_ns(
            instrument_id.as_str(),
            TEST_SECOND_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_EQUAL_NS_WATERMARK,
        ));
        flow.observe(&trade_tick_with_aggressor_ns(
            instrument_id.as_str(),
            TEST_REJECTED_EQUAL_NS_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_EQUAL_NS_WATERMARK,
        ));
        flow.observe(&trade_tick_with_aggressor_ns(
            instrument_id.as_str(),
            TEST_REJECTED_OLDER_NS_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_OLDER_NS_THAN_WATERMARK,
        ));

        assert_eq!(flow.len(), 1);
        assert_eq!(
            flow.samples().front().map(|trade| trade.price),
            Some(Price::new(TEST_SECOND_SAME_MS_TRADE_PRICE, TEST_TRADE_PRICE_PRECISION).as_f64())
        );
    }

    #[test]
    fn signed_trade_flow_samples_within_excludes_trades_aged_out_by_caller_clock() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_NARROW_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FIRST_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_WINDOW_FIRST_TS_MS,
        ));
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_SECOND_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_WINDOW_SECOND_TS_MS,
        ));

        // Observe-time eviction at the second trade retains both; the raw
        // buffer is not filtered by the caller's clock.
        assert_eq!(flow.len(), 2);

        // As of now, both trades have aged out, so a point-in-time read reports
        // none.
        assert!(
            flow.samples_within(TEST_AGED_OUT_QUERY_TS_MS)
                .next()
                .is_none()
        );

        // At an earlier caller clock, the later trade is in-window and the
        // first has aged out.
        assert_eq!(
            flow.samples_within(TEST_NOW_TS_MS)
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![TEST_WINDOW_SECOND_TS_MS]
        );
    }

    #[test]
    fn signed_trade_flow_samples_within_excludes_future_trades() {
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_NARROW_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));
        let instrument_id = generated_trade_flow_instrument_id();

        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FUTURE_WINDOW_PAST_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_PAST_SAMPLE_TS_MS,
        ));
        flow.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FUTURE_WINDOW_FUTURE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_FUTURE_SAMPLE_TS_MS,
        ));

        assert_eq!(
            flow.samples_within(TEST_NOW_TS_MS)
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![TEST_PAST_SAMPLE_TS_MS]
        );
    }

    #[test]
    fn signed_trade_flow_evicts_by_window_then_caps_by_max_samples() {
        let instrument_id = generated_trade_flow_instrument_id();
        let mut windowed = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_NARROW_TRADE_FLOW_WINDOW_SECS,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));

        windowed.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_FIRST_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_WINDOW_FIRST_TS_MS,
        ));
        windowed.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_SECOND_SAME_MS_TRADE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_WINDOW_SECOND_TS_MS,
        ));
        // Latest trade makes the window cutoff equal to the second trade; the
        // first sample is strictly older than the cutoff and is evicted.
        windowed.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_REJECTED_EQUAL_NS_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_WINDOW_LATEST_TS_MS,
        ));

        assert_eq!(windowed.len(), 2);
        assert_eq!(
            windowed
                .samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![TEST_WINDOW_SECOND_TS_MS, TEST_WINDOW_LATEST_TS_MS]
        );

        let mut capped = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_WIDE_TRADE_FLOW_WINDOW_SECS,
            TEST_CAP_MAX_SAMPLES,
        ));

        for index in u64::MIN..TEST_CAP_SOURCE_SAMPLE_COUNT {
            capped.observe(&trade_tick_with_aggressor(
                instrument_id.as_str(),
                TEST_FIRST_SAME_MS_TRADE_PRICE,
                TEST_TRADE_SIZE,
                AggressorSide::Buyer,
                TEST_CAP_FIRST_TS_MS + index,
            ));
        }

        assert_eq!(capped.len(), TEST_CAP_MAX_SAMPLES as usize);
        assert_eq!(
            capped
                .samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![
                TEST_CAP_FIRST_TS_MS + TEST_CAP_FIRST_RETAINED_OFFSET,
                TEST_CAP_FIRST_TS_MS + TEST_CAP_SECOND_RETAINED_OFFSET,
            ]
        );
    }

    #[test]
    fn signed_trade_flow_zero_config_preserves_current_disabled_behavior() {
        let instrument_id = generated_trade_flow_instrument_id();

        let mut zero_window = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_DISABLED_CONFIG_VALUE,
            TEST_TRADE_FLOW_MAX_SAMPLES,
        ));

        zero_window.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_DISABLED_WINDOW_RETAINED_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_DISABLED_WINDOW_FIRST_TS_MS,
        ));
        zero_window.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_DISABLED_WINDOW_REPLACEMENT_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Seller,
            TEST_DISABLED_WINDOW_SECOND_TS_MS,
        ));

        assert_eq!(
            zero_window
                .samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![TEST_DISABLED_WINDOW_SECOND_TS_MS],
            "zero-second windows retain only samples at the latest observe timestamp",
        );

        let mut zero_cap = SignedTradeFlow::from_config(&signed_trade_flow_config(
            TEST_WIDE_TRADE_FLOW_WINDOW_SECS,
            TEST_DISABLED_CONFIG_VALUE,
        ));

        zero_cap.observe(&trade_tick_with_aggressor(
            instrument_id.as_str(),
            TEST_DISABLED_SAMPLE_PRICE,
            TEST_TRADE_SIZE,
            AggressorSide::Buyer,
            TEST_DISABLED_SAMPLE_TS_MS,
        ));

        assert!(
            zero_cap.is_empty(),
            "zero max-samples disables retention after observe-time eviction",
        );
    }

    #[test]
    fn signed_trade_flow_config_window_ms_is_checked() {
        assert_eq!(
            signed_trade_flow_config(
                TEST_NARROW_TRADE_FLOW_WINDOW_SECS,
                TEST_TRADE_FLOW_MAX_SAMPLES
            )
            .window_ms(),
            Some(TEST_NARROW_TRADE_FLOW_WINDOW_SECS * MILLIS_PER_SECOND_U64)
        );
        assert_eq!(
            signed_trade_flow_config(u64::MAX, TEST_TRADE_FLOW_MAX_SAMPLES).window_ms(),
            None,
            "a window_secs whose millisecond conversion overflows must report None, \
             not silently saturate",
        );
    }
}
