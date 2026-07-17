use bolt_v2::{
    economics::{LifecyclePath, LiquidityRoleAssumption, OrderSide},
    integrations::nautilus::economics::{
        NtEconomicsIntent, NtEconomicsMappingError, canonical_quote_request_from_nt,
    },
};
use nautilus_model::{
    enums::OrderSide as NtOrderSide,
    identifiers::{AccountId, InstrumentId},
    types::{Price, Quantity},
};

fn intent<'a>(legs: &'a [(Price, Quantity)]) -> NtEconomicsIntent<'a> {
    NtEconomicsIntent {
        execution_client_id: "execution-client",
        account_id: AccountId::from("ACCOUNT-001"),
        instrument_id: InstrumentId::from("BTC-USDC.VENUE"),
        product_surface_id: "perpetual",
        order_side: NtOrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: legs,
        routing_attachment_id: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: "primary-pnl",
        reporting_unit: "USDC",
        edge_basis_policy_id: "primary",
        requested_at_ns: 100,
        decision_correlation_id: "decision",
    }
}

#[test]
fn nt_intent_maps_exact_decimal_fill_plan() {
    let legs = [(Price::new(100.25, 2), Quantity::new(3.5, 1))];
    let request = canonical_quote_request_from_nt(intent(&legs)).unwrap();
    assert_eq!(request.order_side, OrderSide::Buy);
    assert_eq!(request.planned_fill_legs[0].price, legs[0].0.as_decimal());
    assert_eq!(
        request.planned_fill_legs[0].quantity,
        legs[0].1.as_decimal()
    );
}

#[test]
fn nt_intent_rejects_empty_fill_plan() {
    assert_eq!(
        canonical_quote_request_from_nt(intent(&[])),
        Err(NtEconomicsMappingError::InvalidFillLeg)
    );
}
