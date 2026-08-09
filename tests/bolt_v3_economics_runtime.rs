use crate::support;

use bolt_v2::{
    bolt_v3_config::{ClientBlock, ExecutionEconomicsConfig, load_bolt_v3_config},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        AuthoritativeVenueEconomicsInput, EconomicsAdmissionIntent, EconomicsAdmissionPurpose,
        EconomicsOrderBinding, EconomicsRuntimeBindingError, bind_execution_economics,
    },
    bolt_v3_order_execution::{
        BoltV3OrderEconomicsHandle, BoltV3OrderEconomicsIntent, BoltV3PlannedFillLeg,
        order_intent_details_from_compiled_order,
    },
    bolt_v3_providers::{
        hyperliquid::{
            HyperliquidProductEconomicsSnapshot, HyperliquidSnapshotMetadata,
            HyperliquidUserFeesSnapshot,
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
        PlannedFillLeg, PlannedFillNotional, ProductSurfaceId, ReportingPolicyId, RoutingContext,
        SnapshotId, SourceIdentity,
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
        account_id: id("account", AccountId::try_new),
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
        "token-yes.POLYMARKET",
        "binary_outcome",
        "token-yes",
        snapshot,
    )
    .expect("Polymarket authority scope should match its market snapshot")
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
    let product = HyperliquidProductEconomicsSnapshot::perp_from_json(
        hyperliquid_metadata(product_metadata_source, "perp-1"),
        id("BTC-PERP.HYPERLIQUID", EconomicsInstrumentId::try_new),
        id("standard_perps", ProductSurfaceId::try_new),
        Decimal::ZERO,
        false,
        None,
        hyperliquid_metadata(funding_source, "funding-1"),
        include_str!("fixtures/economics/hyperliquid/perp_context.json"),
    )
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
    assert_eq!(bound.account_id().as_str(), "POLYMARKET-001");
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
    let notional = PlannedFillNotional::from_legs(&request.planned_fill_legs)
        .expect("test fill should have positive notional");

    let basis = bound
        .adapter()
        .resolve_edge_basis(&request, notional)
        .expect("exact scope should resolve its venue-owned edge basis");

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
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect("fresh valued economics should authorize positive net edge");

    assert!(admission.quote().core_total().is_sign_negative());
    assert!(admission.net_edge().core_net_edge > Decimal::ZERO);
    assert!(admission.full_reservation_liability() > Decimal::from(5));
    assert_eq!(
        admission.quote().valuations()[0].source_snapshot_ids,
        vec![id("collateral-conversion-1", SnapshotId::try_new)]
    );
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
            purpose: EconomicsAdmissionPurpose::TradingEdge,
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

    let error = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .err()
        .expect("missing configured valuation authority must fail binding");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::AuthoritativeValuationBuildFailed { .. }
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
            purpose: EconomicsAdmissionPurpose::TradingEdge,
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
            purpose: EconomicsAdmissionPurpose::TradingEdge,
            gross_expected_value: Decimal::ONE,
            reservation_basis: Decimal::from(5),
        })
        .expect_err("the request cannot select a foreign edge policy");

    assert!(error.to_string().contains("edge-basis"));
}

#[test]
fn execution_economics_rejects_missing_authoritative_input() {
    let loaded = loaded();
    let error = bind_execution_economics(
        &loaded,
        "polymarket_main",
        &AuthoritativeEconomicsInputStore::default(),
    )
    .err()
    .expect("missing authoritative input must fail closed");

    assert_eq!(
        error,
        EconomicsRuntimeBindingError::MissingAuthoritativeInput {
            execution_client_id: "polymarket_main".to_string(),
        }
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
fn execution_economics_rejects_provider_mismatch() {
    let mut loaded = loaded();
    loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .expect("fixture client should exist")
        .venue = nautilus_model::identifiers::Venue::from("HYPERLIQUID");
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");

    let error = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .err()
        .expect("foreign provider authority must fail closed");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::AuthoritativeProviderMismatch { .. }
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

    let estimate = bound
        .adapter()
        .quote(&quote_request("token-yes.POLYMARKET", "binary_outcome"))
        .expect("configured fee-bearing authority should quote");

    assert_eq!(estimate.components.len(), 1);
    assert_eq!(
        estimate.components[0].component_id.as_str(),
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
    let request = hyperliquid_quote_request();
    let notional = PlannedFillNotional::from_legs(&request.planned_fill_legs)
        .expect("test fill should have positive notional");
    let basis = bound
        .adapter()
        .resolve_edge_basis(&request, notional)
        .expect("exact Hyperliquid scope should resolve its edge basis");

    assert_eq!(bound.provider_key(), "HYPERLIQUID");
    assert_eq!(bound.account_id().as_str(), "HYPERLIQUID-001");
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

    let error = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .err()
        .expect("foreign account authority must fail closed");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
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

    let error = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .err()
        .expect("mismatched product authority must fail closed");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
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

    let error = bind_execution_economics(&loaded, "hyperliquid_offline", &inputs)
        .err()
        .expect("mismatched funding authority must fail closed");

    assert!(matches!(
        error,
        EconomicsRuntimeBindingError::AuthoritativeInputBuildFailed { .. }
    ));
}
