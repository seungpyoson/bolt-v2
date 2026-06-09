#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_model::{
        enums::OrderSide,
        identifiers::InstrumentId,
        types::Price,
    };

    use crate::{
        bolt_v3_book_sizing::OutcomeBookState,
        bolt_v3_market_families::OutcomeSide,
    };

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
                - result.edge_cents_per_share
                    / result.cost_breakdown.total_adjusted_cost_cents
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
