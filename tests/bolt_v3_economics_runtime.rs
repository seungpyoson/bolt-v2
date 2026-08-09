use crate::support;

use bolt_v2::{
    bolt_v3_config::{ExecutionEconomicsConfig, load_bolt_v3_config},
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeVenueEconomicsInput,
        EconomicsRuntimeBindingError, bind_execution_economics,
    },
    bolt_v3_providers::polymarket::{
        PolymarketExecutionConfig, PolymarketMarketInfoSnapshot, PolymarketSnapshotMetadata,
        authoritative_economics_input,
    },
    economics::{
        AccountId, CurrencyId, DecisionCorrelationId, EconomicsInstrumentId, EconomicsQuoteRequest,
        EdgeBasisPolicyId, ExecutionClientId, LifecyclePath, LiquidityRole, OrderSide,
        PlannedFillLeg, PlannedFillNotional, ProductSurfaceId, ReportingPolicyId, RoutingContext,
        SnapshotId,
    },
};
use rust_decimal::Decimal;

fn id<T>(
    value: &str,
    constructor: impl FnOnce(String) -> Result<T, bolt_v2::economics::EconomicsError>,
) -> T {
    constructor(value.to_string()).expect("test identifier should be canonical")
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
    authoritative_economics_input("polymarket_main", "token-yes", "binary_outcome", snapshot)
        .expect("Polymarket authority scope should match its market snapshot")
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
fn bound_execution_economics_routes_edge_basis_by_exact_product_scope() {
    let loaded = loaded();
    let inputs = AuthoritativeEconomicsInputStore::try_new([authoritative_input()])
        .expect("one input should construct");
    let bound = bind_execution_economics(&loaded, "polymarket_main", &inputs)
        .expect("matching authority should bind");
    let request = quote_request("token-yes", "binary_outcome");
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
            instrument_id: "token-yes".to_string(),
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
        .quote(&quote_request("token-yes", "binary_outcome"))
        .expect("configured fee-bearing authority should quote");

    assert_eq!(estimate.components.len(), 1);
    assert_eq!(
        estimate.components[0].component_id.as_str(),
        "configured-platform"
    );
}
