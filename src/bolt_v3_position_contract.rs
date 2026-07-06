use nautilus_model::{
    enums::{OrderSide, PositionSide},
    instruments::InstrumentAny,
};

use crate::{
    bolt_v3_market_families::{OutcomeSide, updown::updown_position_instrument_context},
    bolt_v3_numeric::{MILLIS_PER_SECOND_U64, is_positive_finite},
};

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3PositionMarketLifecycle {
    market_id: Option<String>,
    outcome_side: Option<OutcomeSide>,
    strike_price: Option<f64>,
    interval_end_ms: Option<u64>,
    selection_published_at_ms: Option<u64>,
    seconds_to_expiry_at_selection: Option<u64>,
}

impl BoltV3PositionMarketLifecycle {
    pub fn missing() -> Self {
        Self {
            market_id: None,
            outcome_side: None,
            strike_price: None,
            interval_end_ms: None,
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: None,
        }
    }

    pub fn from_entry_context(
        market_id: Option<String>,
        outcome_side: Option<OutcomeSide>,
        strike_price: Option<f64>,
        interval_open: Option<f64>,
        interval_end_ms: Option<u64>,
        selection_published_at_ms: Option<u64>,
        seconds_to_expiry_at_selection: Option<u64>,
    ) -> Self {
        Self {
            market_id,
            outcome_side,
            strike_price: strike_price
                .or(interval_open)
                .filter(|value| is_positive_finite(*value)),
            interval_end_ms,
            selection_published_at_ms,
            seconds_to_expiry_at_selection,
        }
    }

    pub fn recover_from_instrument(instrument: Option<&InstrumentAny>) -> Self {
        let Some(instrument) = instrument else {
            return Self::missing();
        };
        let Some(context) = updown_position_instrument_context(instrument) else {
            return Self::missing();
        };
        let interval_end_ms = context.expiration_milliseconds;
        let selection_published_at_ms = Some(context.activation_milliseconds);
        let seconds_to_expiry_at_selection = selection_published_at_ms
            .zip(interval_end_ms)
            .and_then(|(selection_ms, end_ms)| {
                end_ms
                    .checked_sub(selection_ms)
                    .map(|duration_ms| duration_ms / MILLIS_PER_SECOND_U64)
            });
        Self {
            market_id: Some(context.market_id),
            outcome_side: Some(context.side),
            strike_price: None,
            interval_end_ms,
            selection_published_at_ms,
            seconds_to_expiry_at_selection,
        }
    }

    pub fn market_id(&self) -> Option<&str> {
        self.market_id.as_deref()
    }

    pub fn market_id_owned(&self) -> Option<String> {
        self.market_id.clone()
    }

    pub fn outcome_side(&self) -> Option<OutcomeSide> {
        self.outcome_side
    }

    pub fn settlement_strike(&self) -> Option<f64> {
        self.strike_price.filter(|value| is_positive_finite(*value))
    }

    pub fn interval_end_ms(&self) -> Option<u64> {
        self.interval_end_ms.or_else(|| {
            Self::interval_end_ms_from_selection(
                self.selection_published_at_ms,
                self.seconds_to_expiry_at_selection,
            )
        })
    }

    pub fn selection_published_at_ms(&self) -> Option<u64> {
        self.selection_published_at_ms
    }

    pub fn seconds_to_expiry_at_selection(&self) -> Option<u64> {
        self.seconds_to_expiry_at_selection
    }

    pub fn seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        Self::seconds_to_expiry_from_selection(
            self.selection_published_at_ms,
            self.seconds_to_expiry_at_selection,
            now_ms,
        )
        .or_else(|| {
            self.interval_end_ms().map(|interval_end_ms| {
                interval_end_ms.saturating_sub(now_ms) / MILLIS_PER_SECOND_U64
            })
        })
    }

    pub fn interval_ended_at(&self, now_ms: u64) -> bool {
        self.interval_end_ms()
            .is_some_and(|interval_end_ms| now_ms >= interval_end_ms)
    }

    pub fn matches_resolution_tick_ms(&self, resolution_ts_ms: u64) -> bool {
        self.interval_end_ms() == Some(resolution_ts_ms)
    }

    pub fn market_matches_or_missing(&self, market_id: Option<&str>) -> bool {
        self.market_id().is_none() || self.market_id() == market_id
    }

    pub fn fill_missing_from(&mut self, other: &Self) {
        if self.market_id.is_none() {
            self.market_id = other.market_id.clone();
        }
        if self.outcome_side.is_none() {
            self.outcome_side = other.outcome_side;
        }
        if self.strike_price.is_none() {
            self.strike_price = other.strike_price;
        }
        if self.interval_end_ms.is_none() {
            self.interval_end_ms = other.interval_end_ms;
        }
        if self.selection_published_at_ms.is_none() {
            self.selection_published_at_ms = other.selection_published_at_ms;
        }
        if self.seconds_to_expiry_at_selection.is_none() {
            self.seconds_to_expiry_at_selection = other.seconds_to_expiry_at_selection;
        }
    }

    pub fn without_outcome_side(mut self) -> Self {
        self.outcome_side = None;
        self
    }

    fn seconds_to_expiry_from_selection(
        selection_published_at_ms: Option<u64>,
        seconds_to_expiry_at_selection: Option<u64>,
        now_ms: u64,
    ) -> Option<u64> {
        let published_at_ms = selection_published_at_ms?;
        let seconds_to_expiry_at_selection = seconds_to_expiry_at_selection?;
        let elapsed_seconds = now_ms.saturating_sub(published_at_ms) / MILLIS_PER_SECOND_U64;
        Some(seconds_to_expiry_at_selection.saturating_sub(elapsed_seconds))
    }

    fn interval_end_ms_from_selection(
        selection_published_at_ms: Option<u64>,
        seconds_to_expiry_at_selection: Option<u64>,
    ) -> Option<u64> {
        selection_published_at_ms?
            .checked_add(seconds_to_expiry_at_selection?.checked_mul(MILLIS_PER_SECOND_U64)?)
    }
}

pub fn expected_position_side_for_entry_order(order_side: OrderSide) -> Option<PositionSide> {
    match order_side {
        OrderSide::Buy => Some(PositionSide::Long),
        OrderSide::Sell => Some(PositionSide::Short),
        _ => None,
    }
}

pub fn expected_exit_order_side_for_position(position_side: PositionSide) -> Option<OrderSide> {
    match position_side {
        PositionSide::Long => Some(OrderSide::Sell),
        PositionSide::Short => Some(OrderSide::Buy),
        _ => None,
    }
}

pub fn is_observed_open_side(side: PositionSide) -> bool {
    matches!(side, PositionSide::Long | PositionSide::Short)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{bolt_v3_market_families::OutcomeSide, bolt_v3_numeric::NANOS_PER_MILLI_U64};
    use nautilus_core::Params;
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{InstrumentId, Symbol},
        instruments::{BinaryOption, InstrumentAny},
        types::{Currency, Price, Quantity},
    };

    #[test]
    fn position_market_lifecycle_derives_recovered_updown_context_and_predicates() {
        let lifecycle =
            BoltV3PositionMarketLifecycle::recover_from_instrument(Some(&test_binary_option(
                "condition-UP.POLYMARKET",
                "configuredasset-updown-5m-600",
                "market-1",
                "condition-1",
                "question-1",
                "Up",
                600_000,
                900_000,
            )));

        assert_eq!(lifecycle.market_id(), Some("market-1"));
        assert_eq!(lifecycle.outcome_side(), Some(OutcomeSide::Up));
        assert_eq!(lifecycle.interval_end_ms(), Some(900_000));
        assert_eq!(lifecycle.seconds_to_expiry_at(750_000), Some(150));
        assert!(!lifecycle.interval_ended_at(899_999));
        assert!(lifecycle.interval_ended_at(900_000));
        assert!(lifecycle.matches_resolution_tick_ms(900_000));
        assert!(!lifecycle.matches_resolution_tick_ms(960_000));
        assert_eq!(lifecycle.settlement_strike(), None);
    }

    #[test]
    fn position_market_lifecycle_uses_entry_strike_and_selection_fallback() {
        let lifecycle = BoltV3PositionMarketLifecycle::from_entry_context(
            Some("market-2".to_string()),
            Some(OutcomeSide::Down),
            Some(3_100.0),
            None,
            None,
            Some(1_000),
            Some(300),
        );

        assert_eq!(lifecycle.market_id(), Some("market-2"));
        assert_eq!(lifecycle.outcome_side(), Some(OutcomeSide::Down));
        assert_eq!(lifecycle.settlement_strike(), Some(3_100.0));
        assert_eq!(lifecycle.interval_end_ms(), Some(301_000));
        assert_eq!(lifecycle.seconds_to_expiry_at(61_000), Some(240));
        assert!(lifecycle.matches_resolution_tick_ms(301_000));
    }

    #[allow(clippy::too_many_arguments)]
    fn test_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        condition_id: &str,
        question_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        info.insert(
            "market_slug".to_string(),
            serde_json::Value::String(market_slug.to_string()),
        );
        info.insert(
            "market_id".to_string(),
            serde_json::Value::String(market_id.to_string()),
        );
        info.insert(
            "condition_id".to_string(),
            serde_json::Value::String(condition_id.to_string()),
        );
        info.insert(
            "question_id".to_string(),
            serde_json::Value::String(question_id.to_string()),
        );
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            (expiration_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(info),
            1.into(),
            1.into(),
        ))
    }
}
