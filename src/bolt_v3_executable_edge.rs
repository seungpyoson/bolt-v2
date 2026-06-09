use nautilus_model::enums::OrderSide;

use crate::{
    bolt_v3_book_sizing::OutcomeBookState,
    bolt_v3_market_families::OutcomeSide,
    bolt_v3_numeric::{
        BPS_DENOMINATOR, UNIT_F64, ZERO_F64, is_non_negative_finite, is_positive_finite,
        sanitize_probability,
    },
};

const CENTS_PER_SHARE: f64 = 100.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExecutableEdgeBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    FeeUnavailable,
    InvalidProbability,
    InvalidCost,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
}

impl ExecutableEdgeBlockReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingOrderBook => "missing_order_book",
            Self::InsufficientDepth => "insufficient_depth",
            Self::FeeUnavailable => "fee_unavailable",
            Self::InvalidProbability => "invalid_probability",
            Self::InvalidCost => "invalid_cost",
            Self::EdgeBelowThreshold => "edge_below_threshold",
            Self::SpreadOrSlippageWipedEdge => "spread_or_slippage_wiped_edge",
        }
    }
}

impl std::fmt::Display for ExecutableEdgeBlockReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecutableEdgeInputs<'a> {
    pub(crate) side: OutcomeSide,
    pub(crate) fair_probability_up: Option<f64>,
    pub(crate) adjusted_probability_up: Option<f64>,
    pub(crate) edge_pricing_notional: f64,
    pub(crate) order_side: OrderSide,
    pub(crate) book: Option<&'a OutcomeBookState>,
    pub(crate) fee_bps: Option<f64>,
    pub(crate) vwap_depth_limit_bps: u64,
    pub(crate) slippage_buffer_bps: u64,
    pub(crate) minimum_edge_bps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExactSizeVwap {
    pub(crate) vwap_price: f64,
    pub(crate) vwap_quantity: f64,
    pub(crate) exact_size_filled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecutableCostBreakdown {
    pub(crate) vwap_price: Option<f64>,
    pub(crate) vwap_quantity: Option<f64>,
    pub(crate) exact_size_filled: bool,
    pub(crate) gross_cost_cents: f64,
    pub(crate) fee_cost_cents: f64,
    pub(crate) slippage_buffer_cents: f64,
    pub(crate) total_adjusted_cost_cents: f64,
    pub(crate) cost_available: bool,
    pub(crate) block_reason: Option<ExecutableEdgeBlockReason>,
}

impl ExecutableCostBreakdown {
    fn blocked(reason: ExecutableEdgeBlockReason) -> Self {
        Self {
            vwap_price: None,
            vwap_quantity: None,
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ExecutableEdgeResult {
    pub(crate) selected_side: OutcomeSide,
    pub(crate) adjusted_probability: f64,
    pub(crate) edge_bps: f64,
    pub(crate) edge_cents_per_share: f64,
    pub(crate) cost_breakdown: ExecutableCostBreakdown,
    pub(crate) trade_allowed: bool,
    pub(crate) block_reason: Option<ExecutableEdgeBlockReason>,
}

impl ExecutableEdgeResult {
    pub(crate) fn blocked(side: OutcomeSide, reason: ExecutableEdgeBlockReason) -> Self {
        Self {
            selected_side: side,
            adjusted_probability: ZERO_F64,
            edge_bps: ZERO_F64,
            edge_cents_per_share: ZERO_F64,
            cost_breakdown: ExecutableCostBreakdown::blocked(reason),
            trade_allowed: false,
            block_reason: Some(reason),
        }
    }
}

pub(crate) fn price_exact_size_vwap(
    book: &OutcomeBookState,
    order_side: OrderSide,
    edge_pricing_notional: f64,
    vwap_depth_limit_bps: u64,
) -> Result<ExactSizeVwap, ExecutableEdgeBlockReason> {
    if !is_positive_finite(edge_pricing_notional) {
        return Err(ExecutableEdgeBlockReason::InvalidCost);
    }

    let depth_limit = vwap_depth_limit_bps as f64 / BPS_DENOMINATOR;
    let (best_touch, is_buy) = match order_side {
        OrderSide::Buy => (book.best_ask, true),
        OrderSide::Sell => (book.best_bid, false),
        _ => return Err(ExecutableEdgeBlockReason::MissingOrderBook),
    };
    let best_touch = best_touch
        .filter(|value| is_positive_finite(*value))
        .ok_or(ExecutableEdgeBlockReason::MissingOrderBook)?;
    let allowed_vwap = if is_buy {
        best_touch * (UNIT_F64 + depth_limit)
    } else {
        best_touch * (UNIT_F64 - depth_limit)
    };
    if !is_positive_finite(allowed_vwap) {
        return Err(ExecutableEdgeBlockReason::InvalidCost);
    }

    let mut remaining_notional = edge_pricing_notional;
    let mut filled_quantity = ZERO_F64;
    let mut filled_notional = ZERO_F64;

    match order_side {
        OrderSide::Buy => {
            for (price, size) in &book.ask_levels {
                consume_exact_notional_level(
                    price.as_f64(),
                    *size,
                    &mut remaining_notional,
                    &mut filled_quantity,
                    &mut filled_notional,
                );
                if remaining_notional <= ZERO_F64 {
                    break;
                }
            }
        }
        OrderSide::Sell => {
            for (price, size) in book.bid_levels.iter().rev() {
                consume_exact_notional_level(
                    price.as_f64(),
                    *size,
                    &mut remaining_notional,
                    &mut filled_quantity,
                    &mut filled_notional,
                );
                if remaining_notional <= ZERO_F64 {
                    break;
                }
            }
        }
        _ => return Err(ExecutableEdgeBlockReason::MissingOrderBook),
    }

    if remaining_notional > f64::EPSILON || !is_positive_finite(filled_quantity) {
        return Err(ExecutableEdgeBlockReason::InsufficientDepth);
    }
    let vwap_price = filled_notional / filled_quantity;
    if !is_positive_finite(vwap_price) {
        return Err(ExecutableEdgeBlockReason::InvalidCost);
    }
    let within_limit = if is_buy {
        vwap_price <= allowed_vwap
    } else {
        vwap_price >= allowed_vwap
    };
    if !within_limit {
        return Err(ExecutableEdgeBlockReason::InsufficientDepth);
    }

    Ok(ExactSizeVwap {
        vwap_price,
        vwap_quantity: filled_quantity,
        exact_size_filled: true,
    })
}

fn consume_exact_notional_level(
    price: f64,
    size: f64,
    remaining_notional: &mut f64,
    filled_quantity: &mut f64,
    filled_notional: &mut f64,
) {
    if !is_positive_finite(price) || !is_positive_finite(size) || *remaining_notional <= ZERO_F64 {
        return;
    }
    let level_notional = price * size;
    if !is_positive_finite(level_notional) {
        return;
    }
    let take_notional = remaining_notional.min(level_notional);
    let take_quantity = take_notional / price;
    *filled_quantity += take_quantity;
    *filled_notional += take_notional;
    *remaining_notional -= take_notional;
}

pub(crate) fn evaluate_executable_edge(inputs: &ExecutableEdgeInputs<'_>) -> ExecutableEdgeResult {
    let Some(adjusted_probability_up) = inputs
        .adjusted_probability_up
        .and_then(sanitize_probability)
    else {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::InvalidProbability,
        );
    };
    if inputs
        .fair_probability_up
        .and_then(sanitize_probability)
        .is_none()
    {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::InvalidProbability,
        );
    }
    let Some(book) = inputs.book else {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::MissingOrderBook,
        );
    };
    let vwap = match price_exact_size_vwap(
        book,
        inputs.order_side,
        inputs.edge_pricing_notional,
        inputs.vwap_depth_limit_bps,
    ) {
        Ok(vwap) => vwap,
        Err(reason) => return ExecutableEdgeResult::blocked(inputs.side, reason),
    };
    let Some(fee_bps) = inputs.fee_bps else {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::FeeUnavailable,
        );
    };
    if !is_non_negative_finite(fee_bps) {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::FeeUnavailable,
        );
    }

    let success_probability = match inputs.side {
        OutcomeSide::Up => adjusted_probability_up,
        OutcomeSide::Down => UNIT_F64 - adjusted_probability_up,
    };
    if !is_non_negative_finite(success_probability) || success_probability > UNIT_F64 {
        return ExecutableEdgeResult::blocked(
            inputs.side,
            ExecutableEdgeBlockReason::InvalidProbability,
        );
    }

    let gross_cost_cents = vwap.vwap_price * CENTS_PER_SHARE;
    let fee_cost_cents = gross_cost_cents * fee_bps / BPS_DENOMINATOR;
    let slippage_buffer_cents =
        gross_cost_cents * inputs.slippage_buffer_bps as f64 / BPS_DENOMINATOR;
    let total_adjusted_cost_cents = gross_cost_cents + fee_cost_cents + slippage_buffer_cents;
    if !is_positive_finite(gross_cost_cents) || !is_positive_finite(total_adjusted_cost_cents) {
        return ExecutableEdgeResult::blocked(inputs.side, ExecutableEdgeBlockReason::InvalidCost);
    }

    let gross_edge_cents_per_share = success_probability * CENTS_PER_SHARE - gross_cost_cents;
    let edge_cents_per_share = success_probability * CENTS_PER_SHARE - total_adjusted_cost_cents;
    let edge_bps = edge_cents_per_share / total_adjusted_cost_cents * BPS_DENOMINATOR;
    if !edge_bps.is_finite() || !edge_cents_per_share.is_finite() {
        return ExecutableEdgeResult::blocked(inputs.side, ExecutableEdgeBlockReason::InvalidCost);
    }

    let mut block_reason = None;
    if gross_edge_cents_per_share > ZERO_F64 && edge_cents_per_share <= ZERO_F64 {
        block_reason = Some(ExecutableEdgeBlockReason::SpreadOrSlippageWipedEdge);
    } else if !inputs.minimum_edge_bps.is_finite() || edge_bps <= inputs.minimum_edge_bps {
        block_reason = Some(ExecutableEdgeBlockReason::EdgeBelowThreshold);
    }

    let cost_breakdown = ExecutableCostBreakdown {
        vwap_price: Some(vwap.vwap_price),
        vwap_quantity: Some(vwap.vwap_quantity),
        exact_size_filled: vwap.exact_size_filled,
        gross_cost_cents,
        fee_cost_cents,
        slippage_buffer_cents,
        total_adjusted_cost_cents,
        cost_available: true,
        block_reason,
    };
    ExecutableEdgeResult {
        selected_side: inputs.side,
        adjusted_probability: success_probability,
        edge_bps,
        edge_cents_per_share,
        cost_breakdown,
        trade_allowed: block_reason.is_none(),
        block_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_model::{enums::OrderSide, identifiers::InstrumentId, types::Price};

    use crate::{bolt_v3_book_sizing::OutcomeBookState, bolt_v3_market_families::OutcomeSide};

    const EPSILON: f64 = 1e-9;

    fn priced_book(asks: &[(f64, f64)]) -> OutcomeBookState {
        let mut book = OutcomeBookState::from_instrument_id(InstrumentId::from("EDGE.TEST"));
        book.bid_levels.insert(Price::new(0.40, 2), 100.0);
        book.best_bid = Some(0.40);
        for (price, size) in asks {
            book.ask_levels.insert(Price::new(*price, 2), *size);
        }
        book.best_ask = asks.first().map(|(price, _)| *price);
        book.liquidity_available = Some(
            book.bid_levels.values().copied().sum::<f64>()
                + book.ask_levels.values().copied().sum::<f64>(),
        );
        book
    }

    fn inputs<'a>(
        side: OutcomeSide,
        book: Option<&'a OutcomeBookState>,
    ) -> ExecutableEdgeInputs<'a> {
        ExecutableEdgeInputs {
            side,
            fair_probability_up: Some(0.60),
            adjusted_probability_up: Some(0.60),
            edge_pricing_notional: 5.0,
            order_side: OrderSide::Buy,
            book,
            fee_bps: Some(0.0),
            vwap_depth_limit_bps: 0,
            slippage_buffer_bps: 0,
            minimum_edge_bps: 0.0,
        }
    }

    #[test]
    fn exact_size_vwap_prices_requested_notional_across_levels() {
        let book = priced_book(&[(0.50, 5.0), (0.60, 100.0)]);

        let priced = price_exact_size_vwap(&book, OrderSide::Buy, 5.0, 1_000)
            .expect("exact notional should fill inside the depth limit");

        assert!((priced.vwap_price - 0.5454545454545454).abs() < EPSILON);
        assert!((priced.vwap_quantity - 9.166666666666668).abs() < EPSILON);
        assert!(priced.exact_size_filled);
    }

    #[test]
    fn fee_wipes_out_otherwise_positive_edge() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.adjusted_probability_up = Some(0.505);
        inputs.fee_bps = Some(200.0);

        let result = evaluate_executable_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::SpreadOrSlippageWipedEdge)
        );
        assert!(!result.trade_allowed);
        assert_eq!(result.cost_breakdown.vwap_price, Some(0.50));
        assert!((result.cost_breakdown.fee_cost_cents - 1.0).abs() < EPSILON);
        assert!((result.edge_cents_per_share + 0.5).abs() < EPSILON);
    }

    #[test]
    fn slippage_buffer_wipes_out_otherwise_positive_edge() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.adjusted_probability_up = Some(0.505);
        inputs.slippage_buffer_bps = 200;

        let result = evaluate_executable_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::SpreadOrSlippageWipedEdge)
        );
        assert!(!result.trade_allowed);
        assert!((result.cost_breakdown.slippage_buffer_cents - 1.0).abs() < EPSILON);
        assert!((result.edge_cents_per_share + 0.5).abs() < EPSILON);
    }

    #[test]
    fn no_book_blocks_trade() {
        let result = evaluate_executable_edge(&inputs(OutcomeSide::Up, None));

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::MissingOrderBook)
        );
        assert_eq!(
            result.cost_breakdown.block_reason,
            Some(ExecutableEdgeBlockReason::MissingOrderBook)
        );
        assert!(!result.cost_breakdown.cost_available);
        assert!(!result.trade_allowed);
    }

    #[test]
    fn insufficient_exact_size_depth_blocks_trade() {
        let book = priced_book(&[(0.50, 2.0)]);

        let result = evaluate_executable_edge(&inputs(OutcomeSide::Up, Some(&book)));

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::InsufficientDepth)
        );
        assert!(!result.cost_breakdown.cost_available);
        assert!(!result.trade_allowed);
    }

    #[test]
    fn missing_fee_blocks_trade_with_fee_unavailable() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.fee_bps = None;

        let result = evaluate_executable_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::FeeUnavailable)
        );
        assert!(!result.trade_allowed);
    }

    #[test]
    fn invalid_probability_blocks_trade_before_cost_math() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.adjusted_probability_up = Some(f64::NAN);

        let result = evaluate_executable_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::InvalidProbability)
        );
        assert!(!result.trade_allowed);
    }

    #[test]
    fn cents_per_share_and_bps_outputs_share_one_formula() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.fee_bps = Some(100.0);
        inputs.slippage_buffer_bps = 100;

        let result = evaluate_executable_edge(&inputs);

        assert!(result.trade_allowed);
        assert!((result.cost_breakdown.total_adjusted_cost_cents - 51.0).abs() < EPSILON);
        assert!((result.edge_cents_per_share - 9.0).abs() < EPSILON);
        assert!(
            (result.edge_bps
                - result.edge_cents_per_share / result.cost_breakdown.total_adjusted_cost_cents
                    * 10_000.0)
                .abs()
                < EPSILON
        );
    }

    #[test]
    fn invalid_total_cost_blocks_before_division() {
        let book = priced_book(&[(0.50, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Up, Some(&book));
        inputs.edge_pricing_notional = 0.0;

        let result = evaluate_executable_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(ExecutableEdgeBlockReason::InvalidCost)
        );
        assert_eq!(result.edge_bps, 0.0);
        assert!(!result.trade_allowed);
    }

    #[test]
    fn down_no_uses_one_minus_adjusted_probability_up() {
        let book = priced_book(&[(0.40, 100.0)]);
        let mut inputs = inputs(OutcomeSide::Down, Some(&book));
        inputs.adjusted_probability_up = Some(0.30);

        let result = evaluate_executable_edge(&inputs);

        assert!(result.trade_allowed);
        assert!((result.adjusted_probability - 0.70).abs() < EPSILON);
        assert!((result.edge_cents_per_share - 30.0).abs() < EPSILON);
    }
}
