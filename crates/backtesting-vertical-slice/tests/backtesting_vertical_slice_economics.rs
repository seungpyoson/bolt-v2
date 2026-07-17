use backtesting_vertical_slice::economics::{
    HistoricalAdmissionTreatment, HistoricalEconomicClass, HistoricalEconomicComponent,
    HistoricalEconomicsSnapshot, HistoricalEdgeBasisEvidence, ReplayEconomicsAdapter,
    ReplayQuoteIntent, canonical_quote_request_from_replay,
};
use bolt_v2::economics::{
    LiquidityRoleAssumption, OrderSide, PlannedFillLeg, VenueEconomicsAdapter,
};
use rust_decimal::Decimal;
use std::str::FromStr;

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).expect("fixture decimal")
}

fn snapshot() -> HistoricalEconomicsSnapshot {
    HistoricalEconomicsSnapshot {
        execution_client_id: "execution-client".to_string(),
        account_id: "account".to_string(),
        product_surface_id: "surface".to_string(),
        reporting_policy_id: "reporting-policy".to_string(),
        reporting_unit: "pUSD".to_string(),
        snapshot_id: "quote-snapshot".to_string(),
        source_id: "historical-source".to_string(),
        source_at_ns: 90,
        fetched_at_ns: 95,
        valid_until_ns: 110,
        edge_basis: HistoricalEdgeBasisEvidence {
            policy_id: "edge-policy".to_string(),
            policy_version: 1,
            normalized_amount: "5".to_string(),
            source_snapshot_ids: vec!["basis-snapshot".to_string()],
            valid_until_ns: 110,
        },
        components: vec![HistoricalEconomicComponent {
            component_id: "protocol-charge".to_string(),
            order_id: "order".to_string(),
            class: HistoricalEconomicClass::Charge,
            treatment: HistoricalAdmissionTreatment::GuaranteedConditionalOnAction,
            native_amount: "-0.05".to_string(),
            native_unit: "pUSD".to_string(),
            debit_risk_bound: None,
            formula_id: "historical-formula".to_string(),
            source_id: "component-source".to_string(),
            snapshot_id: "component-snapshot".to_string(),
            source_at_ns: 90,
            fetched_at_ns: 95,
            valid_until_ns: 110,
            valuation: None,
        }],
    }
}

fn request(requested_at_ns: u64) -> bolt_v2::economics::EconomicQuoteRequest {
    canonical_quote_request_from_replay(ReplayQuoteIntent {
        execution_client_id: "execution-client",
        account_id: "account",
        instrument_id: "instrument",
        product_surface_id: "surface",
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: vec![PlannedFillLeg {
            price: dec("0.50"),
            quantity: dec("10"),
        }],
        reporting_policy_id: "reporting-policy",
        reporting_unit: "pUSD",
        requested_at_ns,
        decision_correlation_id: "decision",
        edge_basis_policy_id: "edge-policy",
    })
    .expect("canonical replay request")
}

#[test]
fn immutable_snapshot_maps_to_canonical_quote_and_edge_basis() {
    let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot()).expect("valid snapshot");
    let request = request(100);
    let estimate = adapter.quote(&request).expect("historical quote");
    let edge_basis = adapter.edge_basis(&request).expect("historical edge basis");

    assert_eq!(estimate.authority.snapshot_id.as_str(), "quote-snapshot");
    assert_eq!(estimate.components.len(), 1);
    assert_eq!(edge_basis.normalized_amount, dec("5"));
}

#[test]
fn historical_snapshot_fails_closed_outside_its_validity_window() {
    let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot()).expect("valid snapshot");
    assert!(adapter.quote(&request(111)).is_err());
}

#[test]
fn historical_snapshot_rejects_class_sign_disagreement() {
    let mut fixture = snapshot();
    fixture.components[0].class = HistoricalEconomicClass::Credit;
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("timeline is valid");
    assert!(adapter.quote(&request(100)).is_err());
}

#[test]
fn historical_fee_free_snapshot_is_valid() {
    let mut fixture = snapshot();
    fixture.components.clear();
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("fee-free snapshot");

    assert!(adapter.quote(&request(100)).unwrap().components.is_empty());
}
