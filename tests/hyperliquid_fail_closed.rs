//! Hyperliquid fail-closed and ops-artifact tests.

mod support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::{
    bolt_v3_adapters::{BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_config::{ClientBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_providers::hyperliquid::{
        HyperliquidLatencyProfileConfig, HyperliquidUserFeesRequestWeightStatus,
        ResolvedBoltV3HyperliquidSecrets, hyperliquid_user_fees_request_weight_policy,
    },
    bolt_v3_providers::hyperliquid_artifacts::{
        HyperliquidLatencyProfileArtifactInput, build_hyperliquid_latency_profile_artifact,
        write_hyperliquid_latency_profile_artifact,
    },
    bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
    bolt_v3_submit_admission::{
        BoltV3ExchangeMutationCounts, BoltV3SubmitAdmissionError, validate_no_exchange_mutations,
    },
};
use nautilus_hyperliquid::http::{query::InfoRequest, rate_limits::info_base_weight};
use zeroize::Zeroizing;

fn hash(seed: char) -> String {
    std::iter::repeat_n(seed, 64).collect()
}

fn latency_profile() -> HyperliquidLatencyProfileConfig {
    HyperliquidLatencyProfileConfig {
        local_info_node_url: "http://127.0.0.1:3001/info".to_string(),
        placement_profile: "aws-ap-northeast-1a-near-hyperliquid-info".to_string(),
        measurement_artifact_path: "/var/lib/bolt/hyperliquid/latency-profile.json".to_string(),
    }
}

fn latency_profile_artifact_input(
    exchange_mutations: BoltV3ExchangeMutationCounts,
) -> HyperliquidLatencyProfileArtifactInput {
    HyperliquidLatencyProfileArtifactInput {
        provider_id: "hyperliquid_perps".to_string(),
        toml_checksum: hash('9'),
        latency_profile: latency_profile(),
        exchange_mutations,
    }
}

fn hyperliquid_standard_perps_client_with_latency_profile() -> ClientBlock {
    toml::from_str(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit_approval_id = "hl-standard-perps-approval-001"
live_submit_product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
live_submit_product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit_product_proof_artifact_max_bytes = 65536
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
outcome_settlement_poll_secs = 0

[execution.latency_profile]
local_info_node_url = "http://127.0.0.1:3001/info"
placement_profile = "aws-ap-northeast-1a-near-hyperliquid-info"
measurement_artifact_path = "/var/lib/bolt/hyperliquid/latency-profile.json"

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
    )
    .expect("Hyperliquid latency-profile client should parse")
}

fn loaded_hyperliquid_latency_profile_config() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    loaded.strategies.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        hyperliquid_standard_perps_client_with_latency_profile(),
    );
    loaded
}

fn resolved_hyperliquid_secrets() -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_perps".to_string(),
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

#[test]
fn latency_profile_artifact_records_ops_metadata_without_exchange_mutations() {
    let artifact = build_hyperliquid_latency_profile_artifact(latency_profile_artifact_input(
        BoltV3ExchangeMutationCounts::none(),
    ))
    .expect("latency profile artifact should build");

    assert_eq!(
        artifact.record_kind,
        "bolt_v3.hyperliquid_latency_profile.v1"
    );
    assert_eq!(artifact.provider_key, "HYPERLIQUID");
    assert_eq!(artifact.provider_id, "hyperliquid_perps");
    assert_eq!(artifact.exchange_mutation_count, 0);
    assert_eq!(
        artifact.latency_profile.local_info_node_url,
        "http://127.0.0.1:3001/info"
    );
    assert_eq!(
        artifact.latency_profile.placement_profile,
        "aws-ap-northeast-1a-near-hyperliquid-info"
    );
    assert_eq!(
        artifact.latency_profile.measurement_artifact_path,
        "/var/lib/bolt/hyperliquid/latency-profile.json"
    );
}

#[test]
fn latency_profile_artifact_writes_operator_json() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let output_path = temp.path().join("hyperliquid-latency-profile.json");

    let written = write_hyperliquid_latency_profile_artifact(
        latency_profile_artifact_input(BoltV3ExchangeMutationCounts::none()),
        &output_path,
    )
    .expect("Hyperliquid latency-profile artifact should write");
    let rendered = std::fs::read_to_string(&written.path).expect("artifact should read");
    let artifact: serde_json::Value =
        serde_json::from_str(&rendered).expect("artifact should parse");

    assert_eq!(
        artifact["record_kind"],
        "bolt_v3.hyperliquid_latency_profile.v1"
    );
    assert_eq!(artifact["provider_key"], "HYPERLIQUID");
    assert_eq!(artifact["provider_id"], "hyperliquid_perps");
    assert_eq!(artifact["exchange_mutation_count"], 0);
    assert_eq!(
        artifact["latency_profile"]["local_info_node_url"],
        "http://127.0.0.1:3001/info"
    );
}

#[test]
fn latency_profile_cannot_bypass_live_submit_approval_gate() {
    let loaded = loaded_hyperliquid_latency_profile_config();
    let resolved = resolved_hyperliquid_secrets();

    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("latency profile must not satisfy live-submit approval");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.live_submit_approval_id");
            assert!(
                message.contains("consumed live-submit approval"),
                "latency profile must not bypass submit gate: {message}"
            );
        }
        other => panic!("expected live-submit approval invariant, got {other}"),
    }
}

#[test]
fn latency_profile_artifact_rejects_exchange_mutations() {
    let error = build_hyperliquid_latency_profile_artifact(latency_profile_artifact_input(
        BoltV3ExchangeMutationCounts {
            submit: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
    ))
    .expect_err("latency-profile artifact must not build after exchange mutation")
    .to_string();

    assert!(
        error.contains("exchange_mutation_count"),
        "error must name the failed mutation guard: {error}"
    );
}

#[test]
fn exchange_mutation_guard_blocks_any_mutating_request() {
    let mutating_counts = [
        BoltV3ExchangeMutationCounts {
            submit: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
        BoltV3ExchangeMutationCounts {
            cancel: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
        BoltV3ExchangeMutationCounts {
            modify: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
        BoltV3ExchangeMutationCounts {
            transfer: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
        BoltV3ExchangeMutationCounts {
            account: 1,
            ..BoltV3ExchangeMutationCounts::none()
        },
    ];

    for counts in mutating_counts {
        let error = validate_no_exchange_mutations(counts)
            .expect_err("any exchange mutation must fail closed");
        assert_eq!(
            error,
            BoltV3SubmitAdmissionError::ExchangeMutationsObserved { mutation_count: 1 }
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
    assert_ne!(
        policy.pinned_nt_info_base_weight, policy.official_info_request_weight,
        "this regression must prove Bolt accounts for the official weight even while the pinned NT weight differs"
    );
    assert_eq!(
        policy.status,
        HyperliquidUserFeesRequestWeightStatus::OfficialWeightAccountedByBoltProviderPolicy
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
