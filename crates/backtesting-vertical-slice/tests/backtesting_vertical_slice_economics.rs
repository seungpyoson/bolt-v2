use backtesting_vertical_slice::economics::{
    HistoricalEconomicsSnapshot, HistoricalEdgeBasisEvidence, HistoricalSourceSnapshot,
    ReplayEconomicsAdapter, ReplayEconomicsAdmissionSource, ReplayQuoteIntent,
    canonical_quote_request_from_replay,
};
use bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionSource;
use bolt_v2::economics::{
    ExecutionClientId, InstrumentId, LiquidityRoleAssumption, OrderSide, PlannedFillLeg,
    ProductSurfaceId, VenueEconomicsAdapter,
};
use rust_decimal::Decimal;
use std::str::FromStr;

fn dec(value: &str) -> Decimal {
    Decimal::from_str(value).expect("fixture decimal")
}

fn snapshot() -> HistoricalEconomicsSnapshot {
    let root: toml::Value = toml::from_str(include_str!("../../../config/root.toml")).unwrap();
    HistoricalEconomicsSnapshot {
        provider_key: "POLYMARKET".to_string(),
        execution_client_id: "execution-client".to_string(),
        account_id: "account".to_string(),
        instrument_id: "instrument".to_string(),
        raw_symbol: "instrument".to_string(),
        product_surface_id: "binary_outcome".to_string(),
        reporting_policy_id: "primary-pnl".to_string(),
        reporting_unit: "USD".to_string(),
        snapshot_id: "quote-snapshot".to_string(),
        source_id: "historical-source".to_string(),
        source_at_ns: 90,
        fetched_at_ns: 95,
        valid_until_ns: 110,
        economics: root["clients"]["polymarket_main"]["execution"]["economics"].clone(),
        edge_basis: HistoricalEdgeBasisEvidence {
            policy_id: "primary".to_string(),
            resolver_id: "product-metadata".to_string(),
            product_metadata_source: "polymarket-market-info".to_string(),
            policy_version: 1,
            source_snapshot_ids: vec!["quote-snapshot".to_string()],
            valid_until_ns: 110,
        },
        source_snapshots: vec![HistoricalSourceSnapshot {
            source_id: "clob_market_info".to_string(),
            snapshot_id: "quote-snapshot".to_string(),
            source_at_ns: 90,
            fetched_at_ns: 95,
            valid_until_ns: 110,
            payload_json: include_str!(
                "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-bearing.json"
            )
            .to_string(),
        }],
        valuation_observations: vec![
            backtesting_vertical_slice::economics::HistoricalValuationObservation::ProviderConversion {
                source_id: "collateral".to_string(), from_unit: "pUSD".to_string(),
                to_unit: "USDC".to_string(), rate: "1".to_string(),
                snapshot_id: "pusd-usdc".to_string(), observed_at_ns: 90,
            },
            backtesting_vertical_slice::economics::HistoricalValuationObservation::MarketQuote {
                client_id: "coinbase_data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(), price: "1".to_string(),
                snapshot_id: "usdc-usd".to_string(), observed_at_ns: 90,
            },
        ],
    }
}

fn request(requested_at_ns: u64) -> bolt_v2::economics::EconomicQuoteRequest {
    canonical_quote_request_from_replay(ReplayQuoteIntent {
        execution_client_id: "execution-client",
        account_id: "account",
        instrument_id: "instrument",
        product_surface_id: "binary_outcome",
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRoleAssumption::Taker,
        planned_fill_legs: vec![PlannedFillLeg {
            price: dec("0.50"),
            quantity: dec("10"),
        }],
        routing_attachment_id: None,
        position: None,
        lifecycle_path: bolt_v2::economics::LifecyclePath::PlannedExit,
        reporting_policy_id: "primary-pnl",
        reporting_unit: "USD",
        requested_at_ns,
        decision_correlation_id: "decision",
        edge_basis_policy_id: "primary",
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
    fixture.instrument_id = "other-instrument".to_string();
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("timeline is valid");
    assert!(adapter.quote(&request(100)).is_err());
}

#[test]
fn historical_fee_free_snapshot_is_valid() {
    let mut fixture = snapshot();
    fixture.source_snapshots[0].payload_json = include_str!(
        "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-free.json"
    )
    .to_string();
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("fee-free snapshot");

    assert!(adapter.quote(&request(100)).unwrap().components.is_empty());
}

#[test]
fn product_surface_resolution_deduplicates_successive_snapshot_epochs() {
    let first = snapshot();
    let mut second = first.clone();
    second.snapshot_id = "quote-snapshot-next".to_string();
    second.source_at_ns = 111;
    second.fetched_at_ns = 115;
    second.valid_until_ns = 130;
    second.edge_basis.source_snapshot_ids = vec![second.snapshot_id.clone()];
    second.edge_basis.valid_until_ns = second.valid_until_ns;
    second.source_snapshots[0].snapshot_id = second.snapshot_id.clone();
    second.source_snapshots[0].source_at_ns = second.source_at_ns;
    second.source_snapshots[0].fetched_at_ns = second.fetched_at_ns;
    second.source_snapshots[0].valid_until_ns = second.valid_until_ns;
    let source = ReplayEconomicsAdmissionSource::from_snapshots(vec![first, second]).unwrap();

    assert_eq!(
        source
            .resolve_product_surface(
                &ExecutionClientId::new("execution-client").unwrap(),
                &InstrumentId::new("instrument").unwrap(),
                &[ProductSurfaceId::new("binary_outcome").unwrap()],
            )
            .unwrap()
            .as_str(),
        "binary_outcome"
    );
}
