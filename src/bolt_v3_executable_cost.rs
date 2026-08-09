use std::collections::BTreeMap;

use nautilus_model::{enums::OrderSide, types::Price};

use crate::bolt_v3_numeric::{
    BPS_DENOMINATOR, CENTS_PER_SHARE, UNIT_F64, ZERO_F64, is_non_negative_finite,
    is_positive_finite, notional_float_tolerance,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutableCostBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    FeeUnavailable,
    InvalidCost,
    UnsupportedOrderShape,
}

impl ExecutableCostBlockReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingOrderBook => "missing_order_book",
            Self::InsufficientDepth => "insufficient_depth",
            Self::FeeUnavailable => "fee_unavailable",
            Self::InvalidCost => "invalid_cost",
            Self::UnsupportedOrderShape => "unsupported_order_shape",
        }
    }
}

impl std::fmt::Display for ExecutableCostBlockReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Debug for ExecutableCostBlockReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ExactSizeVwap {
    pub(crate) vwap_price: f64,
    pub(crate) vwap_quantity: f64,
    pub(crate) limit_price: f64,
    pub(crate) exact_size_filled: bool,
    pub(crate) fill_legs: Vec<ExecutableFillLeg>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecutableFillLeg {
    pub(crate) price: f64,
    pub(crate) quantity: f64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExecutableBookQuote<'a> {
    pub(crate) best_bid: Option<f64>,
    pub(crate) best_ask: Option<f64>,
    pub(crate) bid_levels: &'a BTreeMap<Price, f64>,
    pub(crate) ask_levels: &'a BTreeMap<Price, f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecutableCostBreakdown {
    pub(crate) vwap_price: Option<f64>,
    pub(crate) vwap_quantity: Option<f64>,
    pub(crate) limit_price: Option<f64>,
    pub(crate) exact_size_filled: bool,
    pub(crate) gross_cost_cents: f64,
    pub(crate) fee_cost_cents: f64,
    pub(crate) slippage_buffer_cents: f64,
    pub(crate) total_adjusted_cost_cents: f64,
    pub(crate) cost_available: bool,
    pub(crate) block_reason: Option<ExecutableCostBlockReason>,
}

impl ExecutableCostBreakdown {
    pub(crate) fn blocked(reason: ExecutableCostBlockReason) -> Self {
        Self {
            vwap_price: None,
            vwap_quantity: None,
            limit_price: None,
            exact_size_filled: false,
            gross_cost_cents: ZERO_F64,
            fee_cost_cents: ZERO_F64,
            slippage_buffer_cents: ZERO_F64,
            total_adjusted_cost_cents: ZERO_F64,
            cost_available: false,
            block_reason: Some(reason),
        }
    }
}

pub(crate) fn price_exact_size_vwap(
    book: &ExecutableBookQuote<'_>,
    order_side: OrderSide,
    edge_pricing_notional: f64,
    vwap_depth_limit_bps: u64,
) -> Result<ExactSizeVwap, ExecutableCostBlockReason> {
    if !is_positive_finite(edge_pricing_notional) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }

    let depth_limit = vwap_depth_limit_bps as f64 / BPS_DENOMINATOR;
    let (best_touch, is_buy) = match order_side {
        OrderSide::Buy => (book.best_ask, true),
        OrderSide::Sell => (book.best_bid, false),
        _ => return Err(ExecutableCostBlockReason::UnsupportedOrderShape),
    };
    let best_touch = best_touch
        .filter(|value| is_positive_finite(*value))
        .ok_or(ExecutableCostBlockReason::MissingOrderBook)?;
    let allowed_vwap = if is_buy {
        best_touch * (UNIT_F64 + depth_limit)
    } else {
        best_touch * (UNIT_F64 - depth_limit)
    };
    if !is_positive_finite(allowed_vwap) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }

    let mut remaining_notional = edge_pricing_notional;
    let mut filled_quantity = ZERO_F64;
    let mut filled_notional = ZERO_F64;
    let mut limit_price = None;
    let mut fill_legs = Vec::new();

    match order_side {
        OrderSide::Buy => {
            for (price, size) in book.ask_levels {
                let price = price.as_f64();
                if price > allowed_vwap {
                    break;
                }
                let previous_remaining_notional = remaining_notional;
                let previous_filled_quantity = filled_quantity;
                consume_exact_notional_level(
                    price,
                    *size,
                    &mut remaining_notional,
                    &mut filled_quantity,
                    &mut filled_notional,
                )?;
                if remaining_notional < previous_remaining_notional {
                    limit_price = Some(price);
                    fill_legs.push(ExecutableFillLeg {
                        price,
                        quantity: filled_quantity - previous_filled_quantity,
                    });
                }
                if remaining_notional <= ZERO_F64 {
                    break;
                }
            }
        }
        OrderSide::Sell => {
            for (price, size) in book.bid_levels.iter().rev() {
                let price = price.as_f64();
                if price < allowed_vwap {
                    break;
                }
                let previous_remaining_notional = remaining_notional;
                let previous_filled_quantity = filled_quantity;
                consume_exact_notional_level(
                    price,
                    *size,
                    &mut remaining_notional,
                    &mut filled_quantity,
                    &mut filled_notional,
                )?;
                if remaining_notional < previous_remaining_notional {
                    limit_price = Some(price);
                    fill_legs.push(ExecutableFillLeg {
                        price,
                        quantity: filled_quantity - previous_filled_quantity,
                    });
                }
                if remaining_notional <= ZERO_F64 {
                    break;
                }
            }
        }
        _ => return Err(ExecutableCostBlockReason::UnsupportedOrderShape),
    }

    if remaining_notional > notional_float_tolerance(edge_pricing_notional)
        || !is_positive_finite(filled_quantity)
    {
        return Err(ExecutableCostBlockReason::InsufficientDepth);
    }
    let vwap_price = filled_notional / filled_quantity;
    if !is_positive_finite(vwap_price) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    let limit_price = limit_price
        .filter(|value| is_positive_finite(*value))
        .ok_or(ExecutableCostBlockReason::InvalidCost)?;
    let within_depth_limit = if is_buy {
        vwap_price <= allowed_vwap && limit_price <= allowed_vwap
    } else {
        vwap_price >= allowed_vwap && limit_price >= allowed_vwap
    };
    if !within_depth_limit {
        return Err(ExecutableCostBlockReason::InsufficientDepth);
    }

    Ok(ExactSizeVwap {
        vwap_price,
        vwap_quantity: filled_quantity,
        limit_price,
        exact_size_filled: true,
        fill_legs,
    })
}

pub(crate) fn price_exact_quantity_vwap(
    book: &ExecutableBookQuote<'_>,
    order_side: OrderSide,
    requested_quantity: f64,
    vwap_depth_limit_bps: u64,
) -> Result<ExactSizeVwap, ExecutableCostBlockReason> {
    if !is_positive_finite(requested_quantity) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    let depth_limit = vwap_depth_limit_bps as f64 / BPS_DENOMINATOR;
    let (best_touch, is_buy) = match order_side {
        OrderSide::Buy => (book.best_ask, true),
        OrderSide::Sell => (book.best_bid, false),
        _ => return Err(ExecutableCostBlockReason::UnsupportedOrderShape),
    };
    let best_touch = best_touch
        .filter(|value| is_positive_finite(*value))
        .ok_or(ExecutableCostBlockReason::MissingOrderBook)?;
    let allowed_limit = if is_buy {
        best_touch * (UNIT_F64 + depth_limit)
    } else {
        best_touch * (UNIT_F64 - depth_limit)
    };
    if !is_positive_finite(allowed_limit) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }

    let mut remaining_quantity = requested_quantity;
    let mut filled_quantity = ZERO_F64;
    let mut filled_notional = ZERO_F64;
    let mut fill_legs = Vec::new();
    match order_side {
        OrderSide::Buy => {
            for (price, available_quantity) in book.ask_levels {
                let price = price.as_f64();
                if price > allowed_limit {
                    break;
                }
                consume_exact_quantity_level(
                    price,
                    *available_quantity,
                    &mut remaining_quantity,
                    &mut filled_quantity,
                    &mut filled_notional,
                    &mut fill_legs,
                )?;
                if remaining_quantity <= ZERO_F64 {
                    break;
                }
            }
        }
        OrderSide::Sell => {
            for (price, available_quantity) in book.bid_levels.iter().rev() {
                let price = price.as_f64();
                if price < allowed_limit {
                    break;
                }
                consume_exact_quantity_level(
                    price,
                    *available_quantity,
                    &mut remaining_quantity,
                    &mut filled_quantity,
                    &mut filled_notional,
                    &mut fill_legs,
                )?;
                if remaining_quantity <= ZERO_F64 {
                    break;
                }
            }
        }
        _ => return Err(ExecutableCostBlockReason::UnsupportedOrderShape),
    }
    if remaining_quantity > notional_float_tolerance(requested_quantity)
        || !is_positive_finite(filled_quantity)
    {
        return Err(ExecutableCostBlockReason::InsufficientDepth);
    }
    let vwap_price = filled_notional / filled_quantity;
    let limit_price = fill_legs
        .last()
        .map(|leg| leg.price)
        .filter(|price| is_positive_finite(*price))
        .ok_or(ExecutableCostBlockReason::InvalidCost)?;
    if !is_positive_finite(vwap_price) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    Ok(ExactSizeVwap {
        vwap_price,
        vwap_quantity: filled_quantity,
        limit_price,
        exact_size_filled: true,
        fill_legs,
    })
}

fn consume_exact_quantity_level(
    price: f64,
    available_quantity: f64,
    remaining_quantity: &mut f64,
    filled_quantity: &mut f64,
    filled_notional: &mut f64,
    fill_legs: &mut Vec<ExecutableFillLeg>,
) -> Result<(), ExecutableCostBlockReason> {
    if !is_positive_finite(price) || !is_positive_finite(available_quantity) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    let quantity = remaining_quantity.min(available_quantity);
    let notional = price * quantity;
    if !is_positive_finite(quantity) || !is_positive_finite(notional) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    *remaining_quantity -= quantity;
    *filled_quantity += quantity;
    *filled_notional += notional;
    fill_legs.push(ExecutableFillLeg { price, quantity });
    Ok(())
}

fn consume_exact_notional_level(
    price: f64,
    size: f64,
    remaining_notional: &mut f64,
    filled_quantity: &mut f64,
    filled_notional: &mut f64,
) -> Result<(), ExecutableCostBlockReason> {
    if *remaining_notional <= ZERO_F64 {
        return Ok(());
    }
    if !is_positive_finite(price) || !is_positive_finite(size) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    let level_notional = price * size;
    if !is_positive_finite(level_notional) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }
    let take_notional = remaining_notional.min(level_notional);
    let take_quantity = take_notional / price;
    *filled_quantity += take_quantity;
    *filled_notional += take_notional;
    *remaining_notional -= take_notional;
    Ok(())
}

pub(crate) fn executable_cost_breakdown(
    vwap: &ExactSizeVwap,
    fee_bps: f64,
    slippage_buffer_bps: u64,
) -> Result<ExecutableCostBreakdown, ExecutableCostBlockReason> {
    if !is_non_negative_finite(fee_bps) {
        return Err(ExecutableCostBlockReason::FeeUnavailable);
    }

    let gross_cost_cents = vwap.vwap_price * CENTS_PER_SHARE;
    let fee_cost_cents = gross_cost_cents * fee_bps / BPS_DENOMINATOR;
    let slippage_buffer_cents = gross_cost_cents * slippage_buffer_bps as f64 / BPS_DENOMINATOR;
    let total_adjusted_cost_cents = gross_cost_cents + fee_cost_cents + slippage_buffer_cents;
    if !is_positive_finite(gross_cost_cents) || !is_positive_finite(total_adjusted_cost_cents) {
        return Err(ExecutableCostBlockReason::InvalidCost);
    }

    Ok(ExecutableCostBreakdown {
        vwap_price: Some(vwap.vwap_price),
        vwap_quantity: Some(vwap.vwap_quantity),
        limit_price: Some(vwap.limit_price),
        exact_size_filled: vwap.exact_size_filled,
        gross_cost_cents,
        fee_cost_cents,
        slippage_buffer_cents,
        total_adjusted_cost_cents,
        cost_available: true,
        block_reason: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nautilus_model::{enums::OrderSide, types::Price};

    const EPSILON: f64 = 1e-9;

    struct TestBook {
        bid_levels: BTreeMap<Price, f64>,
        ask_levels: BTreeMap<Price, f64>,
        best_bid: Option<f64>,
        best_ask: Option<f64>,
    }

    impl TestBook {
        fn quote(&self) -> super::ExecutableBookQuote<'_> {
            super::ExecutableBookQuote {
                best_bid: self.best_bid,
                best_ask: self.best_ask,
                bid_levels: &self.bid_levels,
                ask_levels: &self.ask_levels,
            }
        }
    }

    fn priced_book_with_levels(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> TestBook {
        let mut bid_levels = BTreeMap::new();
        for (price, size) in bids {
            bid_levels.insert(Price::new(*price, 2), *size);
        }
        let mut ask_levels = BTreeMap::new();
        for (price, size) in asks {
            ask_levels.insert(Price::new(*price, 2), *size);
        }
        TestBook {
            bid_levels,
            ask_levels,
            best_bid: bids.iter().map(|(price, _)| *price).max_by(f64::total_cmp),
            best_ask: asks.iter().map(|(price, _)| *price).min_by(f64::total_cmp),
        }
    }

    fn priced_book(asks: &[(f64, f64)]) -> TestBook {
        priced_book_with_levels(&[(0.40, 100.0)], asks)
    }

    #[test]
    fn generic_book_quote_prices_without_outcome_book_state() {
        let book = priced_book(&[(0.50, 5.0), (0.60, 100.0)]);

        let priced = super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, 2.5, 2_000)
            .expect("generic quote should price without a strategy book type");

        assert_eq!(priced.limit_price, 0.50);
        assert!(priced.exact_size_filled);
    }

    #[test]
    fn exact_size_buy_vwap_prices_requested_notional_across_levels() {
        let levels = [(0.50, 5.0), (0.60, 100.0)];
        let [
            (best_level_price, best_level_quantity),
            (second_level_price, _),
        ] = levels;
        let requested_notional = best_level_price * best_level_quantity * levels.len() as f64;
        let book = priced_book(&levels);

        let priced =
            super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, requested_notional, 2_000)
                .expect("exact notional should fill inside the depth limit");

        let best_level_notional = best_level_price * best_level_quantity;
        let second_level_notional = requested_notional - best_level_notional;
        let expected_vwap_quantity =
            best_level_quantity + (second_level_notional / second_level_price);
        let expected_vwap_price = requested_notional / expected_vwap_quantity;

        assert!((priced.vwap_price - expected_vwap_price).abs() < EPSILON);
        assert!((priced.vwap_quantity - expected_vwap_quantity).abs() < EPSILON);
        assert_eq!(priced.limit_price, second_level_price);
        assert!(priced.exact_size_filled);
        assert_eq!(priced.fill_legs.len(), 2);
        assert_eq!(priced.fill_legs[0].price, best_level_price);
        assert_eq!(priced.fill_legs[0].quantity, best_level_quantity);
        assert_eq!(priced.fill_legs[1].price, second_level_price);
        assert!(
            (priced.fill_legs[1].quantity - (second_level_notional / second_level_price)).abs()
                < EPSILON
        );
    }

    #[test]
    fn exact_size_sell_vwap_prices_requested_notional_across_bid_levels() {
        let levels = [(0.60, 5.0), (0.50, 100.0)];
        let [
            (best_level_price, best_level_quantity),
            (second_level_price, _),
        ] = levels;
        let requested_notional = best_level_price * best_level_quantity * levels.len() as f64;
        let book = priced_book_with_levels(&levels, &[(0.70, 100.0)]);

        let priced =
            super::price_exact_size_vwap(&book.quote(), OrderSide::Sell, requested_notional, 2_000)
                .expect("sell exact notional should fill inside the depth limit");

        let best_level_notional = best_level_price * best_level_quantity;
        let second_level_notional = requested_notional - best_level_notional;
        let expected_vwap_quantity =
            best_level_quantity + (second_level_notional / second_level_price);
        let expected_vwap_price = requested_notional / expected_vwap_quantity;

        assert!((priced.vwap_price - expected_vwap_price).abs() < EPSILON);
        assert!((priced.vwap_quantity - expected_vwap_quantity).abs() < EPSILON);
        assert_eq!(priced.limit_price, second_level_price);
        assert!(priced.exact_size_filled);
    }

    #[test]
    fn exact_quantity_sell_preserves_every_consumed_bid_level() {
        let book = priced_book_with_levels(&[(0.60, 5.0), (0.50, 100.0)], &[(0.70, 100.0)]);

        let priced = super::price_exact_quantity_vwap(&book.quote(), OrderSide::Sell, 10.0, 2_000)
            .expect("exact exit quantity should fill across visible bid levels");

        assert_eq!(priced.limit_price, 0.50);
        assert_eq!(priced.vwap_quantity, 10.0);
        assert!((priced.vwap_price - 0.55).abs() < EPSILON);
        assert_eq!(
            priced.fill_legs,
            vec![
                super::ExecutableFillLeg {
                    price: 0.60,
                    quantity: 5.0,
                },
                super::ExecutableFillLeg {
                    price: 0.50,
                    quantity: 5.0,
                },
            ]
        );
    }

    #[test]
    fn exact_quantity_sweep_rejects_incomplete_visible_depth() {
        let book = priced_book_with_levels(&[(0.60, 5.0)], &[(0.70, 100.0)]);

        let reason = super::price_exact_quantity_vwap(&book.quote(), OrderSide::Sell, 10.0, 2_000)
            .expect_err("an exit without complete planned levels must fail closed");

        assert_eq!(reason, super::ExecutableCostBlockReason::InsufficientDepth);
    }

    #[test]
    fn executable_cost_breakdown_applies_fee_and_slippage_to_vwap_cost() {
        let vwap = super::ExactSizeVwap {
            vwap_price: 0.50,
            vwap_quantity: 10.0,
            limit_price: 0.50,
            exact_size_filled: true,
            fill_legs: Vec::new(),
        };

        let breakdown =
            super::executable_cost_breakdown(&vwap, 100.0, 200).expect("cost should be valid");

        assert_eq!(breakdown.vwap_price, Some(0.50));
        assert_eq!(breakdown.vwap_quantity, Some(10.0));
        assert_eq!(breakdown.limit_price, Some(0.50));
        assert!(breakdown.exact_size_filled);
        assert!((breakdown.gross_cost_cents - 50.0).abs() < EPSILON);
        assert!((breakdown.fee_cost_cents - 0.5).abs() < EPSILON);
        assert!((breakdown.slippage_buffer_cents - 1.0).abs() < EPSILON);
        assert!((breakdown.total_adjusted_cost_cents - 51.5).abs() < EPSILON);
        assert!(breakdown.cost_available);
        assert_eq!(breakdown.block_reason, None);
    }

    #[test]
    fn executable_cost_breakdown_blocks_invalid_fee_bps() {
        let vwap = super::ExactSizeVwap {
            vwap_price: 0.50,
            vwap_quantity: 10.0,
            limit_price: 0.50,
            exact_size_filled: true,
            fill_legs: Vec::new(),
        };

        for fee_bps in [f64::NAN, -1.0, f64::INFINITY] {
            let reason = super::executable_cost_breakdown(&vwap, fee_bps, 0)
                .expect_err("invalid fee inputs must fail closed");

            assert_eq!(reason, super::ExecutableCostBlockReason::FeeUnavailable);
        }
    }

    #[test]
    fn executable_cost_breakdown_pins_multi_level_partial_fill_arithmetic() {
        let book = priced_book(&[(0.50, 5.0), (0.60, 100.0)]);
        let vwap = super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, 5.0, 2_000)
            .expect("exact notional should fill inside the depth limit");

        let breakdown =
            super::executable_cost_breakdown(&vwap, 100.0, 100).expect("cost should be valid");

        let expected_vwap_price = 6.0 / 11.0;
        let expected_gross_cost_cents = 600.0 / 11.0;
        let expected_fee_cost_cents = 6.0 / 11.0;
        let expected_slippage_buffer_cents = 6.0 / 11.0;
        let expected_total_adjusted_cost_cents = 612.0 / 11.0;
        assert!(
            breakdown
                .vwap_price
                .is_some_and(|value| (value - expected_vwap_price).abs() < EPSILON)
        );
        assert!((breakdown.gross_cost_cents - expected_gross_cost_cents).abs() < EPSILON);
        assert!((breakdown.fee_cost_cents - expected_fee_cost_cents).abs() < EPSILON);
        assert!((breakdown.slippage_buffer_cents - expected_slippage_buffer_cents).abs() < EPSILON);
        assert!(
            (breakdown.total_adjusted_cost_cents - expected_total_adjusted_cost_cents).abs()
                < EPSILON
        );
    }

    #[test]
    fn exact_size_vwap_blocks_insufficient_depth() {
        let book = priced_book(&[(0.50, 5.0)]);

        let reason = super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, 5.0, 2_000)
            .expect_err("not enough allowed depth must fail closed");

        assert_eq!(reason, super::ExecutableCostBlockReason::InsufficientDepth);
    }

    #[test]
    fn exact_size_vwap_blocks_when_buy_limit_price_exceeds_depth_limit() {
        let book = priced_book(&[(0.50, 5.0), (0.60, 100.0)]);

        let reason = super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, 5.0, 1_000)
            .expect_err("last consumed buy level outside the depth limit must fail closed");

        assert_eq!(reason, super::ExecutableCostBlockReason::InsufficientDepth);
    }

    #[test]
    fn exact_size_vwap_blocks_when_sell_limit_price_exceeds_depth_limit() {
        let book = priced_book_with_levels(&[(0.50, 100.0), (0.12, 100.0)], &[(1.0, 100.0)]);

        let reason = super::price_exact_size_vwap(&book.quote(), OrderSide::Sell, 100.0, 5_000)
            .expect_err("last consumed sell level outside the depth limit must fail closed");

        assert_eq!(reason, super::ExecutableCostBlockReason::InsufficientDepth);
    }

    #[test]
    fn exact_size_vwap_blocks_corrupt_visible_book_level() {
        let book = priced_book(&[(0.50, 0.0), (0.51, 100.0)]);

        let reason = super::price_exact_size_vwap(&book.quote(), OrderSide::Buy, 5.0, 2_000)
            .expect_err("corrupt visible liquidity must fail closed instead of being skipped");

        assert_eq!(reason, super::ExecutableCostBlockReason::InvalidCost);
    }
}
