use nautilus_model::{
    enums::OrderSide as NautilusOrderSide,
    identifiers::{AccountId as NautilusAccountId, InstrumentId},
    types::{Currency, Price, Quantity},
};

use crate::economics::{
    AccountId, DecisionCorrelationId, EconomicsError, EconomicsInstrumentId, EconomicsQuoteRequest,
    EdgeBasisPolicyId, ExecutionClientId, LifecyclePath, LiquidityRole, OrderSide, PlannedFillLeg,
    PositionContext, ProductSurfaceId, ReportingPolicyId, RoutingAttachmentId, RoutingContext,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NautilusEconomicsAdapterError {
    MissingOrderSide,
    MissingLiquiditySide,
    InvalidEconomics(EconomicsError),
}

impl std::fmt::Display for NautilusEconomicsAdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingOrderSide => f.write_str("Nautilus order side is unspecified"),
            Self::MissingLiquiditySide => f.write_str("Nautilus liquidity side is unspecified"),
            Self::InvalidEconomics(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for NautilusEconomicsAdapterError {}

impl From<EconomicsError> for NautilusEconomicsAdapterError {
    fn from(value: EconomicsError) -> Self {
        Self::InvalidEconomics(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NautilusPlannedFillLeg {
    pub price: Price,
    pub quantity: Quantity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NautilusEstimateLiquidityRole {
    GuaranteedMaker,
    Taker,
    Unspecified,
}

#[derive(Debug, Clone)]
pub struct NautilusEconomicsIntent<'a> {
    pub execution_client_id: &'a str,
    pub account_id: NautilusAccountId,
    pub instrument_id: InstrumentId,
    pub product_surface_id: &'a str,
    pub reporting_policy_id: &'a str,
    pub reporting_currency: &'a Currency,
    pub edge_basis_policy_id: &'a str,
    pub decision_correlation_id: &'a str,
    pub side: NautilusOrderSide,
    pub liquidity_role: NautilusEstimateLiquidityRole,
    pub planned_fill_legs: &'a [NautilusPlannedFillLeg],
    pub routing_attachment_id: Option<&'a str>,
    pub position: Option<PositionContext>,
    pub lifecycle_path: LifecyclePath,
    pub requested_at_ns: u64,
}

pub fn economics_request_from_nautilus(
    intent: NautilusEconomicsIntent<'_>,
) -> Result<EconomicsQuoteRequest, NautilusEconomicsAdapterError> {
    let order_side = match intent.side {
        NautilusOrderSide::Buy => OrderSide::Buy,
        NautilusOrderSide::Sell => OrderSide::Sell,
        NautilusOrderSide::NoOrderSide => {
            return Err(NautilusEconomicsAdapterError::MissingOrderSide);
        }
    };
    let liquidity_role = match intent.liquidity_role {
        NautilusEstimateLiquidityRole::GuaranteedMaker => LiquidityRole::GuaranteedMaker,
        NautilusEstimateLiquidityRole::Taker => LiquidityRole::Taker,
        NautilusEstimateLiquidityRole::Unspecified => {
            return Err(NautilusEconomicsAdapterError::MissingLiquiditySide);
        }
    };
    let request = EconomicsQuoteRequest {
        execution_client_id: ExecutionClientId::try_new(intent.execution_client_id)?,
        account_id: AccountId::try_new(intent.account_id.to_string())?,
        instrument_id: EconomicsInstrumentId::try_new(intent.instrument_id.to_string())?,
        product_surface_id: ProductSurfaceId::try_new(intent.product_surface_id)?,
        order_side,
        liquidity_role,
        planned_fill_legs: intent
            .planned_fill_legs
            .iter()
            .map(|leg| PlannedFillLeg {
                price: leg.price.as_decimal(),
                quantity: leg.quantity.as_decimal(),
            })
            .collect(),
        routing: RoutingContext {
            attached_charge: intent
                .routing_attachment_id
                .map(RoutingAttachmentId::try_new)
                .transpose()?,
        },
        position: intent.position,
        lifecycle_path: intent.lifecycle_path,
        reporting_policy_id: ReportingPolicyId::try_new(intent.reporting_policy_id)?,
        reporting_currency: crate::economics::CurrencyId::try_new(
            intent.reporting_currency.code.to_string(),
        )?,
        edge_basis_policy_id: EdgeBasisPolicyId::try_new(intent.edge_basis_policy_id)?,
        requested_at_ns: intent.requested_at_ns,
        decision_correlation_id: DecisionCorrelationId::try_new(intent.decision_correlation_id)?,
    };
    request.validate()?;
    Ok(request)
}

#[cfg(test)]
mod tests {
    use nautilus_model::{
        enums::CurrencyType,
        identifiers::{AccountId as NautilusAccountId, InstrumentId, Symbol, Venue},
        types::{Currency, Price, Quantity},
    };
    use rust_decimal::Decimal;

    use super::*;

    fn usd() -> Currency {
        Currency::new_checked("USD", 2, 840, "US Dollar", CurrencyType::Fiat)
            .expect("currency fixture should be valid")
    }

    fn intent<'a>(
        currency: &'a Currency,
        legs: &'a [NautilusPlannedFillLeg],
    ) -> NautilusEconomicsIntent<'a> {
        NautilusEconomicsIntent {
            execution_client_id: "execution",
            account_id: NautilusAccountId::from("SIM-001"),
            instrument_id: InstrumentId::new(Symbol::new("BTC-USD"), Venue::new("SIM")),
            product_surface_id: "spot",
            reporting_policy_id: "reporting",
            reporting_currency: currency,
            edge_basis_policy_id: "basis",
            decision_correlation_id: "decision",
            side: NautilusOrderSide::Buy,
            liquidity_role: NautilusEstimateLiquidityRole::Taker,
            planned_fill_legs: legs,
            routing_attachment_id: None,
            position: None,
            lifecycle_path: LifecyclePath::PlannedExit,
            requested_at_ns: 1_000,
        }
    }

    #[test]
    fn nautilus_intent_maps_every_level_to_the_neutral_economics_request() {
        let currency = usd();
        let legs = [
            NautilusPlannedFillLeg {
                price: Price::new(100.25, 2),
                quantity: Quantity::new(3.0, 1),
            },
            NautilusPlannedFillLeg {
                price: Price::new(100.50, 2),
                quantity: Quantity::new(2.0, 1),
            },
        ];
        let request = economics_request_from_nautilus(intent(&currency, &legs))
            .expect("valid Nautilus intent should adapt");

        assert_eq!(request.instrument_id.as_str(), "BTC-USD.SIM");
        assert_eq!(request.reporting_currency.as_str(), "USD");
        assert_eq!(request.order_side, OrderSide::Buy);
        assert_eq!(request.liquidity_role, LiquidityRole::Taker);
        assert_eq!(request.planned_fill_legs.len(), 2);
        assert_eq!(request.planned_fill_legs[0].price, Decimal::new(10_025, 2));
    }

    #[test]
    fn nautilus_intent_rejects_unspecified_execution_facts() {
        let currency = usd();
        let legs = [NautilusPlannedFillLeg {
            price: Price::new(100.0, 1),
            quantity: Quantity::new(1.0, 1),
        }];
        assert_eq!(
            economics_request_from_nautilus(NautilusEconomicsIntent {
                side: NautilusOrderSide::NoOrderSide,
                ..intent(&currency, &legs)
            }),
            Err(NautilusEconomicsAdapterError::MissingOrderSide)
        );
        assert_eq!(
            economics_request_from_nautilus(NautilusEconomicsIntent {
                liquidity_role: NautilusEstimateLiquidityRole::Unspecified,
                ..intent(&currency, &legs)
            }),
            Err(NautilusEconomicsAdapterError::MissingLiquiditySide)
        );
    }
}
