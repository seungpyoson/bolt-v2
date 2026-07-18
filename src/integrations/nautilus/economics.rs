use nautilus_model::{
    enums::OrderSide as NtOrderSide,
    events::OrderInitialized,
    identifiers::{AccountId as NtAccountId, InstrumentId as NtInstrumentId},
    orders::OrderAny,
};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_economics_runtime::EconomicsOrderBinding,
    economics::{
        AccountId, DecisionCorrelationId, EconomicQuoteRequest, EdgeBasisPolicyId,
        ExecutionClientId, InstrumentId, LifecyclePath, LiquidityRoleAssumption, NativeUnitId,
        OrderSide, PlannedFillLeg, PositionContext, ProductSurfaceId, ReportingPolicyId,
        RoutingAttachment, RoutingAttachmentId, RoutingContext,
    },
};

pub struct NtEconomicsIntent<'a> {
    pub execution_client_id: &'a str,
    pub account_id: NtAccountId,
    pub instrument_id: NtInstrumentId,
    pub product_surface_id: &'a str,
    pub order_side: NtOrderSide,
    pub liquidity_role: LiquidityRoleAssumption,
    pub planned_fill_legs: &'a [(Decimal, Decimal)],
    pub routing_attachment_id: Option<&'a str>,
    pub position: Option<PositionContext>,
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
    OrderBindingSerialization,
}

pub fn economics_order_binding(
    order: &OrderAny,
) -> Result<EconomicsOrderBinding, NtEconomicsMappingError> {
    let canonical_order = OrderInitialized::from(order);
    let bytes = serde_json::to_vec(&canonical_order)
        .map_err(|_| NtEconomicsMappingError::OrderBindingSerialization)?;
    Ok(EconomicsOrderBinding::from_sha256(Sha256::digest(bytes)))
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
            price: *price,
            quantity: *quantity,
        })
        .collect::<Vec<_>>();
    if planned_fill_legs.is_empty()
        || planned_fill_legs
            .iter()
            .any(|leg| leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO)
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
        position: intent.position,
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
