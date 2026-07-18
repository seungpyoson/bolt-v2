#![cfg(test)]

use super::*;

#[test]
fn live_node_adapter_mapping_consumes_hyperliquid_live_submit_approval_artifact() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let bundle = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now,
        &build_head_sha,
    )
    .expect("production live-node mapping should consume approval and map execution");

    assert!(
        bundle
            .configs
            .clients
            .get("hyperliquid_perps")
            .and_then(|client| client.execution.as_ref())
            .is_some(),
        "consumed approval should reach the execution adapter mapper"
    );
    let approval_limits = bundle
        .live_submit_approval_limits
        .get("hyperliquid_perps")
        .expect("consumed Hyperliquid approval should carry submit-admission limits");
    assert_eq!(approval_limits.max_order_count, 2);
    assert_eq!(
        approval_limits.max_order_notional,
        Decimal::from_str_exact("25.00").expect("expected decimal should parse")
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("consumed approval should still read"),
    )
    .expect("consumed approval JSON should parse");
    assert_eq!(persisted["used_at"], now);

    let error = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now + 1,
        &build_head_sha,
    )
    .expect_err("persisted consumption must prevent approval reuse");
    assert!(
        error.to_string().contains("used_at"),
        "reuse failure should identify the spent approval field: {error}"
    );
}

#[test]
fn live_node_without_hyperliquid_execution_target_does_not_select_or_consume_approval() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let approvals =
        load_provider_live_submit_approvals_for_live_node(&loaded, &resolved, now, &build_head_sha)
            .expect("no active Hyperliquid execution target should leave approvals unselected");

    assert!(
        approvals.is_empty(),
        "a configured single-surface client must not auto-select live-submit without an active execution target"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "absence of a Hyperliquid execution target must not spend one-time approval artifacts"
    );
}

#[test]
fn live_node_with_only_non_hyperliquid_routes_does_not_select_or_consume_hyperliquid_approval() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
    let private_key = format!("0x{}", "1".repeat(64));
    // Load the fixture with its real strategies, which route execution to a
    // non-Hyperliquid client (polymarket_main), then add an unreferenced
    // Hyperliquid execution client. This exercises the
    // active_target_surfaces_for_client filter on the realistic path where a
    // strategy is active but targets a different venue.
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded.config_bundle_checksum = "b".repeat(64);
    assert!(
        !loaded.strategies.is_empty(),
        "fixture must route at least one strategy so this exercises the routes-elsewhere path"
    );
    assert!(
        loaded.strategies.iter().all(|strategy| {
            strategy.config.execution_client_id != ClientId::from("hyperliquid_perps")
        }),
        "fixture strategies must route to a non-Hyperliquid execution client for this test"
    );
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let approvals =
        load_provider_live_submit_approvals_for_live_node(&loaded, &resolved, now, &build_head_sha)
            .expect(
                "an unreferenced Hyperliquid execution client should leave approvals unselected",
            );

    assert!(
        approvals.is_empty(),
        "a Hyperliquid client no strategy routes to must not auto-select live-submit"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "a non-Hyperliquid route must not spend the Hyperliquid one-time approval artifact"
    );
}

fn hyperliquid_test_product_submit_proof_bytes(order_proof_path: String) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "record_kind": "bolt_v3.hyperliquid_product_submit_proof.v1",
        "provider_key": "HYPERLIQUID",
        "provider_id": "hyperliquid_perps",
        "product_surface": "standard_perps",
        "toml_checksum": "b".repeat(64),
        "order_proof": {
            "artifact_path": order_proof_path,
            "artifact_sha256": "e".repeat(64),
        },
        "fill_proof": {
            "artifact_path": "operator/hyperliquid-fill-proof.json",
            "artifact_sha256": "f".repeat(64),
        },
        "rounding_proof": {
            "artifact_path": "operator/hyperliquid-rounding-proof.json",
            "artifact_sha256": "a".repeat(64),
        },
        "fee_proof": {
            "artifact_path": "operator/hyperliquid-fee-proof.json",
            "artifact_sha256": "c".repeat(64),
        },
        "settlement_proof": null,
    }))
    .expect("test product proof JSON should encode")
}

fn write_hyperliquid_test_product_submit_proof(path: &std::path::Path) -> String {
    let bytes = hyperliquid_test_product_submit_proof_bytes(
        "operator/hyperliquid-order-proof.json".to_string(),
    );
    std::fs::write(path, &bytes).expect("product proof should write");
    hex::encode(Sha256::digest(&bytes))
}

fn write_hyperliquid_semantically_invalid_product_submit_proof(path: &std::path::Path) -> String {
    let bytes = br#"{"provider":"HYPERLIQUID","surface":"standard_perps"}"#;
    std::fs::write(path, bytes).expect("invalid product proof should write");
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn live_node_invalid_product_submit_proof_schema_does_not_spend_hyperliquid_approval_artifact() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let product_proof_sha256 =
        write_hyperliquid_semantically_invalid_product_submit_proof(&product_proof_path);
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let error = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now,
        &build_head_sha,
    )
    .expect_err("matching hash alone must not authorize live-submit approval consumption");

    assert!(
        error.to_string().contains("product_submit_proof"),
        "failure should identify the product proof schema: {error}"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "invalid product proof semantics must not spend one-time approval artifacts"
    );
}

fn write_hyperliquid_test_product_submit_proof_with_padding(
    path: &std::path::Path,
    padding_len: usize,
) -> String {
    let bytes = hyperliquid_test_product_submit_proof_bytes(format!(
        "operator/{}-hyperliquid-order-proof.json",
        "x".repeat(padding_len)
    ));
    std::fs::write(path, &bytes).expect("padded product proof should write");
    hex::encode(Sha256::digest(&bytes))
}

#[test]
fn live_node_product_submit_proof_uses_independent_byte_cap() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let product_proof_sha256 =
        write_hyperliquid_test_product_submit_proof_with_padding(&product_proof_path, 6000);
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 4096
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 8192
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    live_node_adapter_bundle_with_provider_approvals_at(&loaded, &resolved, now, &build_head_sha)
        .expect("product proof should use its own byte cap before approval consumption");

    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("consumed approval should still read"),
    )
    .expect("consumed approval JSON should parse");
    assert_eq!(persisted["used_at"], now);
}

#[test]
fn live_node_static_target_surface_mismatch_does_not_spend_hyperliquid_approval_artifact() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.strategies.truncate(1);
    let strategy = loaded
        .strategies
        .first_mut()
        .expect("fixture should include one strategy");
    strategy.config.execution_client_id = ClientId::from("hyperliquid_perps");
    strategy.config.target = toml::toml! {
        configured_target_id = "hl-spot-btc-usdc"
        kind = "static_instrument"
        rotating_market_family = "hyperliquid_instrument"
        product_surface = "spot"
        instrument_id = "BTC/USDC.HYPERLIQUID"
        quantity_step = "0.001"
    }
    .into();
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
live_submit.standard_perps.product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display()
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: "operator/hyperliquid-product-submit-proof.json".to_string(),
                artifact_sha256: "d".repeat(64),
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let error = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now,
        &build_head_sha,
    )
    .expect_err("static target surface mismatch must fail before approval consumption");

    assert!(
        error.to_string().contains("execution.product_surfaces"),
        "failure should identify the missing execution product surface: {error}"
    );
    assert!(
        error.to_string().contains("spot"),
        "failure should identify the active target surface: {error}"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "surface mismatches must not spend one-time approval artifacts"
    );
}

#[test]
fn live_node_missing_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let missing_product_proof_path = temp.path().join("missing-product-submit-proof.json");
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            missing_product_proof_path.display()
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: missing_product_proof_path.display().to_string(),
                artifact_sha256: "d".repeat(64),
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let error = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now,
        &build_head_sha,
    )
    .expect_err("missing product submit proof must fail before approval consumption");

    assert!(
        error.to_string().contains("product_submit_proof"),
        "failure should identify the missing product proof binding: {error}"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "missing product proof must not spend one-time approval artifacts"
    );
}

#[test]
fn live_node_mismatched_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
    let _actual_product_proof_sha256 =
        write_hyperliquid_test_product_submit_proof(&product_proof_path);
    let mismatched_product_proof_sha256 = "d".repeat(64);
    let private_key = format!("0x{}", "1".repeat(64));
    let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
    loaded.config_bundle_checksum = "b".repeat(64);
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        toml::from_str(&format!(
            r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
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
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
            approval_path.display(),
            product_proof_path.display(),
            mismatched_product_proof_sha256
        ))
        .expect("Hyperliquid client TOML should parse"),
    );
    let build_head_sha = "a".repeat(40);
    let now = 1_800_000_000;
    write_hyperliquid_live_submit_approval_artifact(
        HyperliquidLiveSubmitApprovalInput {
            approval_id: "hl-standard-perps-approval-001".to_string(),
            base_sha: build_head_sha.clone(),
            provider_id: "hyperliquid_perps".to_string(),
            product_surface:
                crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
            toml_checksum: loaded.config_bundle_checksum.clone(),
            signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
            order_limits: HyperliquidLiveSubmitOrderLimits {
                max_order_count: 2,
                max_order_notional: "25.00".to_string(),
            },
            product_submit_proof: HyperliquidProductSubmitProofBinding {
                artifact_path: product_proof_path.display().to_string(),
                artifact_sha256: mismatched_product_proof_sha256,
            },
            expires_at: now + 300,
            used_at: None,
        },
        &approval_path,
    )
    .expect("approval artifact should write");
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::from([(
            "hyperliquid_perps".to_string(),
            Arc::new(ResolvedBoltV3HyperliquidSecrets {
                private_key: Zeroizing::new(private_key),
                account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                vault_address: None,
            }) as _,
        )]),
    };

    let error = live_node_adapter_bundle_with_provider_approvals_at(
        &loaded,
        &resolved,
        now,
        &build_head_sha,
    )
    .expect_err("mismatched product submit proof must fail before approval consumption");

    assert!(
        error
            .to_string()
            .contains("product_submit_proof.artifact_sha256"),
        "failure should identify the product proof checksum: {error}"
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
    )
    .expect("unconsumed approval JSON should parse");
    assert_eq!(
        persisted["used_at"],
        serde_json::Value::Null,
        "mismatched product proof must not spend one-time approval artifacts"
    );
}
