//! Hyperliquid no-submit readiness tests.

use bolt_v2::{
    bolt_v3_operator_artifacts::{
        HyperliquidNoSubmitEvidenceRef, HyperliquidNoSubmitReadinessInput,
        build_hyperliquid_no_submit_readiness_artifact,
        write_hyperliquid_no_submit_readiness_artifact,
    },
    bolt_v3_providers::hyperliquid::{
        HyperliquidProductSurface, HyperliquidUserFeesRequestWeightStatus,
        hyperliquid_user_fees_request_weight_policy,
    },
    bolt_v3_submit_admission::{
        BoltV3ExchangeMutationCounts, BoltV3SubmitAdmissionError, validate_no_exchange_mutations,
    },
};
use nautilus_hyperliquid::http::{query::InfoRequest, rate_limits::info_base_weight};

fn hash(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

fn git_sha(seed: char) -> String {
    std::iter::repeat_n(seed, 40).collect()
}

fn evidence_ref(source_kind: &str, seed: char) -> HyperliquidNoSubmitEvidenceRef {
    HyperliquidNoSubmitEvidenceRef {
        source_kind: source_kind.to_string(),
        artifact_sha256: hash(seed),
    }
}

fn readiness_input(
    exchange_mutations: BoltV3ExchangeMutationCounts,
) -> HyperliquidNoSubmitReadinessInput {
    HyperliquidNoSubmitReadinessInput {
        base_sha: git_sha('a'),
        provider_id: "hyperliquid-standard-perps-test".to_string(),
        toml_checksum: hash('b'),
        signer_fingerprint: hash('c'),
        product_surface: HyperliquidProductSurface::StandardPerps,
        metadata_evidence: evidence_ref("metadata", 'd'),
        fee_evidence: evidence_ref("fee", 'e'),
        admission_evidence: evidence_ref("admission", 'f'),
        exchange_mutations,
    }
}

#[test]
fn standard_perps_no_submit_readiness_artifact_records_zero_mutation_proof() {
    let artifact = build_hyperliquid_no_submit_readiness_artifact(readiness_input(
        BoltV3ExchangeMutationCounts::default(),
    ))
    .expect("zero-mutation Hyperliquid no-submit artifact should build");

    assert_eq!(
        artifact.record_kind,
        "bolt_v3.hyperliquid_no_submit_readiness.v1"
    );
    assert_eq!(artifact.provider_key, "HYPERLIQUID");
    assert_eq!(
        artifact.product_surface,
        HyperliquidProductSurface::StandardPerps
    );
    assert_eq!(artifact.exchange_mutation_count, 0);
    assert_eq!(artifact.metadata_evidence.source_kind, "metadata");
    assert_eq!(artifact.fee_evidence.source_kind, "fee");
    assert_eq!(artifact.admission_evidence.source_kind, "admission");
}

#[test]
fn standard_perps_no_submit_readiness_artifact_writes_operator_json() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let output_path = temp.path().join("hyperliquid-no-submit.json");

    let written = write_hyperliquid_no_submit_readiness_artifact(
        readiness_input(BoltV3ExchangeMutationCounts::default()),
        &output_path,
    )
    .expect("zero-mutation Hyperliquid no-submit artifact should write");
    let rendered = std::fs::read_to_string(&written.path).expect("artifact should read");
    let artifact: serde_json::Value =
        serde_json::from_str(&rendered).expect("artifact should parse");

    assert_eq!(
        artifact["record_kind"],
        "bolt_v3.hyperliquid_no_submit_readiness.v1"
    );
    assert_eq!(artifact["provider_key"], "HYPERLIQUID");
    assert_eq!(artifact["product_surface"], "standard_perps");
    assert_eq!(artifact["exchange_mutation_count"], 0);
    assert_eq!(artifact["metadata_evidence"]["source_kind"], "metadata");
    assert_eq!(artifact["fee_evidence"]["source_kind"], "fee");
    assert_eq!(artifact["admission_evidence"]["source_kind"], "admission");
}

#[test]
fn exchange_mutation_guard_blocks_any_mutating_request() {
    let mutating_counts = [
        BoltV3ExchangeMutationCounts {
            submit: 1,
            ..BoltV3ExchangeMutationCounts::default()
        },
        BoltV3ExchangeMutationCounts {
            cancel: 1,
            ..BoltV3ExchangeMutationCounts::default()
        },
        BoltV3ExchangeMutationCounts {
            modify: 1,
            ..BoltV3ExchangeMutationCounts::default()
        },
        BoltV3ExchangeMutationCounts {
            transfer: 1,
            ..BoltV3ExchangeMutationCounts::default()
        },
        BoltV3ExchangeMutationCounts {
            account: 1,
            ..BoltV3ExchangeMutationCounts::default()
        },
    ];

    for counts in mutating_counts {
        let error = validate_no_exchange_mutations(counts)
            .expect_err("any exchange mutation must fail closed");
        assert_eq!(
            error,
            BoltV3SubmitAdmissionError::ExchangeMutationsObserved { mutation_count: 1 }
        );
        let artifact_error =
            build_hyperliquid_no_submit_readiness_artifact(readiness_input(counts))
                .expect_err("no-submit artifact must not build after exchange mutation")
                .to_string();
        assert!(
            artifact_error.contains("exchange_mutation_count"),
            "error must name the failed mutation guard: {artifact_error}"
        );
    }
}

#[test]
fn user_fees_weight_policy_accounts_official_weight_and_nt_inventory() {
    let request = InfoRequest::user_fees("0x1111111111111111111111111111111111111111");
    let request_json = serde_json::to_value(&request).expect("userFees request should serialize");
    assert_eq!(request_json["type"], "userFees");

    let policy = hyperliquid_user_fees_request_weight_policy();

    assert_eq!(policy.official_info_request_weight, 20);
    assert_eq!(
        policy.pinned_nt_info_base_weight,
        info_base_weight(&request)
    );
    assert_eq!(
        policy.status,
        HyperliquidUserFeesRequestWeightStatus::FailClosedPinnedNtWeightMismatch
    );
    assert!(
        policy
            .nt_callers
            .iter()
            .any(|caller| caller == &"nautilus_hyperliquid::http::query::InfoRequest::user_fees")
    );
    assert!(policy.nt_callers.iter().any(|caller| caller
        == &"nautilus_hyperliquid::http::client::HyperliquidHttpClient::info_user_fees"));
}
