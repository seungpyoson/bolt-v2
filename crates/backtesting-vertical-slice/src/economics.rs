use bolt_v2::economics::{
    AccountId, ActionId, CurrencyId, DecisionCorrelationId, EconomicsError, EconomicsInstrumentId,
    EconomicsQuoteRequest, EdgeBasisPolicyId, ExecutionClientId, LifecyclePath, LiquidityRole,
    OrderSide, PlannedFillLeg, PositionContext, PositionId, PositionSide, ProductSurfaceId,
    ReportingPolicyId, RoutingAttachmentId, RoutingContext,
};
use rust_decimal::Decimal;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayLiquidityRole {
    GuaranteedMaker,
    Taker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPositionSide {
    Long,
    Short,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPlannedFillLeg {
    pub price: Decimal,
    pub quantity: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPositionContext {
    pub position_id: String,
    pub side: ReplayPositionSide,
    pub quantity: Decimal,
    pub holding_horizon_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayLifecyclePath {
    PlannedExit,
    HoldToSettlement,
    HoldToRedemption,
    Transfer { action_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayEconomicsIntent {
    pub execution_client_id: String,
    pub account_id: String,
    pub instrument_id: String,
    pub product_surface_id: String,
    pub reporting_policy_id: String,
    pub reporting_currency: String,
    pub edge_basis_policy_id: String,
    pub decision_correlation_id: String,
    pub side: ReplayOrderSide,
    pub liquidity_role: ReplayLiquidityRole,
    pub planned_fill_legs: Vec<ReplayPlannedFillLeg>,
    pub routing_attachment_id: Option<String>,
    pub position: Option<ReplayPositionContext>,
    pub lifecycle_path: ReplayLifecyclePath,
    pub requested_at_ns: u64,
}

pub fn economics_request_from_replay(
    intent: ReplayEconomicsIntent,
) -> Result<EconomicsQuoteRequest, EconomicsError> {
    let request = EconomicsQuoteRequest {
        execution_client_id: ExecutionClientId::try_new(intent.execution_client_id)?,
        account_id: AccountId::try_new(intent.account_id)?,
        instrument_id: EconomicsInstrumentId::try_new(intent.instrument_id)?,
        product_surface_id: ProductSurfaceId::try_new(intent.product_surface_id)?,
        order_side: match intent.side {
            ReplayOrderSide::Buy => OrderSide::Buy,
            ReplayOrderSide::Sell => OrderSide::Sell,
        },
        liquidity_role: match intent.liquidity_role {
            ReplayLiquidityRole::GuaranteedMaker => LiquidityRole::GuaranteedMaker,
            ReplayLiquidityRole::Taker => LiquidityRole::Taker,
        },
        planned_fill_legs: intent
            .planned_fill_legs
            .into_iter()
            .map(|leg| PlannedFillLeg {
                price: leg.price,
                quantity: leg.quantity,
            })
            .collect(),
        routing: RoutingContext {
            attached_charge: intent
                .routing_attachment_id
                .map(RoutingAttachmentId::try_new)
                .transpose()?,
        },
        position: intent
            .position
            .map(|position| {
                Ok(PositionContext {
                    position_id: PositionId::try_new(position.position_id)?,
                    side: match position.side {
                        ReplayPositionSide::Long => PositionSide::Long,
                        ReplayPositionSide::Short => PositionSide::Short,
                    },
                    quantity: position.quantity,
                    holding_horizon_ns: position.holding_horizon_ns,
                })
            })
            .transpose()?,
        lifecycle_path: match intent.lifecycle_path {
            ReplayLifecyclePath::PlannedExit => LifecyclePath::PlannedExit,
            ReplayLifecyclePath::HoldToSettlement => LifecyclePath::HoldToSettlement,
            ReplayLifecyclePath::HoldToRedemption => LifecyclePath::HoldToRedemption,
            ReplayLifecyclePath::Transfer { action_id } => LifecyclePath::Transfer {
                action_id: ActionId::try_new(action_id)?,
            },
        },
        reporting_policy_id: ReportingPolicyId::try_new(intent.reporting_policy_id)?,
        reporting_currency: CurrencyId::try_new(intent.reporting_currency)?,
        edge_basis_policy_id: EdgeBasisPolicyId::try_new(intent.edge_basis_policy_id)?,
        requested_at_ns: intent.requested_at_ns,
        decision_correlation_id: DecisionCorrelationId::try_new(intent.decision_correlation_id)?,
    };
    request.validate()?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use bolt_v2::integrations::nautilus::economics::{
        NautilusEconomicsIntent, NautilusEstimateLiquidityRole, NautilusPlannedFillLeg,
        economics_request_from_nautilus,
    };
    use nautilus_model::{
        enums::OrderSide as NautilusOrderSide,
        identifiers::{InstrumentId, Symbol, Venue},
    };

    use super::*;

    fn replay_intent() -> ReplayEconomicsIntent {
        ReplayEconomicsIntent {
            execution_client_id: "execution".to_owned(),
            account_id: "SIM-001".to_owned(),
            instrument_id: "BTC-USD.SIM".to_owned(),
            product_surface_id: "spot".to_owned(),
            reporting_policy_id: "reporting".to_owned(),
            reporting_currency: "USD".to_owned(),
            edge_basis_policy_id: "basis".to_owned(),
            decision_correlation_id: "decision".to_owned(),
            side: ReplayOrderSide::Buy,
            liquidity_role: ReplayLiquidityRole::Taker,
            planned_fill_legs: vec![ReplayPlannedFillLeg {
                price: Decimal::new(10_025, 2),
                quantity: Decimal::new(30, 1),
            }],
            routing_attachment_id: None,
            position: None,
            lifecycle_path: ReplayLifecyclePath::PlannedExit,
            requested_at_ns: 1_000,
        }
    }

    #[test]
    fn replay_intent_maps_to_the_neutral_request() {
        let request = economics_request_from_replay(replay_intent())
            .expect("valid replay intent should adapt");

        assert_eq!(request.instrument_id.as_str(), "BTC-USD.SIM");
        assert_eq!(request.reporting_currency.as_str(), "USD");
        assert_eq!(request.order_side, OrderSide::Buy);
        assert_eq!(request.liquidity_role, LiquidityRole::Taker);
        assert_eq!(request.planned_fill_legs[0].price, Decimal::new(10_025, 2));
    }

    #[test]
    fn live_and_replay_facts_produce_the_identical_neutral_request() {
        let legs = [NautilusPlannedFillLeg {
            price: Decimal::new(10_025, 2),
            quantity: Decimal::new(30, 1),
        }];
        let live = economics_request_from_nautilus(NautilusEconomicsIntent {
            execution_client_id: "execution",
            account_id: "SIM-001",
            instrument_id: InstrumentId::new(Symbol::new("BTC-USD"), Venue::new("SIM")),
            product_surface_id: "spot",
            reporting_policy_id: "reporting",
            reporting_currency: "USD",
            edge_basis_policy_id: "basis",
            decision_correlation_id: "decision",
            side: NautilusOrderSide::Buy,
            liquidity_role: NautilusEstimateLiquidityRole::Taker,
            planned_fill_legs: &legs,
            routing_attachment_id: None,
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            requested_at_ns: 1_000,
        })
        .expect("valid live intent should adapt");
        let replay = economics_request_from_replay(replay_intent())
            .expect("valid replay intent should adapt");

        assert_eq!(live, replay);
    }

    #[test]
    fn replay_intent_rejects_missing_fill_and_holding_horizon() {
        let mut missing_fill = replay_intent();
        missing_fill.planned_fill_legs.clear();
        assert_eq!(
            economics_request_from_replay(missing_fill),
            Err(EconomicsError::InvalidPlannedFill)
        );

        let mut missing_horizon = replay_intent();
        missing_horizon.position = Some(ReplayPositionContext {
            position_id: "position".to_owned(),
            side: ReplayPositionSide::Long,
            quantity: Decimal::ONE,
            holding_horizon_ns: 0,
        });
        assert_eq!(
            economics_request_from_replay(missing_horizon),
            Err(EconomicsError::MissingHoldingHorizon)
        );
    }
}
