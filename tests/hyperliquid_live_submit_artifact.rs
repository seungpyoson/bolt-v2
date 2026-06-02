//! Hyperliquid live-submit approval artifact tests.

use bolt_v2::{
    bolt_v3_providers::hyperliquid::HyperliquidProductSurface,
    bolt_v3_providers::hyperliquid_artifacts::{
        HyperliquidLiveSubmitApprovalArtifact, HyperliquidLiveSubmitApprovalBinding,
        HyperliquidLiveSubmitApprovalInput, HyperliquidLiveSubmitOrderLimits,
        HyperliquidProductSubmitProofBinding, build_hyperliquid_live_submit_approval_artifact,
        consume_hyperliquid_live_submit_approval_artifact,
        validate_hyperliquid_live_submit_approval_artifact,
        write_hyperliquid_live_submit_approval_artifact,
    },
};

const NOW: u64 = 1_000_000;

fn hash(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

fn git_sha(seed: char) -> String {
    std::iter::repeat_n(seed, 40).collect()
}

fn order_limits() -> HyperliquidLiveSubmitOrderLimits {
    HyperliquidLiveSubmitOrderLimits {
        max_order_count: 1,
        max_order_notional: "10.00".to_string(),
    }
}

fn product_submit_proof() -> HyperliquidProductSubmitProofBinding {
    HyperliquidProductSubmitProofBinding {
        artifact_path: "operator/hyperliquid-standard-perps-product-proof.json".to_string(),
        artifact_sha256: hash('d'),
    }
}

fn binding() -> HyperliquidLiveSubmitApprovalBinding {
    HyperliquidLiveSubmitApprovalBinding {
        base_sha: git_sha('a'),
        provider_id: "hyperliquid-standard-perps-test".to_string(),
        product_surface: HyperliquidProductSurface::StandardPerps,
        toml_checksum: hash('b'),
        signer_fingerprint: hash('c'),
        order_limits: order_limits(),
        product_submit_proof: product_submit_proof(),
    }
}

fn approval_input() -> HyperliquidLiveSubmitApprovalInput {
    let current = binding();
    HyperliquidLiveSubmitApprovalInput {
        approval_id: "hl-standard-perps-approval-001".to_string(),
        base_sha: current.base_sha,
        provider_id: current.provider_id,
        product_surface: current.product_surface,
        toml_checksum: current.toml_checksum,
        signer_fingerprint: current.signer_fingerprint,
        order_limits: current.order_limits,
        product_submit_proof: current.product_submit_proof,
        expires_at: NOW + 60,
        used_at: None,
    }
}

fn legacy_approval_without_product_submit_proof() -> HyperliquidLiveSubmitApprovalArtifact {
    serde_json::from_value(serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.hyperliquid_live_submit_approval.v1",
        "provider_key": "HYPERLIQUID",
        "approval_id": "hl-standard-perps-approval-001",
        "base_sha": git_sha('a'),
        "provider_id": "hyperliquid-standard-perps-test",
        "product_surface": "standard_perps",
        "toml_checksum": hash('b'),
        "signer_fingerprint": hash('c'),
        "order_limits": {
            "max_order_count": 1,
            "max_order_notional": "10.00"
        },
        "expires_at": NOW + 60,
        "used_at": null
    }))
    .expect("legacy approval JSON should parse before product-proof validation")
}

#[test]
fn missing_standard_perps_live_submit_approval_fails_closed() {
    let error = validate_hyperliquid_live_submit_approval_artifact(None, &binding(), NOW)
        .expect_err("standard perps live submit must reject a missing approval artifact");

    assert!(
        error.to_string().contains("approval_artifact"),
        "missing approval error must name the gate: {error}"
    );
}

#[test]
fn live_submit_approval_without_product_submit_proof_fails_closed() {
    let approval = legacy_approval_without_product_submit_proof();

    let error =
        validate_hyperliquid_live_submit_approval_artifact(Some(&approval), &binding(), NOW)
            .expect_err("live submit approval without product submit proof must fail closed")
            .to_string();

    assert!(
        error.contains("product_submit_proof"),
        "missing product proof error must name product_submit_proof: {error}"
    );
}

#[test]
fn standard_perps_live_submit_approval_artifact_binds_runtime_fields() {
    let approval = build_hyperliquid_live_submit_approval_artifact(approval_input())
        .expect("bounded approval artifact should build");

    validate_hyperliquid_live_submit_approval_artifact(Some(&approval), &binding(), NOW)
        .expect("matching unexpired unused approval artifact should validate");
    assert_eq!(
        approval.record_kind,
        "bolt_v3.hyperliquid_live_submit_approval.v1"
    );
    assert_eq!(approval.provider_key, "HYPERLIQUID");
    assert_eq!(
        approval.product_surface,
        HyperliquidProductSurface::StandardPerps
    );
    assert_eq!(approval.used_at, None);
}

#[test]
fn live_submit_approval_artifact_accepts_each_hyperliquid_product_surface() {
    for product_surface in [
        HyperliquidProductSurface::StandardPerps,
        HyperliquidProductSurface::Spot,
        HyperliquidProductSurface::Hip3BuilderPerps,
        HyperliquidProductSurface::Hip4Outcomes,
    ] {
        let mut input = approval_input();
        input.product_surface = product_surface;
        let mut binding = binding();
        binding.product_surface = product_surface;

        let approval = build_hyperliquid_live_submit_approval_artifact(input)
            .expect("surface-bound approval artifact should build");
        validate_hyperliquid_live_submit_approval_artifact(Some(&approval), &binding, NOW)
            .expect("approval should validate against the same product surface");
    }
}

#[test]
fn standard_perps_live_submit_approval_artifact_writes_operator_json() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let output_path = temp.path().join("hyperliquid-live-submit-approval.json");

    let written = write_hyperliquid_live_submit_approval_artifact(approval_input(), &output_path)
        .expect("bounded approval artifact should write");
    let rendered = std::fs::read_to_string(&written.path).expect("artifact should read");
    let artifact: serde_json::Value =
        serde_json::from_str(&rendered).expect("artifact should parse");

    assert_eq!(
        artifact["record_kind"],
        "bolt_v3.hyperliquid_live_submit_approval.v1"
    );
    assert_eq!(artifact["provider_key"], "HYPERLIQUID");
    assert_eq!(artifact["product_surface"], "standard_perps");
    assert_eq!(artifact["approval_id"], "hl-standard-perps-approval-001");
    assert_eq!(artifact["order_limits"]["max_order_count"], 1);
    assert_eq!(artifact["order_limits"]["max_order_notional"], "10.00");
    assert_eq!(
        artifact["product_submit_proof"]["artifact_path"],
        "operator/hyperliquid-standard-perps-product-proof.json"
    );
    assert_eq!(
        artifact["product_submit_proof"]["artifact_sha256"],
        hash('d')
    );
    assert!(artifact["used_at"].is_null());
}

#[test]
fn standard_perps_live_submit_approval_rejects_stale_mismatched_expired_reused_and_overbroad() {
    let current = binding();
    let broader_order_count = current.order_limits.max_order_count + 1;

    for (field, mutate) in [
        (
            "base_sha",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.base_sha = git_sha('d');
            }) as Box<dyn Fn(&mut HyperliquidLiveSubmitApprovalArtifact)>,
        ),
        (
            "provider_id",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.provider_id = "other-hyperliquid-provider".to_string();
            }),
        ),
        (
            "product_surface",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.product_surface = HyperliquidProductSurface::Spot;
            }),
        ),
        (
            "toml_checksum",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.toml_checksum = hash('e');
            }),
        ),
        (
            "signer_fingerprint",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.signer_fingerprint = hash('f');
            }),
        ),
        (
            "expires_at",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.expires_at = NOW;
            }),
        ),
        (
            "used_at",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact.used_at = Some(NOW - 1);
            }),
        ),
        (
            "order_limits",
            Box::new(
                move |artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                    artifact.order_limits.max_order_count = broader_order_count;
                },
            ),
        ),
        (
            "product_submit_proof",
            Box::new(|artifact: &mut HyperliquidLiveSubmitApprovalArtifact| {
                artifact
                    .product_submit_proof
                    .as_mut()
                    .expect("valid approval should have product submit proof")
                    .artifact_sha256 = hash('9');
            }),
        ),
    ] {
        let mut approval = build_hyperliquid_live_submit_approval_artifact(approval_input())
            .expect("valid approval artifact should build before mutation");
        mutate(&mut approval);
        let error =
            validate_hyperliquid_live_submit_approval_artifact(Some(&approval), &current, NOW)
                .expect_err("invalid approval must fail closed")
                .to_string();
        assert!(
            error.contains(field),
            "error for {field} must name failed binding: {error}"
        );
    }
}

#[test]
fn provider_consumes_standard_perps_live_submit_approval_once() {
    let current = binding();
    let mut approval = build_hyperliquid_live_submit_approval_artifact(approval_input())
        .expect("bounded approval artifact should build");

    let consumed = consume_hyperliquid_live_submit_approval_artifact(
        &mut approval,
        &current,
        "hl-standard-perps-approval-001",
        NOW,
    )
    .expect("matching unused approval should consume once");
    assert_eq!(consumed.approval_id(), "hl-standard-perps-approval-001");
    assert_eq!(consumed.used_at(), NOW);
    assert_eq!(consumed.product_submit_proof(), &product_submit_proof());
    assert_eq!(approval.used_at, Some(NOW));

    let error = consume_hyperliquid_live_submit_approval_artifact(
        &mut approval,
        &current,
        "hl-standard-perps-approval-001",
        NOW + 1,
    )
    .expect_err("reused approval must fail closed")
    .to_string();
    assert!(
        error.contains("used_at"),
        "reused approval error must name used_at: {error}"
    );
}
