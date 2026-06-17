//! Hyperliquid product-surface matrix tests.
//!
//! The matrix is the operator-facing boundary between "we can discover this
//! product" and "we may submit live orders". Discovery evidence must not open
//! submit.

mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::bolt_v3_adapters::{BoltV3AdapterMappingError, map_bolt_v3_adapters};
use bolt_v2::bolt_v3_config::{ClientBlock, LoadedBoltV3Config, load_bolt_v3_config};
use bolt_v2::bolt_v3_providers::hyperliquid::{
    HyperliquidDiscoveryStatus, HyperliquidProductMatrixEntry, HyperliquidProductSurface,
    HyperliquidSubmitStatus, ResolvedBoltV3HyperliquidSecrets, hyperliquid_product_matrix,
};
use bolt_v2::bolt_v3_providers::hyperliquid_artifacts::write_hyperliquid_product_matrix_artifact;
use bolt_v2::bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets};
use nautilus_hyperliquid::http::query::InfoRequest;
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
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = {outcome_settlement_poll_secs}

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

fn assert_surface_without_approval_rejects_live_submit(surface: &str) {
    let loaded = loaded_config_for_surface(surface);
    let resolved = resolved_hyperliquid_secrets();

    let error = map_bolt_v3_adapters(&loaded, &resolved)
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
