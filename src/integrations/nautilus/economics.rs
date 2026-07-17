use nautilus_model::{
    enums::OrderSide as NtOrderSide,
    identifiers::{AccountId as NtAccountId, InstrumentId as NtInstrumentId},
    types::{Price, Quantity},
};

use crate::economics::{
    AccountId, DecisionCorrelationId, EconomicQuoteRequest, EdgeBasisPolicyId, ExecutionClientId,
    InstrumentId, LifecyclePath, LiquidityRoleAssumption, NativeUnitId, OrderSide, PlannedFillLeg,
    ProductSurfaceId, ReportingPolicyId, RoutingAttachment, RoutingAttachmentId, RoutingContext,
};

pub struct NtEconomicsIntent<'a> {
    pub execution_client_id: &'a str,
    pub account_id: NtAccountId,
    pub instrument_id: NtInstrumentId,
    pub product_surface_id: &'a str,
    pub order_side: NtOrderSide,
    pub liquidity_role: LiquidityRoleAssumption,
    pub planned_fill_legs: &'a [(Price, Quantity)],
    pub routing_attachment_id: Option<&'a str>,
    pub lifecycle_path: LifecyclePath,
    pub reporting_policy_id: &'a str,
    pub reporting_unit: &'a str,
    pub edge_basis_policy_id: &'a str,
    pub requested_at_ns: u64,
    pub decision_correlation_id: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NtEconomicsMappingError {
    InvalidIdentity,
    UnsupportedOrderSide,
    InvalidFillLeg,
}

pub fn canonical_quote_request_from_nt(
    intent: NtEconomicsIntent<'_>,
) -> Result<EconomicQuoteRequest, NtEconomicsMappingError> {
    let order_side = match intent.order_side {
        NtOrderSide::Buy => OrderSide::Buy,
        NtOrderSide::Sell => OrderSide::Sell,
        _ => return Err(NtEconomicsMappingError::UnsupportedOrderSide),
    };
    let planned_fill_legs = intent
        .planned_fill_legs
        .iter()
        .map(|(price, quantity)| PlannedFillLeg {
            price: price.as_decimal(),
            quantity: quantity.as_decimal(),
        })
        .collect::<Vec<_>>();
    if planned_fill_legs.is_empty()
        || planned_fill_legs.iter().any(|leg| {
            leg.price <= rust_decimal::Decimal::ZERO || leg.quantity <= rust_decimal::Decimal::ZERO
        })
    {
        return Err(NtEconomicsMappingError::InvalidFillLeg);
    }
    let attached_charge = match intent.routing_attachment_id {
        None => None,
        Some(value) => Some(RoutingAttachment {
            attachment_id: RoutingAttachmentId::new(value)
                .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        }),
    };
    Ok(EconomicQuoteRequest {
        execution_client_id: ExecutionClientId::new(intent.execution_client_id)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        account_id: AccountId::new(intent.account_id.to_string())
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        instrument_id: InstrumentId::new(intent.instrument_id.to_string())
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        product_surface_id: ProductSurfaceId::new(intent.product_surface_id)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        order_side,
        liquidity_role: intent.liquidity_role,
        planned_fill_legs,
        routing: RoutingContext { attached_charge },
        position: None,
        lifecycle_path: intent.lifecycle_path,
        reporting_policy_id: ReportingPolicyId::new(intent.reporting_policy_id)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        reporting_unit: NativeUnitId::new(intent.reporting_unit)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        edge_basis_policy_id: EdgeBasisPolicyId::new(intent.edge_basis_policy_id)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
        requested_at_ns: intent.requested_at_ns,
        decision_correlation_id: DecisionCorrelationId::new(intent.decision_correlation_id)
            .map_err(|_| NtEconomicsMappingError::InvalidIdentity)?,
    })
}
