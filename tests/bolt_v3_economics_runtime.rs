use crate::support;

use bolt_v2::{
    bolt_v3_config::{ClientBlock, ExecutionEconomicsConfig, load_bolt_v3_config},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        AuthoritativeVenueEconomicsInput, EconomicsAdmissionIntent, EconomicsAdmissionPolicy,
        EconomicsOrderBinding, EconomicsRuntimeBindingError, RestingOrderEconomicsCancelReason,
        RestingOrderEconomicsRefresh, bind_execution_economics, refresh_resting_order_economics,
    },
    bolt_v3_order_execution::{
        BoltV3OrderEconomicsHandle, BoltV3OrderEconomicsIntent, BoltV3PlannedFillLeg,
        order_intent_details_from_compiled_order,
    },
    bolt_v3_providers::{
        hyperliquid::{
            HyperliquidPerpetualSnapshotInput, HyperliquidProductEconomicsSnapshot,
            HyperliquidSnapshotMetadata, HyperliquidUserFeesSnapshot,
            authoritative_economics_input as hyperliquid_authoritative_economics_input,
        },
        polymarket::{
            PolymarketExecutionConfig, PolymarketMarketInfoSnapshot, PolymarketSnapshotMetadata,
            authoritative_economics_input as polymarket_authoritative_economics_input,
        },
    },
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequestInput, BoltV3SubmitIntentKind, OrderValuationContext,
        build_submit_admission_request_from_economics,
    },
    economics::{
        AccountId, CurrencyId, DecisionCorrelationId, EconomicsInstrumentId, EconomicsQuoteRequest,
        EdgeBasisPolicyId, ExecutionClientId, LifecyclePath, LiquidityRole, OrderSide,
        PlannedFillLeg, PositionContext, PositionId, PositionSide, ProductSurfaceId,
        ReportingPolicyId, RoutingContext, SnapshotId, SourceIdentity, VenueEconomicsUnavailable,
    },
    integrations::nautilus::economics::NautilusEstimateLiquidityRole,
};
use nautilus_model::{
    enums::{OrderSide as NautilusOrderSide, TimeInForce},
    identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
    orders::{LimitOrder, Order, OrderAny},
    types::{Price, Quantity},
};
use rust_decimal::Decimal;

fn id<T>(
    value: &str,
    constructor: impl FnOnce(String) -> Result<T, bolt_v2::economics::EconomicsError>,
) -> T {
    constructor(value.to_string()).expect("test identifier should be canonical")
}

fn order_binding() -> EconomicsOrderBinding {
    EconomicsOrderBinding::from_sha256([1; 32])
}

fn quote_request(instrument_id: &str, product_surface_id: &str) -> EconomicsQuoteRequest {
    EconomicsQuoteRequest {
        execution_client_id: id("polymarket_main", ExecutionClientId::try_new),
        account_id: id("POLYMARKET-001", AccountId::try_new),
        instrument_id: id(instrument_id, EconomicsInstrumentId::try_new),
        product_surface_id: id(product_surface_id, ProductSurfaceId::try_new),
        order_side: OrderSide::Buy,
        liquidity_role: LiquidityRole::Taker,
        planned_fill_legs: vec![PlannedFillLeg {
            price: Decimal::new(5, 1),
            quantity: Decimal::TEN,
        }],
        routing: RoutingContext {
            attached_charge: None,
        },
        position: None,
        lifecycle_path: LifecyclePath::PlannedExit,
        reporting_policy_id: id("primary-pnl", ReportingPolicyId::try_new),
        reporting_currency: id("USD", CurrencyId::try_new),
        edge_basis_policy_id: id("primary", EdgeBasisPolicyId::try_new),
        requested_at_ns: 1_000,
        decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
    }
}

fn loaded() -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture config should load")
}

fn configured_economics(
    loaded: &bolt_v2::bolt_v3_config::LoadedBoltV3Config,
) -> ExecutionEconomicsConfig {
    loaded.root.clients["polymarket_main"]
        .execution
        .clone()
        .expect("fixture execution config should exist")
        .try_into::<PolymarketExecutionConfig>()
        .expect("fixture execution config should parse")
        .economics
        .expect("fixture economics config should exist")
}

fn authoritative_input() -> AuthoritativeVenueEconomicsInput {
    authoritative_input_with_valuation_deadline(2_000)
}

fn authoritative_input_with_valuation_deadline(
    valid_until_ns: u64,
) -> AuthoritativeVenueEconomicsInput {
    authoritative_input_without_valuation().with_valuation_observations([
        AuthoritativeValuationObservation::ProviderExactConversion {
            source_id: id("fixture-collateral", SourceIdentity::try_new),
            from_unit: id("pUSD", CurrencyId::try_new),
            to_unit: id("USD", CurrencyId::try_new),
            snapshot_id: id("collateral-conversion-1", SnapshotId::try_new),
            observed_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns,
        },
    ])
}

fn authoritative_input_without_valuation() -> AuthoritativeVenueEconomicsInput {
    authoritative_input_without_valuation_for("token-yes.POLYMARKET", "token-yes")
}

fn authoritative_input_without_valuation_for(
    instrument_id: &str,
    provider_instrument_id: &str,
) -> AuthoritativeVenueEconomicsInput {
    let snapshot = PolymarketMarketInfoSnapshot::from_json(
        PolymarketSnapshotMetadata {
            snapshot_id: id("market-info-1", SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 2_000,
            builder_attachment_id: None,
        },
        include_str!("fixtures/economics/polymarket/fee_enabled.json"),
    )
    .expect("test market-info snapshot should parse");
    polymarket_authoritative_economics_input(
        "polymarket_main",
        instrument_id,
        "binary_outcome",
        provider_instrument_id,
        snapshot,
    )
    .expect("Polymarket authority scope should match its market snapshot")
}

fn authoritative_refresh_input(
    snapshot_id: &str,
    market_info_json: &str,
    valid_until_ns: u64,
) -> AuthoritativeVenueEconomicsInput {
    let snapshot = PolymarketMarketInfoSnapshot::from_json(
        PolymarketSnapshotMetadata {
            snapshot_id: id(snapshot_id, SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns,
            builder_attachment_id: None,
        },
        market_info_json,
    )
    .expect("refresh market-info snapshot should parse");
    polymarket_authoritative_economics_input(
        "polymarket_main",
        "token-yes.POLYMARKET",
        "binary_outcome",
        "token-yes",
        snapshot,
    )
    .expect("refresh authority scope should match its market snapshot")
    .with_valuation_observations([
        AuthoritativeValuationObservation::ProviderExactConversion {
            source_id: id("fixture-collateral", SourceIdentity::try_new),
            from_unit: id("pUSD", CurrencyId::try_new),
            to_unit: id("USD", CurrencyId::try_new),
            snapshot_id: id("collateral-refresh", SnapshotId::try_new),
            observed_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns,
        },
    ])
}

fn maker_admission_for_refresh(
    bound: &bolt_v2::bolt_v3_economics_runtime::BoundExecutionEconomics,
) -> bolt_v2::bolt_v3_economics_runtime::EconomicsAdmission {
    let mut request = quote_request("token-yes.POLYMARKET", "binary_outcome");
    request.liquidity_role = LiquidityRole::GuaranteedMaker;
    request.requested_at_ns = 1_000_000_000;
    bound
        .quote_admission(EconomicsAdmissionIntent {
            request,
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("fresh maker economics should quote")
}

fn polymarket_limit_order() -> OrderAny {
    OrderAny::Limit(
        LimitOrder::new_checked(
            TraderId::from("TRADER-001"),
            StrategyId::from("strategy-a"),
            InstrumentId::from("token-yes.POLYMARKET"),
            ClientOrderId::from("O-19700101-000000-001-E1-1"),
            NautilusOrderSide::Buy,
            Quantity::new(10.0, 2),
            Price::new(0.50, 2),
            TimeInForce::Gtc,
            None,
            false,
            false,
            false,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_u64),
        )
        .expect("fixture final order should be valid"),
    )
}

fn hyperliquid_metadata(source: &str, snapshot_id: &str) -> HyperliquidSnapshotMetadata {
    HyperliquidSnapshotMetadata {
        source: id(source, bolt_v2::economics::SourceIdentity::try_new),
        snapshot_id: id(snapshot_id, SnapshotId::try_new),
        source_at_ns: 900,
        fetched_at_ns: 950,
        valid_until_ns: 2_000,
    }
}

fn hyperliquid_input() -> AuthoritativeVenueEconomicsInput {
    hyperliquid_input_for(
        "HYPERLIQUID-001",
        "meta-and-asset-contexts",
        "funding-context",
    )
}

fn hyperliquid_input_for(
    account_id: &str,
    product_metadata_source: &str,
    funding_source: &str,
) -> AuthoritativeVenueEconomicsInput {
    let user_fees = HyperliquidUserFeesSnapshot::from_json(
        hyperliquid_metadata("user-fees", "fees-1"),
        id(account_id, AccountId::try_new),
        include_str!("fixtures/economics/hyperliquid/user_fees_discounted.json"),
    )
    .expect("test user-fees authority should parse");
    let product =
        HyperliquidProductEconomicsSnapshot::perp_from_json(HyperliquidPerpetualSnapshotInput {
            metadata: hyperliquid_metadata(product_metadata_source, "perp-1"),
            instrument_id: id("BTC-PERP.HYPERLIQUID", EconomicsInstrumentId::try_new),
            product_surface_id: id("standard_perps", ProductSurfaceId::try_new),
            deployer_fee_scale: Decimal::ZERO,
            growth_mode: false,
            aligned_quote_json: None,
            context_metadata: hyperliquid_metadata(funding_source, "funding-1"),
            context_json: include_str!("fixtures/economics/hyperliquid/perp_context.json"),
        })
        .expect("test product authority should parse");
    hyperliquid_authoritative_economics_input(
        "hyperliquid_offline",
        "BTC-PERP.HYPERLIQUID",
        "standard_perps",
        user_fees,
        product,
        None,
    )
    .expect("Hyperliquid authority scope should match its product snapshot")
    .with_valuation_observations([
        AuthoritativeValuationObservation::ProviderExactConversion {
            source_id: id("fixture-settlement", SourceIdentity::try_new),
            from_unit: id("hUSD", CurrencyId::try_new),
            to_unit: id("USD", CurrencyId::try_new),
            snapshot_id: id("settlement-conversion-1", SnapshotId::try_new),
            observed_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 2_000,
        },
    ])
}

fn hyperliquid_refresh_input(
    user_add_rate: &str,
    snapshot_suffix: &str,
    valid_until_ns: u64,
) -> AuthoritativeVenueEconomicsInput {
    let metadata = |source: &str, snapshot: &str| HyperliquidSnapshotMetadata {
        source: id(source, SourceIdentity::try_new),
        snapshot_id: id(
            &format!("{snapshot}-{snapshot_suffix}"),
            SnapshotId::try_new,
        ),
        source_at_ns: 900,
        fetched_at_ns: 950,
        valid_until_ns,
    };
    let mut user_fees_json = serde_json::from_str::<serde_json::Value>(include_str!(
        "fixtures/economics/hyperliquid/user_fees_discounted.json"
    ))
    .expect("fixture user fees should parse");
    user_fees_json["userAddRate"] = serde_json::Value::String(user_add_rate.to_string());
    let user_fees = HyperliquidUserFeesSnapshot::from_json(
        metadata("user-fees", "fees"),
        id("HYPERLIQUID-001", AccountId::try_new),
        &serde_json::to_string(&user_fees_json).expect("changed user fees should serialize"),
    )
    .expect("changed user fees should parse");
    let product =
        HyperliquidProductEconomicsSnapshot::perp_from_json(HyperliquidPerpetualSnapshotInput {
            metadata: metadata("meta-and-asset-contexts", "perp"),
            instrument_id: id("BTC-PERP.HYPERLIQUID", EconomicsInstrumentId::try_new),
            product_surface_id: id("standard_perps", ProductSurfaceId::try_new),
            deployer_fee_scale: Decimal::ZERO,
            growth_mode: false,
            aligned_quote_json: None,
            context_metadata: metadata("funding-context", "funding"),
            context_json: include_str!("fixtures/economics/hyperliquid/perp_context.json"),
        })
        .expect("refresh product authority should parse");
    hyperliquid_authoritative_economics_input(
        "hyperliquid_offline",
        "BTC-PERP.HYPERLIQUID",
        "standard_perps",
        user_fees,
        product,
        None,
    )
    .expect("refresh Hyperliquid scope should match")
    .with_valuation_observations([
        AuthoritativeValuationObservation::ProviderExactConversion {
            source_id: id("fixture-settlement", SourceIdentity::try_new),
            from_unit: id("hUSD", CurrencyId::try_new),
            to_unit: id("USD", CurrencyId::try_new),
            snapshot_id: id(
                &format!("settlement-{snapshot_suffix}"),
                SnapshotId::try_new,
            ),
            observed_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns,
        },
    ])
}

fn hyperliquid_maker_admission_for_refresh(
    bound: &bolt_v2::bolt_v3_economics_runtime::BoundExecutionEconomics,
) -> bolt_v2::bolt_v3_economics_runtime::EconomicsAdmission {
    let mut request = hyperliquid_quote_request();
    request.liquidity_role = LiquidityRole::GuaranteedMaker;
    request.requested_at_ns = 1_000_000_000;
    bound
        .quote_admission(EconomicsAdmissionIntent {
            request,
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("fresh Hyperliquid maker economics should quote")
}

fn hyperliquid_loaded() -> bolt_v2::bolt_v3_config::LoadedBoltV3Config {
    let mut loaded = loaded();
    let execution = toml::from_str::<toml::Value>(include_str!(
        "fixtures/economics/hyperliquid/execution.toml"
    ))
    .expect("test Hyperliquid execution TOML should parse");
    loaded.root.clients.insert(
        "hyperliquid_offline".to_string(),
        ClientBlock {
            venue: nautilus_model::identifiers::Venue::from("HYPERLIQUID"),
            data: None,
            execution: Some(execution),
            secrets: None,
            readiness_probe: None,
        },
    );
    loaded
}

fn hyperliquid_quote_request() -> EconomicsQuoteRequest {
    let mut request = quote_request("BTC-PERP.HYPERLIQUID", "standard_perps");
    request.execution_client_id = id("hyperliquid_offline", ExecutionClientId::try_new);
    request.account_id = id("HYPERLIQUID-001", AccountId::try_new);
    request.position = Some(PositionContext {
        position_id: id("position", PositionId::try_new),
        side: PositionSide::Long,
        quantity: Decimal::TEN,
        holding_horizon_ns: 250,
    });
    request
}

#[test]
fn execution_economics_binds_one_matching_toml_authority() {
    let loaded = loaded();
    let config = configured_economics(&loaded);
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");

    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching provider, client, and TOML should bind");

    assert_eq!(bound.execution_client_id(), "polymarket_main");
    assert_eq!(bound.provider_key(), "POLYMARKET");
    assert_eq!(bound.config(), &config);
}

#[test]
fn final_nautilus_order_routes_through_its_exact_provider_authority() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");
    let handle = BoltV3OrderEconomicsHandle::new(bound);
    let order = polymarket_limit_order();
    let intent = order_intent_details_from_compiled_order(
        "strategy-a".to_string(),
        "0.50".to_string(),
        &order,
    );
    let admission_input = BoltV3SubmitAdmissionRequestInput {
        execution_client_id: "polymarket_main",
        intent: &intent,
        intent_kind: BoltV3SubmitIntentKind::Entry,
        order: &order,
        valuation: OrderValuationContext::empty(),
        risk_reducing_exit_position: None,
    };

    let admission = handle
        .quote_admission(BoltV3OrderEconomicsIntent {
            request: &admission_input,
            planned_fill_legs: vec![BoltV3PlannedFillLeg {
                price: Decimal::new(5, 1),
                quantity: Decimal::TEN,
            }],
            liquidity_role: NautilusEstimateLiquidityRole::Taker,
            position: None,
            lifecycle_path: LifecyclePath::HoldToRedemption,
            requested_at_ns: 1_000,
            decision_correlation_id: "decision-final-order",
            gross_expected_value: Decimal::ONE,
            minimum_core_edge_ratio: Decimal::ZERO,
        })
        .expect("the exact final order identity should reach its provider quote");

    let sealed = build_submit_admission_request_from_economics(admission_input, admission)
        .expect("shared admission should consume the exact economics result");

    assert_eq!(
        sealed.economics().request().instrument_id.as_str(),
        "token-yes.POLYMARKET"
    );
    assert_eq!(
        sealed.economics().request().account_id.as_str(),
        "POLYMARKET-001"
    );
    assert_eq!(sealed.economics().request().planned_fill_legs.len(), 1);
    assert_eq!(sealed.economics().reservation_basis(), Decimal::from(5));
    assert_eq!(
        sealed.request().notional,
        sealed.economics().full_reservation_liability()
    );
    assert!(sealed.request().notional > Decimal::from(5));

    let mut changed = order.clone();
    changed.set_quantity(Quantity::new(5.0, 2));
    changed.set_leaves_qty(Quantity::new(5.0, 2));
    assert!(
        sealed
            .validate_final_order(&changed, "polymarket_main")
            .is_err(),
        "an order mutation after quoting must invalidate the sealed authority"
    );
}

#[test]
fn bound_execution_economics_routes_edge_basis_by_exact_product_scope() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");
    let request = quote_request("token-yes.POLYMARKET", "binary_outcome");
    let admission = bound
        .quote_admission(EconomicsAdmissionIntent {
            request,
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("exact scope should resolve its venue-owned edge basis");
    let basis = &admission.net_edge().basis;

    assert_eq!(basis.normalized_amount.amount(), Decimal::from(5));
    assert_eq!(basis.resolver_id.as_str(), "product-metadata");
    assert_eq!(
        basis.product_metadata_source.as_str(),
        "polymarket-market-info"
    );
}

#[test]
fn bound_execution_economics_quotes_and_folds_admission_from_one_authority() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");

    let admission = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: quote_request("token-yes.POLYMARKET", "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("fresh valued economics should authorize positive net edge");

    assert!(admission.quote().core_total().is_sign_negative());
    assert!(admission.net_edge().core_net_edge > Decimal::ZERO);
    assert!(admission.full_reservation_liability() > Decimal::from(5));
    assert_eq!(
        admission.quote().components()[0]
            .point_valuation()
            .expect("fee-bearing component must retain its point valuation")
            .source_snapshot_ids,
        vec![id("collateral-conversion-1", SnapshotId::try_new)]
    );
    assert_eq!(
        admission
            .quote()
            .source_snapshot_ids()
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>(),
        vec!["collateral-conversion-1", "market-info-1"]
    );
}

#[test]
fn bound_execution_economics_enforces_the_declared_minimum_net_edge() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");

    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: quote_request("token-yes.POLYMARKET", "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::new(2, 1),
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("fees must not leave the order below its declared minimum edge");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::CoreEdgeBelowMinimum {
            minimum_core_edge_ratio,
            actual_core_edge_ratio,
        } if minimum_core_edge_ratio == Decimal::new(2, 1)
            && actual_core_edge_ratio < minimum_core_edge_ratio
    ));
}

#[test]
fn bound_execution_economics_rejects_stale_required_valuation() {
    let loaded = loaded();
    let inputs =
        AuthoritativeEconomicsInputStore::try_new([authoritative_input_with_valuation_deadline(
            999,
        )])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("authority shape should bind before quote-time freshness evaluation");

    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: quote_request("token-yes.POLYMARKET", "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("stale required valuation must fail before admission");

    assert!(error.to_string().contains("stale"));
}

#[test]
fn execution_economics_rejects_missing_required_valuation_authority() {
    let loaded = loaded();
    let inputs =
        AuthoritativeEconomicsInputStore::try_new([authoritative_input_without_valuation()])
            .expect("one input should construct");

    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("TOML authority should bind before a runtime scope is published");
    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: quote_request("token-yes.POLYMARKET", "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("missing configured valuation authority must fail at quote time");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::AuthorityBinding(
            EconomicsRuntimeBindingError::AuthoritativeValuationBuildFailed { .. }
        )
    ));
}

#[test]
fn bound_execution_economics_rejects_foreign_reporting_policy() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");
    let mut request = quote_request("token-yes.POLYMARKET", "binary_outcome");
    request.reporting_policy_id = id("foreign-reporting-policy", ReportingPolicyId::try_new);

    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request,
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("the request cannot select a foreign reporting policy");

    assert!(error.to_string().contains("reporting"));
}

#[test]
fn bound_execution_economics_rejects_foreign_product_edge_policy() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");
    let mut request = quote_request("token-yes.POLYMARKET", "binary_outcome");
    request.edge_basis_policy_id = id("foreign-edge-policy", EdgeBasisPolicyId::try_new);

    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request,
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("the request cannot select a foreign edge policy");

    assert!(error.to_string().contains("edge-basis"));
}

#[test]
fn execution_economics_rotates_authoritative_scopes_without_rebinding() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::default();
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("TOML authority should bind before the first runtime snapshot");
    let quote = |instrument_id: &str| {
        bound.quote_admission(EconomicsAdmissionIntent {
            request: quote_request(instrument_id, "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
    };

    assert!(matches!(
        quote("token-yes.POLYMARKET"),
        Err(
            bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::Venue(
                VenueEconomicsUnavailable::MissingAuthoritativeSnapshot
            )
        )
    ));

    inputs
        .replace_execution_client("polymarket_main", [authoritative_input()])
        .expect("publishing the first scope should succeed");
    quote("token-yes.POLYMARKET").expect("the published scope should quote");

    let replacement = authoritative_input_without_valuation_for("token-no.POLYMARKET", "token-no")
        .with_valuation_observations([
            AuthoritativeValuationObservation::ProviderExactConversion {
                source_id: id("fixture-collateral", SourceIdentity::try_new),
                from_unit: id("pUSD", CurrencyId::try_new),
                to_unit: id("USD", CurrencyId::try_new),
                snapshot_id: id("collateral-conversion-2", SnapshotId::try_new),
                observed_at_ns: 900,
                fetched_at_ns: 950,
                valid_until_ns: 2_000,
            },
        ]);
    inputs
        .replace_execution_client("polymarket_main", [replacement])
        .expect("rotating the execution-client scope should be atomic");

    assert!(matches!(
        quote("token-yes.POLYMARKET"),
        Err(
            bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::Venue(
                VenueEconomicsUnavailable::MissingAuthoritativeSnapshot
            )
        )
    ));
    quote("token-no.POLYMARKET").expect("the replacement scope should quote immediately");
}

#[test]
fn resting_maker_economics_refreshes_before_expiry_without_changing_terms() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_refresh_input(
        "market-refresh-1",
        include_str!("fixtures/economics/polymarket/fee_enabled.json"),
        40_000_000_000,
    )])
    .expect("one refresh authority should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("refresh authority should bind");
    let prior = maker_admission_for_refresh(&bound);

    assert_eq!(
        refresh_resting_order_economics(
            &bound,
            &prior,
            Decimal::from(5),
            Decimal::TEN,
            true,
            25_000_000_000,
        ),
        RestingOrderEconomicsRefresh::NotDue
    );
    let RestingOrderEconomicsRefresh::Refreshed(refreshed) = refresh_resting_order_economics(
        &bound,
        &prior,
        Decimal::from(5),
        Decimal::TEN,
        true,
        26_000_000_000,
    ) else {
        panic!("unchanged authoritative terms should refresh the resting order");
    };
    assert_eq!(refreshed.request().requested_at_ns, 26_000_000_000);
    assert_eq!(
        refreshed.request().planned_fill_legs[0].quantity,
        Decimal::from(5)
    );
    assert_eq!(refreshed.reservation_basis(), Decimal::new(25, 1));
    assert_eq!(refreshed.order_binding(), prior.order_binding());
}

#[test]
fn resting_maker_economics_cancels_on_lost_or_unavailable_authority() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_refresh_input(
        "market-refresh-1",
        include_str!("fixtures/economics/polymarket/fee_enabled.json"),
        40_000_000_000,
    )])
    .expect("one refresh authority should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("refresh authority should bind");
    let prior = maker_admission_for_refresh(&bound);

    assert_eq!(
        refresh_resting_order_economics(
            &bound,
            &prior,
            Decimal::TEN,
            Decimal::TEN,
            false,
            26_000_000_000,
        ),
        RestingOrderEconomicsRefresh::CancelRequired(
            RestingOrderEconomicsCancelReason::MakerGuaranteeLost
        )
    );
    assert_eq!(
        refresh_resting_order_economics(
            &bound,
            &prior,
            Decimal::TEN,
            Decimal::TEN,
            true,
            prior.quote().valid_until_ns() + 1,
        ),
        RestingOrderEconomicsRefresh::CancelRequired(
            RestingOrderEconomicsCancelReason::QuoteUnavailable
        )
    );

    inputs
        .replace_execution_client("polymarket_main", [])
        .expect("retiring authority should publish atomically");
    assert_eq!(
        refresh_resting_order_economics(
            &bound,
            &prior,
            Decimal::TEN,
            Decimal::TEN,
            true,
            26_000_000_000,
        ),
        RestingOrderEconomicsRefresh::CancelRequired(
            RestingOrderEconomicsCancelReason::QuoteUnavailable
        )
    );
}

#[test]
fn resting_maker_economics_cancels_when_provider_terms_change() {
    let loaded = hyperliquid_loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([hyperliquid_refresh_input(
        "-0.00001",
        "one",
        40_000_000_000,
    )])
    .expect("one Hyperliquid refresh authority should construct");
    let bound = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .expect("Hyperliquid refresh authority should bind");
    let prior = hyperliquid_maker_admission_for_refresh(&bound);

    inputs
        .replace_execution_client(
            "hyperliquid_offline",
            [hyperliquid_refresh_input("-0.00002", "two", 40_000_000_000)],
        )
        .expect("changed Hyperliquid authority should publish atomically");
    assert_eq!(
        refresh_resting_order_economics(
            &bound,
            &prior,
            Decimal::TEN,
            Decimal::TEN,
            true,
            26_000_000_000,
        ),
        RestingOrderEconomicsRefresh::CancelRequired(
            RestingOrderEconomicsCancelReason::TermsChanged
        )
    );
}

#[test]
fn execution_economics_rejects_duplicate_authoritative_inputs() {
    let error =
        AuthoritativeEconomicsInputStore::try_new([authoritative_input(), authoritative_input()])
            .err()
            .expect("duplicate client authority must fail closed");

    assert_eq!(
        error,
        EconomicsRuntimeBindingError::DuplicateAuthoritativeInput {
            execution_client_id: "polymarket_main".to_string(),
            instrument_id: "token-yes.POLYMARKET".to_string(),
            product_surface_id: "binary_outcome".to_string(),
        }
    );
}

#[test]
fn execution_economics_rejects_unsupported_provider() {
    let mut loaded = loaded();
    loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .unwrap()
        .venue = nautilus_model::identifiers::Venue::from("UNREGISTERED");

    let error = bind_execution_economics(
        &loaded,
        "polymarket_main",
        &AuthoritativeEconomicsInputStore::default(),
    )
    .err()
    .expect("unregistered provider must fail closed");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::UnsupportedProvider { .. }
    ));
}

#[test]
fn execution_economics_builds_the_adapter_from_the_loaded_toml() {
    let mut loaded = loaded();
    loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture client should exist")
        .execution
        .as_mut()
        .expect("fixture execution block should exist")["economics"]["quote_components"]["platform"]
        ["component_id"] = toml::Value::String("configured-platform".to_string());
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("loaded TOML and raw market authority should construct the adapter");

    let admission = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: quote_request("token-yes.POLYMARKET", "binary_outcome"),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("configured fee-bearing authority should quote");
    let components = admission.quote().components();

    assert_eq!(components.len(), 1);
    assert_eq!(
        components[0].component().component_id.as_str(),
        "configured-platform"
    );
}

#[test]
fn hyperliquid_execution_economics_binds_from_offline_toml_and_raw_authority() {
    let loaded = hyperliquid_loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([hyperliquid_input()])
        .expect("one Hyperliquid input should construct");

    let bound = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .expect("offline Hyperliquid TOML and raw authority should bind");
    let admission = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: hyperliquid_quote_request(),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("exact Hyperliquid scope should resolve its edge basis");
    let basis = &admission.net_edge().basis;

    assert_eq!(bound.provider_key(), "HYPERLIQUID");
    assert_eq!(basis.normalized_amount.amount(), Decimal::from(5));
    assert_eq!(
        basis.product_metadata_source.as_str(),
        "meta-and-asset-contexts"
    );
}

#[test]
fn hyperliquid_execution_economics_rejects_foreign_account_authority() {
    let loaded = hyperliquid_loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([hyperliquid_input_for(
        "FOREIGN-001",
        "meta-and-asset-contexts",
        "funding-context",
    )])
    .expect("one Hyperliquid input should construct");

    let bound = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .expect("TOML authority should bind before the runtime scope is evaluated");
    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: hyperliquid_quote_request(),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("foreign account authority must fail closed");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::AuthorityBinding(
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
        )
    ));
}

#[test]
fn hyperliquid_execution_economics_rejects_mismatched_product_source() {
    let loaded = hyperliquid_loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([hyperliquid_input_for(
        "HYPERLIQUID-001",
        "foreign-product-source",
        "funding-context",
    )])
    .expect("one Hyperliquid input should construct");

    let bound = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .expect("TOML authority should bind before the runtime scope is evaluated");
    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: hyperliquid_quote_request(),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("mismatched product authority must fail closed");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::AuthorityBinding(
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
        )
    ));
}

#[test]
fn hyperliquid_execution_economics_rejects_mismatched_funding_source() {
    let loaded = hyperliquid_loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([hyperliquid_input_for(
        "HYPERLIQUID-001",
        "meta-and-asset-contexts",
        "foreign-funding-source",
    )])
    .expect("one Hyperliquid input should construct");

    let bound = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .expect("TOML authority should bind before the runtime scope is evaluated");
    let error = bound
        .quote_admission(EconomicsAdmissionIntent {
            request: hyperliquid_quote_request(),
            order_binding: order_binding(),
            policy: EconomicsAdmissionPolicy::TradingEdge {
                minimum_core_edge_ratio: Decimal::ZERO,
            },
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("mismatched funding authority must fail closed");

    assert!(matches!(
        error,
        bolt_v2::bolt_v3_economics_runtime::EconomicsAdmissionError::AuthorityBinding(
            EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
        )
    ));
}
