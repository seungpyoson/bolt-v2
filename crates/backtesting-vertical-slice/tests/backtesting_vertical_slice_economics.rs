use backtesting_vertical_slice::economics::{
    HistoricalEconomicsSnapshot, HistoricalEdgeBasisEvidence, HistoricalSourceSnapshot,
    ReplayEconomicsAdapter, ReplayEconomicsAdmissionSource, ReplayQuoteIntent,
    canonical_quote_request_from_replay,
};
use bolt_v2::bolt_v3_economics_runtime::{
    AuthoritativeEconomicsInputStore, AuthoritativeEconomicsQuoteDependencies,
    AuthoritativeEdgeBasis, ConfiguredEconomicsAdmissionSource, ConfiguredEconomicsSourcePolicy,
    EconomicsAdmissionPurpose, EconomicsAdmissionQuoteIntent, EconomicsAdmissionSource,
    EconomicsOrderBinding, identity_valuation_provider,
};
use bolt_v2::economics::{
    ExecutionClientId, FormulaId, InstrumentId, LiquidityRoleAssumption, OrderSide, PlannedFillLeg,
    PlannedFillNotional, ProductSurfaceId, ReservationBasis, SnapshotId, SourceId,
    VenueEconomicsAdapter,
};
use rust_decimal::Decimal;
use std::{str::FromStr, sync::Arc};

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
        economics_toml: toml::to_string(
            &root["clients"]["polymarket_main"]["execution"]["economics"],
        )
        .unwrap(),
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
                fetched_at_ns: 95, valid_until_ns: 110,
            },
            backtesting_vertical_slice::economics::HistoricalValuationObservation::MarketQuote {
                client_id: "coinbase_data".to_string(),
                instrument_id: "USDC-USD.COINBASE".to_string(), price: "1".to_string(),
                snapshot_id: "usdc-usd".to_string(), observed_at_ns: 90,
                fetched_at_ns: 95, valid_until_ns: 110,
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

fn planned_fill_notional(
    request: &bolt_v2::economics::EconomicQuoteRequest,
) -> PlannedFillNotional {
    PlannedFillNotional::from_legs(&request.planned_fill_legs).expect("valid planned legs")
}

#[test]
fn immutable_snapshot_maps_to_canonical_quote_and_edge_basis() {
    let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot()).expect("valid snapshot");
    let request = request(100);
    let planned_fill_notional = planned_fill_notional(&request);
    let estimate = adapter
        .quote(&request, planned_fill_notional)
        .expect("historical quote");
    let edge_basis = adapter
        .edge_basis(&request, planned_fill_notional)
        .expect("historical edge basis");

    assert_eq!(estimate.authority.snapshot_id.as_str(), "quote-snapshot");
    assert_eq!(estimate.components.len(), 1);
    assert_eq!(edge_basis.normalized_amount.amount(), dec("5"));
}

#[test]
fn historical_snapshot_fails_closed_outside_its_validity_window() {
    let adapter = ReplayEconomicsAdapter::from_snapshot(snapshot()).expect("valid snapshot");
    let request = request(111);
    assert!(
        adapter
            .quote(&request, planned_fill_notional(&request))
            .is_err()
    );
}

#[test]
fn historical_snapshot_rejects_class_sign_disagreement() {
    let mut fixture = snapshot();
    fixture.instrument_id = "other-instrument".to_string();
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("timeline is valid");
    let request = request(100);
    assert!(
        adapter
            .quote(&request, planned_fill_notional(&request))
            .is_err()
    );
}

#[test]
fn historical_fee_free_snapshot_is_valid() {
    let mut fixture = snapshot();
    fixture.source_snapshots[0].payload_json = include_str!(
        "../../../tests/fixtures/bolt_v3/boundary_evidence/polymarket-market-info-fee-free.json"
    )
    .to_string();
    let adapter = ReplayEconomicsAdapter::from_snapshot(fixture).expect("fee-free snapshot");

    let request = request(100);
    assert!(
        adapter
            .quote(&request, planned_fill_notional(&request))
            .unwrap()
            .components
            .is_empty()
    );
}

#[test]
fn production_and_replay_sources_produce_identical_sealed_admission() {
    let mut fixture = snapshot();
    fixture.reporting_unit = "pUSD".to_string();
    let request = {
        let mut request = request(100);
        request.reporting_unit = bolt_v2::economics::NativeUnitId::new("pUSD").unwrap();
        request
    };
    let replay = ReplayEconomicsAdmissionSource::from_snapshots(vec![fixture.clone()]).unwrap();
    let inputs = AuthoritativeEconomicsInputStore::default();
    inputs
        .publish(
            &fixture.execution_client_id,
            &fixture.instrument_id,
            &fixture.product_surface_id,
            AuthoritativeEconomicsQuoteDependencies {
                provider_key: fixture.provider_key.clone(),
                refreshed_at_ns: fixture.fetched_at_ns,
                adapter: Arc::new(ReplayEconomicsAdapter::from_snapshot(fixture.clone()).unwrap()),
                edge_basis: AuthoritativeEdgeBasis {
                    resolver_id: FormulaId::new(fixture.edge_basis.resolver_id.clone()).unwrap(),
                    product_metadata_source: SourceId::new(
                        fixture.edge_basis.product_metadata_source.clone(),
                    )
                    .unwrap(),
                    policy_version: fixture.edge_basis.policy_version,
                    source_snapshot_ids: fixture
                        .edge_basis
                        .source_snapshot_ids
                        .iter()
                        .cloned()
                        .map(SnapshotId::new)
                        .collect::<Result<Vec<_>, _>>()
                        .unwrap(),
                    valid_until_ns: fixture.edge_basis.valid_until_ns,
                },
                valuation_provider: identity_valuation_provider(),
            },
        )
        .unwrap();
    let production = ConfiguredEconomicsAdmissionSource::new(
        &fixture.provider_key,
        inputs,
        ConfiguredEconomicsSourcePolicy {
            quote_refresh_ns: 30_000_000_000,
            quote_max_age_ns: 60_000_000_000,
            quote_validity_ns: 30_000_000_000,
            resting_order_refresh_margin_ns: 5_000_000_000,
        },
    )
    .unwrap();
    let order_binding =
        EconomicsOrderBinding::from_sha256(<sha2::Sha256 as sha2::Digest>::digest(b"parity-order"));
    let make_intent = || EconomicsAdmissionQuoteIntent {
        request: request.clone(),
        order_binding: order_binding.clone(),
        purpose: EconomicsAdmissionPurpose::TradingEdge,
        gross_expected_value: dec("10"),
        reservation_basis: ReservationBasis::new(dec("5.50")).unwrap(),
    };

    assert_eq!(
        production.quote_admission(make_intent()).unwrap(),
        replay.quote_admission(make_intent()).unwrap()
    );
}

#[test]
fn product_surface_resolution_deduplicates_successive_snapshot_epochs() {
    let mut first = snapshot();
    first.valid_until_ns = 130;
    first.source_snapshots[0].valid_until_ns = first.valid_until_ns;
    for observation in &mut first.valuation_observations {
        match observation {
            backtesting_vertical_slice::economics::HistoricalValuationObservation::MarketQuote {
                valid_until_ns,
                ..
            }
            | backtesting_vertical_slice::economics::HistoricalValuationObservation::ProviderConversion {
                valid_until_ns,
                ..
            } => *valid_until_ns = first.valid_until_ns,
        }
    }
    let mut second = first.clone();
    second.snapshot_id = "quote-snapshot-next".to_string();
    second.source_at_ns = 105;
    second.fetched_at_ns = 115;
    second.valid_until_ns = 140;
    second.edge_basis.source_snapshot_ids = vec![second.snapshot_id.clone()];
    second.edge_basis.valid_until_ns = second.valid_until_ns;
    second.source_snapshots[0].snapshot_id = second.snapshot_id.clone();
    second.source_snapshots[0].source_at_ns = second.source_at_ns;
    second.source_snapshots[0].fetched_at_ns = second.fetched_at_ns;
    second.source_snapshots[0].valid_until_ns = second.valid_until_ns;
    for observation in &mut second.valuation_observations {
        match observation {
            backtesting_vertical_slice::economics::HistoricalValuationObservation::MarketQuote {
                valid_until_ns,
                ..
            }
            | backtesting_vertical_slice::economics::HistoricalValuationObservation::ProviderConversion {
                valid_until_ns,
                ..
            } => *valid_until_ns = second.valid_until_ns,
        }
    }
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
    assert_eq!(
        source
            .snapshot_for_request(&request(112))
            .unwrap()
            .snapshot_id,
        "quote-snapshot"
    );
    assert_eq!(
        source
            .snapshot_for_request(&request(120))
            .unwrap()
            .snapshot_id,
        "quote-snapshot-next"
    );
}

#[test]
fn replay_authority_rejects_mixed_provider_keys() {
    let first = snapshot();
    let mut second = first.clone();
    second.provider_key = "HYPERLIQUID".to_string();

    assert!(matches!(
        ReplayEconomicsAdmissionSource::from_snapshots(vec![first, second]),
        Err(bolt_v2::economics::EconomicsUnavailable::AmbiguousQuoteAuthority)
    ));
}

#[test]
fn replay_authority_rejects_future_valuation_observation() {
    let mut fixture = snapshot();
    let backtesting_vertical_slice::economics::HistoricalValuationObservation::MarketQuote {
        observed_at_ns,
        fetched_at_ns,
        ..
    } = &mut fixture.valuation_observations[1]
    else {
        panic!("expected market quote fixture")
    };
    *observed_at_ns = fixture.source_at_ns + 1;
    *fetched_at_ns = fixture.fetched_at_ns + 1;

    assert!(matches!(
        ReplayEconomicsAdmissionSource::from_snapshots(vec![fixture]),
        Err(bolt_v2::economics::EconomicsUnavailable::InvalidQuoteValidityPolicy)
    ));
}

#[test]
fn replay_authority_rejects_duplicate_valuation_authority() {
    let mut fixture = snapshot();
    let duplicate = fixture.valuation_observations[0].clone();
    fixture.valuation_observations.push(duplicate);

    assert!(matches!(
        ReplayEconomicsAdmissionSource::from_snapshots(vec![fixture]),
        Err(bolt_v2::economics::EconomicsUnavailable::InvalidQuoteValidityPolicy)
    ));
}

#[test]
fn replay_authority_rejects_source_outside_snapshot_timeline() {
    let mut fixture = snapshot();
    fixture.source_snapshots[0].fetched_at_ns = fixture.fetched_at_ns + 1;

    assert!(matches!(
        ReplayEconomicsAdmissionSource::from_snapshots(vec![fixture]),
        Err(bolt_v2::economics::EconomicsUnavailable::InvalidSourceTimeline { .. })
    ));
}
