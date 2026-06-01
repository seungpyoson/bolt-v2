//! Hyperliquid product-surface matrix tests.
//!
//! The matrix is the operator-facing boundary between "we can discover this
//! product" and "we may submit live orders". Discovery evidence must not open
//! submit.

use bolt_v2::bolt_v3_operator_artifacts::write_hyperliquid_product_matrix_artifact;
use bolt_v2::bolt_v3_providers::hyperliquid::{
    HyperliquidDiscoveryStatus, HyperliquidProductMatrixEntry, HyperliquidProductSurface,
    HyperliquidSubmitStatus, hyperliquid_product_matrix,
};
use nautilus_hyperliquid::http::query::InfoRequest;

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

fn assert_fail_closed(entry: &HyperliquidProductMatrixEntry, missing_proof: &str) {
    assert_eq!(
        entry.discovery_status,
        HyperliquidDiscoveryStatus::Supported
    );
    assert_eq!(
        entry.live_submit_status,
        HyperliquidSubmitStatus::FailClosed
    );
    assert!(
        entry
            .missing_submit_proof
            .iter()
            .any(|proof| proof == &missing_proof),
        "matrix missing fail-closed proof gap {missing_proof}"
    );
}

#[test]
fn standard_perps_product_matrix_records_nt_discovery_and_fail_closed_submit() {
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
    assert_fail_closed(
        standard_perps,
        "standard perps no-submit readiness artifact",
    );
    assert_fail_closed(
        standard_perps,
        "standard perps live-submit approval artifact",
    );
}

#[test]
fn spot_product_matrix_records_nt_discovery_and_fail_closed_submit() {
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
    assert_fail_closed(spot, "spot order/fill/rounding/fee proof");
}

#[test]
fn hip3_builder_perps_product_matrix_records_nt_discovery_and_fail_closed_submit() {
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
    assert_fail_closed(hip3, "HIP-3 asset-id/order/fill/rounding/fee proof");
}

#[test]
fn hip4_outcomes_product_matrix_records_nt_discovery_and_fail_closed_submit() {
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
    assert_fail_closed(
        hip4,
        "HIP-4 outcome order/fill/rounding/fee/settlement/userOutcome proof",
    );
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
            .all(|entry| entry["live_submit_status"] == "fail_closed")
    );
}
