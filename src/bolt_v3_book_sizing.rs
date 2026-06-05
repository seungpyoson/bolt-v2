use std::collections::BTreeMap;

use nautilus_model::{
    data::OrderBookDeltas,
    enums::{BookAction, OrderSide},
    identifiers::InstrumentId,
    types::Price,
};

use crate::bolt_v3_numeric::{BPS_DENOMINATOR, UNIT_F64, ZERO_F64, is_positive_finite};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutcomeBookState {
    pub(crate) instrument_id: Option<InstrumentId>,
    pub(crate) last_observed_instrument_id: Option<InstrumentId>,
    pub(crate) bid_levels: BTreeMap<Price, f64>,
    pub(crate) ask_levels: BTreeMap<Price, f64>,
    pub(crate) best_bid: Option<f64>,
    pub(crate) best_ask: Option<f64>,
    pub(crate) liquidity_available: Option<f64>,
}

impl OutcomeBookState {
    pub(crate) fn empty() -> Self {
        Self {
            instrument_id: None,
            last_observed_instrument_id: None,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            liquidity_available: None,
        }
    }

    pub(crate) fn from_instrument_id(instrument_id: InstrumentId) -> Self {
        Self {
            instrument_id: Some(instrument_id),
            last_observed_instrument_id: None,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            liquidity_available: None,
        }
    }

    pub(crate) fn is_priced(&self) -> bool {
        self.best_bid.is_some() && self.best_ask.is_some()
    }

    /// Whether this book is priced and strictly crossed (`best_bid > best_ask`).
    ///
    /// A locked book (`best_bid == best_ask`) is not crossed and is intentionally
    /// not flagged. An unpriced book (either side missing) is not crossed either;
    /// the entry gate already covers that case.
    pub(crate) fn is_crossed(&self) -> bool {
        matches!(
            (self.best_bid, self.best_ask),
            (Some(best_bid), Some(best_ask)) if best_bid > best_ask
        )
    }

    pub(crate) fn metadata_matches_selection(&self) -> bool {
        self.last_observed_instrument_id.is_some()
            && self.last_observed_instrument_id == self.instrument_id
    }

    pub(crate) fn update_from_deltas(&mut self, deltas: &OrderBookDeltas) {
        for delta in &deltas.deltas {
            let price = delta.order.price;
            let size = delta.order.size.as_f64();
            let levels = match delta.order.side {
                OrderSide::Buy => Some(&mut self.bid_levels),
                OrderSide::Sell => Some(&mut self.ask_levels),
                _ => None,
            };

            match delta.action {
                BookAction::Add | BookAction::Update => {
                    if let Some(levels) = levels {
                        if is_positive_finite(size) {
                            levels.insert(price, size);
                        } else {
                            levels.remove(&price);
                        }
                    }
                }
                BookAction::Delete => {
                    if let Some(levels) = levels {
                        levels.remove(&price);
                    }
                }
                BookAction::Clear => {
                    self.bid_levels.clear();
                    self.ask_levels.clear();
                }
            }
        }

        self.last_observed_instrument_id = Some(deltas.instrument_id);
        self.best_bid = self
            .bid_levels
            .last_key_value()
            .map(|(price, _)| price.as_f64());
        self.best_ask = self
            .ask_levels
            .first_key_value()
            .map(|(price, _)| price.as_f64());
        self.liquidity_available = Some(
            self.bid_levels.values().copied().sum::<f64>()
                + self.ask_levels.values().copied().sum::<f64>(),
        );
    }

    pub(crate) fn executable_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_ask,
            OrderSide::Sell => self.best_bid,
            _ => None,
        }
        .filter(|value| is_positive_finite(*value))
    }

    pub(crate) fn passive_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_bid,
            OrderSide::Sell => self.best_ask,
            _ => None,
        }
        .filter(|value| is_positive_finite(*value))
    }

    pub(crate) fn max_execution_within_vwap_slippage_bps(
        &self,
        order_side: OrderSide,
        slippage_bps: u64,
    ) -> Option<ImpactCappedExecution> {
        let slippage = slippage_bps as f64 / BPS_DENOMINATOR;
        match order_side {
            OrderSide::Buy => {
                let best_ask = self.executable_price_for_order_side(OrderSide::Buy)?;
                let allowed_vwap = best_ask * (UNIT_F64 + slippage);
                max_execution_within_vwap_limit(
                    self.ask_levels
                        .iter()
                        .map(|(price, size)| (price.as_f64(), *size)),
                    allowed_vwap,
                    true,
                )
            }
            OrderSide::Sell => {
                let best_bid = self.executable_price_for_order_side(OrderSide::Sell)?;
                let allowed_vwap = best_bid * (UNIT_F64 - slippage);
                max_execution_within_vwap_limit(
                    self.bid_levels
                        .iter()
                        .rev()
                        .map(|(price, size)| (price.as_f64(), *size)),
                    allowed_vwap,
                    false,
                )
            }
            _ => None,
        }
    }
}

fn max_execution_within_vwap_limit<I>(
    levels: I,
    allowed_vwap: f64,
    is_buy: bool,
) -> Option<ImpactCappedExecution>
where
    I: IntoIterator<Item = (f64, f64)>,
{
    if !is_positive_finite(allowed_vwap) {
        return None;
    }

    let mut cumulative_qty = ZERO_F64;
    let mut cumulative_notional = ZERO_F64;

    for (price, size) in levels {
        if !is_positive_finite(price) || !is_positive_finite(size) {
            continue;
        }

        let next_qty = cumulative_qty + size;
        let next_notional = cumulative_notional + price * size;
        let next_vwap = next_notional / next_qty;
        let within_limit = if is_buy {
            next_vwap <= allowed_vwap
        } else {
            next_vwap >= allowed_vwap
        };
        if within_limit {
            cumulative_qty = next_qty;
            cumulative_notional = next_notional;
            continue;
        }

        let partial_qty = if is_buy {
            let denominator = price - allowed_vwap;
            if denominator <= ZERO_F64 {
                size
            } else {
                ((allowed_vwap * cumulative_qty - cumulative_notional) / denominator)
                    .clamp(ZERO_F64, size)
            }
        } else {
            let denominator = allowed_vwap - price;
            if denominator <= ZERO_F64 {
                size
            } else {
                ((cumulative_notional - allowed_vwap * cumulative_qty) / denominator)
                    .clamp(ZERO_F64, size)
            }
        };

        let total_qty = cumulative_qty + partial_qty;
        let total_notional = cumulative_notional + partial_qty * price;
        return is_positive_finite(total_qty).then_some(ImpactCappedExecution {
            quantity: total_qty,
            vwap_price: total_notional / total_qty,
        });
    }

    is_positive_finite(cumulative_qty).then_some(ImpactCappedExecution {
        quantity: cumulative_qty,
        vwap_price: cumulative_notional / cumulative_qty,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ImpactCappedExecution {
    pub(crate) quantity: f64,
    pub(crate) vwap_price: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutcomePreparedBooks {
    pub(crate) up: OutcomeBookState,
    pub(crate) down: OutcomeBookState,
}

impl OutcomePreparedBooks {
    pub(crate) fn empty() -> Self {
        Self {
            up: OutcomeBookState::empty(),
            down: OutcomeBookState::empty(),
        }
    }

    pub(crate) fn is_priced(&self) -> bool {
        self.up.is_priced() && self.down.is_priced()
    }

    /// Whether either active outcome book is priced and strictly crossed.
    ///
    /// Mirrors how [`OutcomePreparedBooks::is_priced`] treats both outcome books
    /// as the active book for the entry gate: a cross on either side is an invalid
    /// market state that must block entry.
    pub(crate) fn any_crossed(&self) -> bool {
        self.up.is_crossed() || self.down.is_crossed()
    }

    pub(crate) fn metadata_matches_selection(&self) -> bool {
        self.up.metadata_matches_selection() && self.down.metadata_matches_selection()
    }

    pub(crate) fn minimum_liquidity(&self) -> Option<f64> {
        Some(
            self.up
                .liquidity_available?
                .min(self.down.liquidity_available?),
        )
    }

    pub(crate) fn update_from_deltas(&mut self, deltas: &OrderBookDeltas) -> bool {
        if self.up.instrument_id == Some(deltas.instrument_id) {
            self.up.update_from_deltas(deltas);
            true
        } else if self.down.instrument_id == Some(deltas.instrument_id) {
            self.down.update_from_deltas(deltas);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OutcomeBookSubscriptions {
    pub(crate) up_instrument_id: Option<InstrumentId>,
    pub(crate) down_instrument_id: Option<InstrumentId>,
    pub(crate) tracked_position_instrument_id: Option<InstrumentId>,
}

impl OutcomeBookSubscriptions {
    pub(crate) fn empty() -> Self {
        Self {
            up_instrument_id: None,
            down_instrument_id: None,
            tracked_position_instrument_id: None,
        }
    }

    pub(crate) fn is_same_market(&self, other: &Self) -> bool {
        self.up_instrument_id == other.up_instrument_id
            && self.down_instrument_id == other.down_instrument_id
            && self.tracked_position_instrument_id == other.tracked_position_instrument_id
    }
}

pub(crate) fn should_replace_book_subscriptions(
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) -> bool {
    !current.is_same_market(next)
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        data::{BookOrder, OrderBookDelta, OrderBookDeltas},
        enums::{BookAction, OrderSide},
        identifiers::InstrumentId,
        types::{Price, Quantity},
    };

    use super::{OutcomeBookState, max_execution_within_vwap_limit};

    fn book_deltas(
        instrument_id: InstrumentId,
        deltas: &[(BookAction, OrderSide, f64, f64)],
    ) -> OrderBookDeltas {
        let deltas = deltas
            .iter()
            .map(|(action, side, price, size)| {
                OrderBookDelta::new_checked(
                    instrument_id,
                    *action,
                    BookOrder::new(*side, Price::new(*price, 2), Quantity::new(*size, 2), 0),
                    0,
                    0,
                    0.into(),
                    0.into(),
                )
                .expect("test order book delta should build")
            })
            .collect();

        OrderBookDeltas::new(instrument_id, deltas)
    }

    #[test]
    fn book_state_updates_best_touch_and_vwap_cap_in_shared_module() {
        let instrument_id = InstrumentId::from("condition-A-A-UP.POLYMARKET");
        let mut state = OutcomeBookState::from_instrument_id(instrument_id);

        state.update_from_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Update, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Update, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Update, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        assert_eq!(state.best_bid, Some(0.49));
        assert_eq!(state.best_ask, Some(0.50));
        assert_eq!(state.liquidity_available, Some(30.0));

        let zero_bps = state
            .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 0)
            .expect("best-touch-only size should exist");
        let loose = state
            .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 5_000)
            .expect("full displayed size should exist");

        assert_eq!(zero_bps.quantity, 10.0);
        assert_eq!(loose.quantity, 20.0);
        assert!(loose.vwap_price > zero_bps.vwap_price);
    }

    #[test]
    fn vwap_limit_helper_accepts_single_pass_iterators_without_collecting() {
        let levels = [(0.50, 10.0), (0.60, 10.0)]
            .into_iter()
            .filter(|(_, size)| *size > 0.0);

        let execution = max_execution_within_vwap_limit(levels, 0.55, true)
            .expect("partial execution should fit at the buy-side VWAP limit");

        assert_eq!(execution.quantity, 20.0);
        assert_eq!(execution.vwap_price, 0.55);
    }
}
