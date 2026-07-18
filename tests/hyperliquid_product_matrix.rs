//! Hyperliquid product-surface matrix tests.
//!
//! The matrix is the operator-facing boundary between "we can discover this
//! product" and "we may submit live orders". Discovery evidence must not open
//! submit.

use crate::support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::bolt_v3_adapters::{
    BoltV3AdapterMappingError, map_bolt_v3_adapters_with_market_identity_and_runtime_approvals,
};
use bolt_v2::bolt_v3_config::{ClientBlock, LoadedBoltV3Config, load_bolt_v3_config};
use bolt_v2::bolt_v3_market_families::{
    MarketIdentityPlan,
    hyperliquid_instrument::{HyperliquidInstrumentTargetPlan, ProductSurface},
};
use bolt_v2::bolt_v3_providers::ProviderRuntimeApprovals;
use bolt_v2::bolt_v3_providers::hyperliquid::{
    HyperliquidDiscoveryStatus, HyperliquidProductMatrixEntry, HyperliquidProductSurface,
    HyperliquidSubmitStatus, ResolvedBoltV3HyperliquidSecrets, hyperliquid_product_matrix,
};
use bolt_v2::bolt_v3_providers::hyperliquid_artifacts::write_hyperliquid_product_matrix_artifact;
use bolt_v2::bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets};
use nautilus_hyperliquid::http::query::InfoRequest;
use nautilus_model::identifiers::InstrumentId;
use rust_decimal::Decimal;
use zeroize::Zeroizing;

fn assert_info_request_type(request: InfoRequest, expected_type: &str, context: &str) {
    let request_json = serde_json::to_value(&request).expect(context);
    assert_eq!(request_json["type"], expected_type);
}

fn product_entry(surface: HyperliquidProductSurface) -> &'static HyperliquidProductMatrixEntry {
    hyperliquid_product_matrix()
        .iter()
        .find(|entry| entry.product_surface == surface)
        .expect("surface must be listed in Hyperliquid product matrix")
}

fn assert_sources(entry: &HyperliquidProductMatrixEntry, expected_sources: &[&str]) {
    for expected_source in expected_sources {
        assert!(
            entry
                .discovery_sources
                .iter()
                .any(|source| source == expected_source),
            "matrix missing discovery source {expected_source}"
        );
    }
}

fn assert_approval_gated(entry: &HyperliquidProductMatrixEntry) {
    assert_eq!(
        entry.discovery_status,
        HyperliquidDiscoveryStatus::Supported
    );
    assert_eq!(
        entry.live_submit_status,
        HyperliquidSubmitStatus::ApprovalGated
    );
    assert!(
        entry.missing_submit_proof.is_empty(),
        "approval-gated matrix entries should not advertise stale missing submit proof gaps"
    );
}

fn assert_serializes_approval_gated(entry: &HyperliquidProductMatrixEntry) {
    let value = serde_json::to_value(entry).expect("matrix entry should serialize");
    assert_eq!(
        value["live_submit_status"], "approval_gated",
        "operator matrix must reflect approval-gated live submit once product proof binding is verified"
    );
    assert_eq!(
        value["missing_submit_proof"],
        serde_json::json!([]),
        "approval-gated surfaces should not advertise stale missing proof gaps"
    );
}

fn hyperliquid_execution_client_for_surface(surface: &str) -> ClientBlock {
    let outcome_settlement_poll_secs = if surface == "hip4_outcomes" { 5 } else { 0 };
    toml::from_str(&format!(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["{surface}"]
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
include_builder_attribution = false
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = {outcome_settlement_poll_secs}

[execution.economics]
economics_slice = "quote_only"
routing_attachment_policy = "forbidden"
reporting_policy = "primary-pnl"
quote_refresh_secs = 30
refresh_max_concurrency = 8
quote_max_age_secs = 60
quote_validity_ms = 30000
resting_order_refresh_margin_ms = 5000
carry_surfaces = []

[execution.economics.sources]
account_fees = "user_fees"
builder_approval = "max_builder_fee"
funding = "user_funding_stream_and_history"

[execution.economics.formula]
stable_pair_scale = "0.2"
growth_mode_scale = "0.1"
hip3_scale_threshold = "1"
hip3_below_threshold_base = "1"
hip3_at_or_above_threshold_multiplier = "2"
hip3_at_or_above_deployer_share = "0.5"
fee_volume_history_days = "15"
fee_eligibility_window_days = "14"
fee_history_latest_day_offset_days = "0"

[execution.economics.quote_components.protocol]
component_id = "hyperliquid-protocol-execution"
formula_id = "hyperliquid-effective-account-rate"
rate_factor_id = "hyperliquid-live-effective-rate"

[execution.economics.quote_components.builder]
component_id = "hyperliquid-builder-execution"
formula_id = "hyperliquid-builder-notional-fee"
rate_factor_id = "hyperliquid-live-builder-rate"

[execution.economics.assets.settlement]
native_unit = "USD"
identity_kind = "currency"
evidence_fixture_id = "hyperliquid-settlement-fixture"

[execution.economics.edge_basis.primary]
resolver_id = "product-metadata"
product_metadata_source = "hyperliquid-meta"
policy_version = 1

[execution.economics.product_surface_policies]
{surface} = "primary"

[execution.economics.valuation.routes]

[execution.live_submit.{surface}]
approval_id = "hl-unproven-surface-approval"
approval_artifact_path = "operator/hyperliquid-live-submit-approval.json"
approval_artifact_max_bytes = 65536
max_order_count = 1
max_order_notional = "10.00"
product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
product_proof_artifact_max_bytes = 65536

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#
    ))
    .expect("Hyperliquid unproven surface client should parse")
}

fn loaded_config_for_surface(surface: &str) -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    loaded.strategies.clear();
    loaded.root.clients.insert(
        "hyperliquid_unproven_surface".to_string(),
        hyperliquid_execution_client_for_surface(surface),
    );
    loaded
}

fn resolved_hyperliquid_secrets() -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_unproven_surface".to_string(),
        Arc::new(ResolvedBoltV3HyperliquidSecrets {
            private_key: Zeroizing::new(
                "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
            ),
            account_address: Zeroizing::new(
                "0x1111111111111111111111111111111111111111".to_string(),
            ),
            vault_address: None,
        }),
    );
    ResolvedBoltV3Secrets { clients }
}

fn target_product_surface(surface: &str) -> ProductSurface {
    match surface {
        "standard_perps" => ProductSurface::StandardPerps,
        "spot" => ProductSurface::Spot,
        "hip3_builder_perps" => ProductSurface::Hip3BuilderPerps,
        "hip4_outcomes" => ProductSurface::Hip4Outcomes,
        other => panic!("unknown test Hyperliquid product surface {other}"),
    }
}

fn target_instrument_id(surface: &str) -> &'static str {
    match surface {
        "spot" => "BTC/USDC.HYPERLIQUID",
        "hip4_outcomes" => "BTC-YES.HYPERLIQUID",
        _ => "BTC-PERP.HYPERLIQUID",
    }
}

fn target_plan_for_surface(surface: &str) -> MarketIdentityPlan {
    let mut plan = MarketIdentityPlan::empty();
    plan.push_target(HyperliquidInstrumentTargetPlan {
        strategy_instance_id: "hyperliquid-product-matrix-strategy".to_string(),
        configured_target_id: format!("hyperliquid-product-matrix-{surface}"),
        execution_client_id: "hyperliquid_unproven_surface".to_string(),
        product_surface: target_product_surface(surface),
        instrument_id: InstrumentId::from(target_instrument_id(surface)),
        quantity_step: Decimal::new(1, 3),
        notional_step: None,
        min_quantity: Some(Decimal::new(1, 3)),
        min_notional: Some(Decimal::new(100, 2)),
    });
    plan
}

fn assert_surface_without_approval_rejects_live_submit(surface: &str) {
    let loaded = loaded_config_for_surface(surface);
    let resolved = resolved_hyperliquid_secrets();

    let error = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &target_plan_for_surface(surface),
        Arc::new(|| 1_800_000_000),
        ProviderRuntimeApprovals::none(),
    )
    .expect_err("Hyperliquid surface must fail closed without consumed approval");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_unproven_surface");
            assert_eq!(field, "execution.live_submit.approval_id");
            assert!(
                message.contains("consumed live-submit approval"),
                "surface rejection must name missing approval: {message}"
            );
        }
        other => panic!("expected unproven-surface validation invariant, got {other}"),
    }
}

#[test]
fn product_matrix_marks_all_surfaces_approval_gated_after_product_proof_binding() {
    for surface in [
        HyperliquidProductSurface::StandardPerps,
        HyperliquidProductSurface::Spot,
        HyperliquidProductSurface::Hip3BuilderPerps,
        HyperliquidProductSurface::Hip4Outcomes,
    ] {
        assert_serializes_approval_gated(product_entry(surface));
    }
}

#[test]
fn standard_perps_product_matrix_records_nt_discovery_and_approval_gated_submit() {
    assert_info_request_type(
        InfoRequest::meta(),
        "meta",
        "NT standard-perps metadata request serializes",
    );
    let standard_perps = product_entry(HyperliquidProductSurface::StandardPerps);

    assert_sources(
        standard_perps,
        &[
            "nautilus_hyperliquid::http::query::InfoRequest::meta",
            "nautilus_hyperliquid::http::models::PerpMeta",
            "nautilus_hyperliquid::http::parse::parse_perp_instruments",
        ],
    );
    assert_approval_gated(standard_perps);
    assert!(
        !standard_perps
            .missing_submit_proof
            .contains(&"standard perps userFees rate-limit policy reconciliation"),
        "userFees official-weight reconciliation is now accounted by the Hyperliquid provider policy"
    );
}

#[test]
fn spot_product_matrix_records_nt_discovery_and_approval_gated_submit() {
    assert_info_request_type(
        InfoRequest::spot_meta(),
        "spotMeta",
        "NT spot metadata request serializes",
    );
    let spot = product_entry(HyperliquidProductSurface::Spot);

    assert_sources(
        spot,
        &[
            "nautilus_hyperliquid::http::query::InfoRequest::spot_meta",
            "nautilus_hyperliquid::http::models::SpotMeta",
            "nautilus_hyperliquid::http::parse::parse_spot_instruments",
        ],
    );
    assert_approval_gated(spot);
}

#[test]
fn hip3_builder_perps_product_matrix_records_nt_discovery_and_approval_gated_submit() {
    assert_info_request_type(
        InfoRequest::all_perp_metas(),
        "allPerpMetas",
        "NT HIP-3 metadata request serializes",
    );
    let hip3 = product_entry(HyperliquidProductSurface::Hip3BuilderPerps);

    assert_sources(
        hip3,
        &[
            "nautilus_hyperliquid::http::query::InfoRequest::all_perp_metas",
            "nautilus_hyperliquid::http::models::PerpMeta",
            "nautilus_hyperliquid::http::parse::parse_perp_instruments",
        ],
    );
    assert_approval_gated(hip3);
}

#[test]
fn hip4_outcomes_product_matrix_records_nt_discovery_and_approval_gated_submit() {
    assert_info_request_type(
        InfoRequest::outcome_meta(),
        "outcomeMeta",
        "NT HIP-4 metadata request serializes",
    );
    let hip4 = product_entry(HyperliquidProductSurface::Hip4Outcomes);

    assert_sources(
        hip4,
        &[
            "nautilus_hyperliquid::http::query::InfoRequest::outcome_meta",
            "nautilus_hyperliquid::http::models::OutcomeMeta",
            "nautilus_hyperliquid::http::parse::parse_outcome_instruments",
        ],
    );
    assert_approval_gated(hip4);
}

#[test]
fn standard_perps_live_submit_enablement_rejects_without_consumed_surface_approval() {
    assert_surface_without_approval_rejects_live_submit("standard_perps");
}

#[test]
fn spot_live_submit_enablement_rejects_without_consumed_surface_approval() {
    assert_surface_without_approval_rejects_live_submit("spot");
}

#[test]
fn hip3_live_submit_enablement_rejects_without_consumed_surface_approval() {
    assert_surface_without_approval_rejects_live_submit("hip3_builder_perps");
}

#[test]
fn hip4_live_submit_enablement_rejects_without_consumed_surface_approval() {
    assert_surface_without_approval_rejects_live_submit("hip4_outcomes");
}

#[test]
fn product_matrix_artifact_exports_all_hyperliquid_surfaces() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let output_path = temp.path().join("hyperliquid-product-matrix.json");

    let written = write_hyperliquid_product_matrix_artifact(&output_path)
        .expect("Hyperliquid product matrix artifact should write");
    let rendered = std::fs::read_to_string(&written.path).expect("artifact should read");
    let artifact: serde_json::Value =
        serde_json::from_str(&rendered).expect("artifact should parse");

    assert_eq!(
        artifact["record_kind"],
        "bolt_v3.hyperliquid_product_matrix.v1"
    );
    assert_eq!(artifact["provider_key"], "HYPERLIQUID");
    let surfaces = artifact["surfaces"]
        .as_array()
        .expect("artifact surfaces should be array");
    assert_eq!(surfaces.len(), 4);
    for surface in [
        "standard_perps",
        "spot",
        "hip3_builder_perps",
        "hip4_outcomes",
    ] {
        assert!(
            surfaces
                .iter()
                .any(|entry| entry["product_surface"] == surface),
            "artifact must include {surface}"
        );
    }
    assert!(
        surfaces
            .iter()
            .all(|entry| entry["live_submit_status"] == "approval_gated")
    );
}
