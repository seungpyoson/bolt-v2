use nautilus_model::enums::OrderSide;

use crate::{
    bolt_v3_executable_cost::{ExecutableCostBlockReason, ExecutableCostBreakdown},
    bolt_v3_market_families::OutcomeSide,
    bolt_v3_numeric::{
        BPS_DENOMINATOR, UNIT_F64, ZERO_F64, is_non_negative_finite, sanitize_probability,
    },
};

const CENTS_PER_SHARE: f64 = 100.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BinaryOutcomeEdgeBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    FeeUnavailable,
    InvalidProbability,
    InvalidCost,
    UnsupportedOrderShape,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
}

impl BinaryOutcomeEdgeBlockReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingOrderBook => "missing_order_book",
            Self::InsufficientDepth => "insufficient_depth",
            Self::FeeUnavailable => "fee_unavailable",
            Self::InvalidProbability => "invalid_probability",
            Self::InvalidCost => "invalid_cost",
            Self::UnsupportedOrderShape => "unsupported_order_shape",
            Self::EdgeBelowThreshold => "edge_below_threshold",
            Self::SpreadOrSlippageWipedEdge => "spread_or_slippage_wiped_edge",
        }
    }

    fn cost_block_reason(self) -> Option<ExecutableCostBlockReason> {
        match self {
            Self::MissingOrderBook => Some(ExecutableCostBlockReason::MissingOrderBook),
            Self::InsufficientDepth => Some(ExecutableCostBlockReason::InsufficientDepth),
            Self::FeeUnavailable => Some(ExecutableCostBlockReason::FeeUnavailable),
            Self::InvalidCost => Some(ExecutableCostBlockReason::InvalidCost),
            Self::UnsupportedOrderShape => Some(ExecutableCostBlockReason::UnsupportedOrderShape),
            Self::InvalidProbability
            | Self::EdgeBelowThreshold
            | Self::SpreadOrSlippageWipedEdge => None,
        }
    }
}

impl std::fmt::Display for BinaryOutcomeEdgeBlockReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::fmt::Debug for BinaryOutcomeEdgeBlockReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<ExecutableCostBlockReason> for BinaryOutcomeEdgeBlockReason {
    fn from(reason: ExecutableCostBlockReason) -> Self {
        match reason {
            ExecutableCostBlockReason::MissingOrderBook => Self::MissingOrderBook,
            ExecutableCostBlockReason::InsufficientDepth => Self::InsufficientDepth,
            ExecutableCostBlockReason::FeeUnavailable => Self::FeeUnavailable,
            ExecutableCostBlockReason::InvalidCost => Self::InvalidCost,
            ExecutableCostBlockReason::UnsupportedOrderShape => Self::UnsupportedOrderShape,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BinaryOutcomeEdgeInputs {
    pub(crate) side: OutcomeSide,
    pub(crate) fair_probability_up: Option<f64>,
    pub(crate) adjusted_probability_up: Option<f64>,
    pub(crate) order_side: OrderSide,
    pub(crate) cost_breakdown: ExecutableCostBreakdown,
    pub(crate) minimum_edge_bps: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BinaryOutcomeEdgeResult {
    pub(crate) selected_side: OutcomeSide,
    pub(crate) adjusted_probability: f64,
    pub(crate) edge_bps: f64,
    pub(crate) edge_cents_per_share: f64,
    pub(crate) cost_breakdown: ExecutableCostBreakdown,
    pub(crate) trade_allowed: bool,
    pub(crate) block_reason: Option<BinaryOutcomeEdgeBlockReason>,
}

impl BinaryOutcomeEdgeResult {
    pub(crate) fn blocked(side: OutcomeSide, reason: BinaryOutcomeEdgeBlockReason) -> Self {
        let cost_reason = reason
            .cost_block_reason()
            .unwrap_or(ExecutableCostBlockReason::InvalidCost);
        Self::blocked_with_cost(side, ExecutableCostBreakdown::blocked(cost_reason), reason)
    }

    fn blocked_with_cost(
        side: OutcomeSide,
        cost_breakdown: ExecutableCostBreakdown,
        reason: BinaryOutcomeEdgeBlockReason,
    ) -> Self {
        Self {
            selected_side: side,
            adjusted_probability: ZERO_F64,
            edge_bps: ZERO_F64,
            edge_cents_per_share: ZERO_F64,
            cost_breakdown,
            trade_allowed: false,
            block_reason: Some(reason),
        }
    }
}

pub(crate) fn evaluate_binary_outcome_edge(
    inputs: &BinaryOutcomeEdgeInputs,
) -> BinaryOutcomeEdgeResult {
    if inputs.order_side != OrderSide::Buy {
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape,
        );
    }

    let Some(adjusted_probability_up) = inputs
        .adjusted_probability_up
        .and_then(sanitize_probability)
    else {
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            BinaryOutcomeEdgeBlockReason::InvalidProbability,
        );
    };
    let Some(fair_probability_up) = inputs.fair_probability_up.and_then(sanitize_probability)
    else {
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            BinaryOutcomeEdgeBlockReason::InvalidProbability,
        );
    };

    if !inputs.cost_breakdown.cost_available {
        let reason = inputs
            .cost_breakdown
            .block_reason
            .map(BinaryOutcomeEdgeBlockReason::from)
            .unwrap_or(BinaryOutcomeEdgeBlockReason::InvalidCost);
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            reason,
        );
    }

    let success_probability = match inputs.side {
        OutcomeSide::Up => adjusted_probability_up,
        OutcomeSide::Down => UNIT_F64 - adjusted_probability_up,
    };
    let fair_success_probability = match inputs.side {
        OutcomeSide::Up => fair_probability_up,
        OutcomeSide::Down => UNIT_F64 - fair_probability_up,
    };
    if !is_non_negative_finite(success_probability) || success_probability > UNIT_F64 {
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            BinaryOutcomeEdgeBlockReason::InvalidProbability,
        );
    }

    let gross_edge_cents_per_share =
        fair_success_probability * CENTS_PER_SHARE - inputs.cost_breakdown.gross_cost_cents;
    let edge_cents_per_share =
        success_probability * CENTS_PER_SHARE - inputs.cost_breakdown.total_adjusted_cost_cents;
    let edge_bps =
        edge_cents_per_share / inputs.cost_breakdown.total_adjusted_cost_cents * BPS_DENOMINATOR;
    if !edge_bps.is_finite() || !edge_cents_per_share.is_finite() {
        return BinaryOutcomeEdgeResult::blocked_with_cost(
            inputs.side,
            inputs.cost_breakdown,
            BinaryOutcomeEdgeBlockReason::InvalidCost,
        );
    }

    let mut block_reason = None;
    if edge_cents_per_share <= ZERO_F64
        || !inputs.minimum_edge_bps.is_finite()
        || edge_bps <= inputs.minimum_edge_bps
    {
        block_reason = Some(
            if gross_edge_cents_per_share > ZERO_F64 && edge_cents_per_share <= ZERO_F64 {
                BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge
            } else {
                BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold
            },
        );
    }

    BinaryOutcomeEdgeResult {
        selected_side: inputs.side,
        adjusted_probability: success_probability,
        edge_bps,
        edge_cents_per_share,
        cost_breakdown: inputs.cost_breakdown,
        trade_allowed: block_reason.is_none(),
        block_reason,
    }
}

#[cfg(test)]
mod tests {
    use nautilus_model::enums::OrderSide;

    use crate::{
        bolt_v3_executable_cost::{self, ExactSizeVwap, ExecutableCostBreakdown},
        bolt_v3_market_families::OutcomeSide,
    };

    const EPSILON: f64 = 1e-9;

    fn cost_breakdown(
        vwap_price: f64,
        fee_bps: f64,
        slippage_buffer_bps: u64,
    ) -> ExecutableCostBreakdown {
        bolt_v3_executable_cost::executable_cost_breakdown(
            ExactSizeVwap {
                vwap_price,
                vwap_quantity: 10.0,
                limit_price: vwap_price,
                exact_size_filled: true,
            },
            fee_bps,
            slippage_buffer_bps,
        )
        .expect("test cost breakdown should be available")
    }

    fn inputs(
        side: OutcomeSide,
        cost_breakdown: ExecutableCostBreakdown,
    ) -> super::BinaryOutcomeEdgeInputs {
        super::BinaryOutcomeEdgeInputs {
            side,
            fair_probability_up: Some(0.64),
            adjusted_probability_up: Some(0.64),
            order_side: OrderSide::Buy,
            cost_breakdown,
            minimum_edge_bps: 0.0,
        }
    }

    #[test]
    fn up_and_down_use_adjusted_probability_from_precomputed_cost() {
        let cost_breakdown = cost_breakdown(0.50, 0.0, 0);

        let up = super::evaluate_binary_outcome_edge(&inputs(OutcomeSide::Up, cost_breakdown));
        let down = super::evaluate_binary_outcome_edge(&inputs(OutcomeSide::Down, cost_breakdown));

        assert_eq!(up.adjusted_probability, 0.64);
        assert!((up.edge_bps - 2_800.0).abs() < EPSILON);
        assert!(up.trade_allowed);
        assert_eq!(down.adjusted_probability, 0.36);
        assert!((down.edge_bps - -2_800.0).abs() < EPSILON);
        assert_eq!(
            down.block_reason,
            Some(super::BinaryOutcomeEdgeBlockReason::EdgeBelowThreshold)
        );
    }

    #[test]
    fn fair_probability_controls_fee_slippage_wipe_classification() {
        let mut inputs = inputs(OutcomeSide::Up, cost_breakdown(0.50, 200.0, 0));
        inputs.fair_probability_up = Some(0.51);
        inputs.adjusted_probability_up = Some(0.505);

        let result = super::evaluate_binary_outcome_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(super::BinaryOutcomeEdgeBlockReason::SpreadOrSlippageWipedEdge)
        );
        assert!(!result.trade_allowed);
    }

    #[test]
    fn sell_side_binary_edge_remains_fail_closed() {
        let mut inputs = inputs(OutcomeSide::Up, cost_breakdown(0.50, 0.0, 0));
        inputs.order_side = OrderSide::Sell;

        let result = super::evaluate_binary_outcome_edge(&inputs);

        assert_eq!(
            result.block_reason,
            Some(super::BinaryOutcomeEdgeBlockReason::UnsupportedOrderShape)
        );
        assert!(!result.trade_allowed);
    }
}
