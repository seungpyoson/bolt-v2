use crate::support;

use std::{collections::BTreeMap, sync::Arc};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, map_bolt_v3_adapters,
        map_bolt_v3_adapters_with_market_identity_and_runtime_approvals,
    },
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_live_node::{BoltV3LiveNodeError, build_bolt_v3_live_node_with},
    bolt_v3_market_families::{
        MarketIdentityPlan,
        hyperliquid_instrument::{HyperliquidInstrumentTargetPlan, ProductSurface},
        outcome_group::OutcomeGroupTargetPlan,
        updown::UpdownTargetPlan,
    },
    bolt_v3_providers::hyperliquid_artifacts::{
        HyperliquidLiveSubmitApprovalBinding, HyperliquidLiveSubmitApprovalConsumption,
        HyperliquidLiveSubmitApprovalInput, HyperliquidLiveSubmitOrderLimits,
        HyperliquidProductSubmitProofBinding, build_hyperliquid_live_submit_approval_artifact,
        consume_hyperliquid_live_submit_approval_artifact,
    },
    bolt_v3_providers::{
        ProviderRuntimeApprovals,
        binance::ResolvedBoltV3BinanceSecrets,
        chainlink::ResolvedBoltV3ChainlinkSecrets,
        chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets,
        hyperliquid::{HyperliquidProductSurface, ResolvedBoltV3HyperliquidSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
        polyresearch::ResolvedBoltV3PolyResearchSecrets,
    },
    bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
    bolt_v3_wire_boundary::TransportBackend,
};
use nautilus_binance::common::enums::{
    BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
};
use nautilus_binance::config::{
    BinanceDataClientConfig, BinanceSpotMarketDataMode as NtBinanceSpotMarketDataMode,
};
use nautilus_binance::spot::sbe::SBE_SCHEMA_VERSION as NT_BINANCE_SPOT_SBE_SCHEMA_VERSION;
use nautilus_hyperliquid::{
    common::enums::HyperliquidEnvironment as NtHyperliquidEnvironment,
    config::HyperliquidDataClientConfig, factories::HyperliquidExecFactoryConfig,
};
use nautilus_model::identifiers::InstrumentId;
use nautilus_network::transport::sockudo::SockudoTransport;
use nautilus_polymarket::{
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
};
use rust_decimal::Decimal;
use zeroize::Zeroizing;

fn fixture_polymarket_secrets() -> ResolvedBoltV3PolymarketSecrets {
    ResolvedBoltV3PolymarketSecrets {
        private_key: zeroize::Zeroizing::new("regression-poly-private-key".to_string()),
        api_key: zeroize::Zeroizing::new("regression-poly-api-key".to_string()),
        api_secret: zeroize::Zeroizing::new("regression-poly-api-secret".to_string()),
        passphrase: zeroize::Zeroizing::new("regression-poly-passphrase".to_string()),
    }
}

fn fixture_binance_secrets() -> ResolvedBoltV3BinanceSecrets {
    ResolvedBoltV3BinanceSecrets {
        api_key: zeroize::Zeroizing::new("regression-binance-api-key".to_string()),
        api_secret: zeroize::Zeroizing::new("regression-binance-api-secret".to_string()),
    }
}

fn fixture_hyperliquid_secrets() -> ResolvedBoltV3HyperliquidSecrets {
    ResolvedBoltV3HyperliquidSecrets {
        private_key: Zeroizing::new(
            "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
        ),
        account_address: Zeroizing::new("0x1111111111111111111111111111111111111111".to_string()),
        vault_address: None,
    }
}

fn fixture_chainlink_secrets() -> ResolvedBoltV3ChainlinkSecrets {
    ResolvedBoltV3ChainlinkSecrets {
        api_key: zeroize::Zeroizing::new("regression-chainlink-api-key".to_string()),
        api_secret: zeroize::Zeroizing::new("regression-chainlink-api-secret".to_string()),
    }
}

fn fixture_chainlink_reference_secrets() -> ResolvedBoltV3ChainlinkReferenceSecrets {
    ResolvedBoltV3ChainlinkReferenceSecrets {
        api_key: zeroize::Zeroizing::new("regression-chainlink-reference-api-key".to_string()),
        api_secret: zeroize::Zeroizing::new(
            "regression-chainlink-reference-api-secret".to_string(),
        ),
    }
}

fn fixture_polyresearch_secrets() -> ResolvedBoltV3PolyResearchSecrets {
    ResolvedBoltV3PolyResearchSecrets {
        api_key: zeroize::Zeroizing::new("regression-polyresearch-api-key".to_string()),
    }
}

fn nt_polymarket_signature_type(
    value: polymarket::PolymarketSignatureType,
) -> NtPolymarketSignatureType {
    match value {
        polymarket::PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
        polymarket::PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
        polymarket::PolymarketSignatureType::PolyGnosisSafe => {
            NtPolymarketSignatureType::PolyGnosisSafe
        }
    }
}

fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "polymarket_main".to_string(),
        Arc::new(fixture_polymarket_secrets()),
    );
    clients.insert(
        "binance_reference".to_string(),
        Arc::new(fixture_binance_secrets()),
    );
    clients.insert(
        "chainlink_strike".to_string(),
        Arc::new(fixture_chainlink_secrets()),
    );
    clients.insert(
        "chainlink_reference".to_string(),
        Arc::new(fixture_chainlink_reference_secrets()),
    );
    clients.insert(
        "polyresearch_reference".to_string(),
        Arc::new(fixture_polyresearch_secrets()),
    );
    ResolvedBoltV3Secrets { clients }
}

fn fixture_resolved_hyperliquid_secrets() -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_perps".to_string(),
        Arc::new(fixture_hyperliquid_secrets()),
    );
    ResolvedBoltV3Secrets { clients }
}

fn hyperliquid_updown_target_plan() -> MarketIdentityPlan {
    let mut plan = MarketIdentityPlan::empty();
    plan.push_target(UpdownTargetPlan {
        strategy_instance_id: "hyperliquid-updown-strategy".to_string(),
        configured_target_id: "hyperliquid-updown-target".to_string(),
        execution_client_id: "hyperliquid_perps".to_string(),
        underlying_asset: "BTC".to_string(),
        cadence_secs: 300,
        cadence_slug_token: "window".to_string(),
    });
    plan
}

fn hyperliquid_outcome_group_target_plan() -> MarketIdentityPlan {
    let mut plan = MarketIdentityPlan::empty();
    plan.push_target(OutcomeGroupTargetPlan {
        strategy_instance_id: "hyperliquid-complete-set-strategy".to_string(),
        configured_target_id: "hyperliquid-outcome-group-target".to_string(),
        execution_client_id: "hyperliquid_perps".to_string(),
        group_sources: vec!["hl_world_cup".to_string()],
    });
    plan
}

fn hyperliquid_static_instrument_target_plan(
    product_surface: ProductSurface,
    instrument_id: &str,
) -> MarketIdentityPlan {
    let mut plan = MarketIdentityPlan::empty();
    plan.push_target(HyperliquidInstrumentTargetPlan {
        strategy_instance_id: "hyperliquid-static-strategy".to_string(),
        configured_target_id: "hyperliquid-static-target".to_string(),
        execution_client_id: "hyperliquid_perps".to_string(),
        product_surface,
        instrument_id: InstrumentId::from(instrument_id),
        quantity_step: Decimal::new(1, 3),
        notional_step: None,
        min_quantity: Some(Decimal::new(1, 3)),
        min_notional: Some(Decimal::new(100, 2)),
    });
    plan
}

fn hyperliquid_multi_static_instrument_target_plan() -> MarketIdentityPlan {
    let mut plan = hyperliquid_static_instrument_target_plan(
        ProductSurface::StandardPerps,
        "BTC-PERP.HYPERLIQUID",
    );
    plan.push_target(HyperliquidInstrumentTargetPlan {
        strategy_instance_id: "hyperliquid-static-spot-strategy".to_string(),
        configured_target_id: "hyperliquid-static-spot-target".to_string(),
        execution_client_id: "hyperliquid_perps".to_string(),
        product_surface: ProductSurface::Spot,
        instrument_id: InstrumentId::from("BTC/USDC.HYPERLIQUID"),
        quantity_step: Decimal::new(1, 3),
        notional_step: None,
        min_quantity: Some(Decimal::new(1, 3)),
        min_notional: Some(Decimal::new(100, 2)),
    });
    plan
}

fn fixed_market_clock(now_unix_seconds: i64) -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(move || now_unix_seconds)
}

fn fixture_loaded_config_with_binance_reference() -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.insert(
        "binance_reference".to_string(),
        toml::from_str(&support::repo_text(
            "tests/fixtures/bolt_v3/binance_reference_client.toml",
        ))
        .expect("binance provider fixture client should parse"),
    );
    loaded
}

fn hyperliquid_client(
    product_surface: &str,
    approval_id: &str,
) -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client_with_outcome_settlement_poll(product_surface, approval_id, 0)
}

fn hyperliquid_client_with_outcome_settlement_poll(
    product_surface: &str,
    approval_id: &str,
    outcome_settlement_poll_secs: u64,
) -> bolt_v2::bolt_v3_config::ClientBlock {
    let product_proof_hash = "d".repeat(64);
    toml::from_str(&format!(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["{product_surface}"]
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

[execution.live_submit.{product_surface}]
approval_id = "{approval_id}"
approval_artifact_path = "operator/hyperliquid-live-submit-approval.json"
approval_artifact_max_bytes = 65536
max_order_count = 1
max_order_notional = "10.00"
product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
product_proof_artifact_sha256 = "{product_proof_hash}"
product_proof_artifact_max_bytes = 65536

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
    ))
    .expect("hyperliquid standard-perps client should parse")
}

fn set_hyperliquid_product_surfaces(
    client: &mut bolt_v2::bolt_v3_config::ClientBlock,
    surfaces: &[&str],
) {
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "product_surfaces".to_string(),
            toml::Value::Array(
                surfaces
                    .iter()
                    .map(|surface| toml::Value::String((*surface).to_string()))
                    .collect(),
            ),
        );
}

fn hyperliquid_data_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    toml::from_str(
        r#"
venue = "HYPERLIQUID"

[data]
environment = "testnet"
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
ws_timeout_secs = 30
stale_stream_receive_timeout_secs = 45
stream_health_check_interval_secs = 5
stale_stream_warning_cooldown_secs = 20
stale_stream_recovery_enabled = true
stale_stream_recovery_cooldown_secs = 90
stale_stream_max_targeted_resubscribes = 4
update_instruments_interval_mins = 5
transport_backend = "sockudo"
"#,
    )
    .expect("hyperliquid data client should parse")
}

fn hyperliquid_standard_perps_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client("standard_perps", "hl-standard-perps-approval-001")
}

fn hyperliquid_spot_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client("spot", "hl-spot-approval-001")
}

fn hyperliquid_hip3_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client("hip3_builder_perps", "hl-hip3-approval-001")
}

fn hyperliquid_hip4_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client_with_outcome_settlement_poll("hip4_outcomes", "hl-hip4-approval-001", 5)
}

fn hyperliquid_hip4_without_settlement_poll_client() -> bolt_v2::bolt_v3_config::ClientBlock {
    hyperliquid_client("hip4_outcomes", "hl-hip4-approval-001")
}

fn fixture_loaded_config_with_hyperliquid_standard_perps() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_standard_perps_client())
}

fn fixture_loaded_config_with_hyperliquid_data() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_data_client())
}

fn fixture_loaded_config_with_hyperliquid_spot() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_spot_client())
}

fn fixture_loaded_config_with_hyperliquid_hip3() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_hip3_client())
}

fn fixture_loaded_config_with_hyperliquid_hip4() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_hip4_client())
}

fn fixture_loaded_config_with_hyperliquid_hip4_without_settlement_poll() -> LoadedBoltV3Config {
    fixture_loaded_config_with_hyperliquid_client(hyperliquid_hip4_without_settlement_poll_client())
}

fn fixture_loaded_config_with_hyperliquid_client(
    client: bolt_v2::bolt_v3_config::ClientBlock,
) -> LoadedBoltV3Config {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    loaded.strategies.clear();
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    loaded
}

fn consumed_hyperliquid_standard_perps_approval() -> HyperliquidLiveSubmitApprovalConsumption {
    consumed_hyperliquid_approval(
        HyperliquidProductSurface::StandardPerps,
        "hl-standard-perps-approval-001",
    )
}

fn consumed_hyperliquid_spot_approval() -> HyperliquidLiveSubmitApprovalConsumption {
    consumed_hyperliquid_approval(HyperliquidProductSurface::Spot, "hl-spot-approval-001")
}

fn consumed_hyperliquid_hip3_approval() -> HyperliquidLiveSubmitApprovalConsumption {
    consumed_hyperliquid_approval(
        HyperliquidProductSurface::Hip3BuilderPerps,
        "hl-hip3-approval-001",
    )
}

fn consumed_hyperliquid_hip4_approval() -> HyperliquidLiveSubmitApprovalConsumption {
    consumed_hyperliquid_approval(
        HyperliquidProductSurface::Hip4Outcomes,
        "hl-hip4-approval-001",
    )
}

fn consumed_hyperliquid_approval(
    product_surface: HyperliquidProductSurface,
    approval_id: &str,
) -> HyperliquidLiveSubmitApprovalConsumption {
    let order_limits = HyperliquidLiveSubmitOrderLimits {
        max_order_count: 1,
        max_order_notional: "10.00".to_string(),
    };
    let binding = HyperliquidLiveSubmitApprovalBinding {
        base_sha: "a".repeat(40),
        provider_id: "hyperliquid_perps".to_string(),
        product_surface,
        toml_checksum: "b".repeat(64),
        signer_fingerprint: "c".repeat(64),
        order_limits: order_limits.clone(),
        product_submit_proof: HyperliquidProductSubmitProofBinding {
            artifact_path: "operator/hyperliquid-product-submit-proof.json".to_string(),
            artifact_sha256: "d".repeat(64),
        },
    };
    let mut approval =
        build_hyperliquid_live_submit_approval_artifact(HyperliquidLiveSubmitApprovalInput {
            approval_id: approval_id.to_string(),
            base_sha: binding.base_sha.clone(),
            provider_id: binding.provider_id.clone(),
            product_surface: binding.product_surface,
            toml_checksum: binding.toml_checksum.clone(),
            signer_fingerprint: binding.signer_fingerprint.clone(),
            order_limits,
            product_submit_proof: binding.product_submit_proof.clone(),
            expires_at: 1_800_000_300,
            used_at: None,
        })
        .expect("test Hyperliquid approval artifact should build");
    consume_hyperliquid_live_submit_approval_artifact(
        &mut approval,
        &binding,
        approval_id,
        1_800_000_000,
    )
    .expect("test Hyperliquid approval should consume once")
}

#[test]
fn configured_sockudo_transport_backend_is_compiled_for_live_connectivity() {
    let _ = std::any::type_name::<SockudoTransport<tokio::net::TcpStream>>();
}

/// Captured live exchangeInfo wire (schema 3, vendor version 5).
/// Same fixture as `tests/binance_sbe_schema_v5_decode.rs`; the official pin's
/// matching version is direct capability-presence evidence.
const CAPTURED_LIVE_EXCHANGE_INFO_SBE: &[u8] =
    include_bytes!("fixtures/binance_sbe/exchange_info_btc_usdt_schema_3_5.bin");

fn captured_live_exchange_info_sbe_version() -> u16 {
    assert!(
        CAPTURED_LIVE_EXCHANGE_INFO_SBE.len() >= 8,
        "captured exchangeInfo fixture must contain at least the SBE message header"
    );
    u16::from_le_bytes([
        CAPTURED_LIVE_EXCHANGE_INFO_SBE[6],
        CAPTURED_LIVE_EXCHANGE_INFO_SBE[7],
    ])
}

#[test]
fn nt_binance_spot_sbe_schema_matches_live_exchange_info_version() {
    assert_eq!(NT_BINANCE_SPOT_SBE_SCHEMA_VERSION, 5);
    assert_eq!(captured_live_exchange_info_sbe_version(), 5);
    assert_eq!(
        NT_BINANCE_SPOT_SBE_SCHEMA_VERSION,
        captured_live_exchange_info_sbe_version()
    );
}

#[test]
fn polymarket_client_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

    let polymarket = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");

    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket [data] should downcast to NT PolymarketDataClientConfig");
    assert_eq!(
        data.base_url_http.as_deref(),
        Some("https://clob.polymarket.com")
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some("wss://ws-subscriptions-clob.polymarket.com/ws/market")
    );
    assert_eq!(
        data.base_url_gamma.as_deref(),
        Some("https://gamma-api.polymarket.com")
    );
    assert_eq!(
        data.base_url_data_api.as_deref(),
        Some("https://data-api.polymarket.com")
    );
    assert_eq!(data.http_timeout_secs, 60);
    assert_eq!(data.ws_timeout_secs, 30);
    assert_eq!(data.ws_max_subscriptions, 200);
    assert_eq!(data.update_instruments_interval_mins, Some(1));
    assert!(!data.subscribe_new_markets);
    assert!(data.drop_quotes_missing_side);
    assert_eq!(
        data.base_url_rtds.as_deref(),
        Some("wss://ws-live-data.polymarket.com")
    );
    assert_eq!(data.new_market_fetch_max_concurrency, 8);
    assert!(!data.resolve_poll_enabled);
    assert_eq!(data.resolve_poll_interval_secs, 30);
    assert_eq!(data.resolve_poll_grace_secs, 10);
    assert_eq!(data.resolve_poll_max_wait_secs, 1800);
    assert!(!data.auto_load_missing_instruments);
    assert_eq!(data.auto_load_debounce_ms, 250);
    assert_eq!(data.transport_backend, TransportBackend::Sockudo);
    assert_eq!(
        data.filters.len(),
        1,
        "production adapter mapping must install the updown market-slug filter"
    );
    assert_eq!(
        data.filters[0]
            .market_slugs()
            .expect("installed updown market-slug filter must return current and next slugs")
            .len(),
        2
    );

    let exec = polymarket
        .execution
        .as_ref()
        .expect("polymarket [execution] block must produce an NT exec config")
        .config_as::<PolymarketExecClientConfig>()
        .expect("polymarket [execution] should downcast to NT PolymarketExecClientConfig");
    let expected_execution: polymarket::PolymarketExecutionConfig = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture Polymarket client should exist")
        .execution
        .clone()
        .expect("fixture Polymarket execution block should exist")
        .try_into()
        .expect("fixture Polymarket execution block should parse");
    assert_eq!(
        exec.signature_type,
        nt_polymarket_signature_type(expected_execution.signature_type)
    );
    assert_eq!(
        exec.private_key.as_deref(),
        Some(fixture_polymarket_secrets().private_key.as_str())
    );
    assert_eq!(
        exec.api_key.as_deref(),
        Some(fixture_polymarket_secrets().api_key.as_str())
    );
    assert_eq!(
        exec.api_secret.as_deref(),
        Some(fixture_polymarket_secrets().api_secret.as_str())
    );
    assert_eq!(
        exec.passphrase.as_deref(),
        Some(fixture_polymarket_secrets().passphrase.as_str())
    );
    assert!(
        exec.funder.as_deref() == expected_execution.funder.as_deref(),
        "mapped Polymarket funder must match the fixture funder"
    );
    assert_eq!(
        exec.base_url_http.as_deref(),
        Some("https://clob.polymarket.com")
    );
    assert_eq!(
        exec.base_url_ws.as_deref(),
        Some("wss://ws-subscriptions-clob.polymarket.com/ws/user")
    );
    assert_eq!(
        exec.base_url_data_api.as_deref(),
        Some("https://data-api.polymarket.com")
    );
    assert_eq!(exec.http_timeout_secs, 60);
    assert_eq!(exec.max_retries, 3);
    assert_eq!(exec.retry_delay_initial_ms, 250);
    assert_eq!(exec.retry_delay_max_ms, 2000);
    assert_eq!(exec.ack_timeout_secs, 5);
    assert_eq!(exec.transport_backend, TransportBackend::Sockudo);
}

#[test]
fn hyperliquid_standard_perps_requires_consumed_live_submit_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_perps".to_string(),
        Arc::new(fixture_hyperliquid_secrets()),
    );
    let resolved = ResolvedBoltV3Secrets { clients };

    let error = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::StandardPerps,
            "BTC-PERP.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals::none(),
    )
    .expect_err("Hyperliquid live submit must fail without consumed approval");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.live_submit.approval_id");
            assert!(message.contains("consumed live-submit approval"));
        }
        other => panic!("expected Hyperliquid live-submit approval invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_data_maps_to_nt_market_data_adapter_without_execution_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_data();
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };

    let configs =
        map_bolt_v3_adapters(&loaded, &resolved).expect("Hyperliquid data should map cleanly");

    let hyperliquid = configs
        .clients
        .get("hyperliquid_perps")
        .expect("Hyperliquid client must be present in mapper output");
    let data = hyperliquid
        .data
        .as_ref()
        .expect("Hyperliquid [data] block must produce an NT data config");
    assert!(hyperliquid.execution.is_none());
    assert_eq!(data.factory.name(), "HYPERLIQUID");
    assert_eq!(data.factory.config_type(), "HyperliquidDataClientConfig");

    let config = data
        .config_as::<HyperliquidDataClientConfig>()
        .expect("Hyperliquid data should downcast to NT HyperliquidDataClientConfig");
    assert_eq!(config.private_key, None);
    assert_eq!(
        config.base_url_ws.as_deref(),
        Some("wss://api.hyperliquid-testnet.xyz/ws")
    );
    assert_eq!(
        config.base_url_http.as_deref(),
        Some("https://api.hyperliquid-testnet.xyz/info")
    );
    assert_eq!(config.proxy_url.as_deref(), Some("http://127.0.0.1:8080"));
    assert_eq!(config.environment, NtHyperliquidEnvironment::Testnet);
    assert_eq!(config.http_timeout_secs, 60);
    assert_eq!(config.ws_timeout_secs, 30);
    assert_eq!(config.stale_stream_receive_timeout_secs, 45);
    assert_eq!(config.stream_health_check_interval_secs, 5);
    assert_eq!(config.stale_stream_warning_cooldown_secs, 20);
    assert!(config.stale_stream_recovery_enabled);
    assert_eq!(config.stale_stream_recovery_cooldown_secs, 90);
    assert_eq!(config.stale_stream_max_targeted_resubscribes, 4);
    assert_eq!(config.update_instruments_interval_mins, 5);
    assert_eq!(config.transport_backend, TransportBackend::Sockudo);
}

#[test]
fn hyperliquid_standard_perps_maps_to_nt_after_consumed_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_perps".to_string(),
        Arc::new(fixture_hyperliquid_secrets()),
    );
    let resolved = ResolvedBoltV3Secrets { clients };
    let consumed = consumed_hyperliquid_standard_perps_approval();

    let configs = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::StandardPerps,
            "BTC-PERP.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect("consumed approval should open the standard-perps NT adapter path");

    let hyperliquid = configs
        .clients
        .get("hyperliquid_perps")
        .expect("Hyperliquid client must be present in mapper output");
    assert!(hyperliquid.data.is_none());
    let execution = hyperliquid
        .execution
        .as_ref()
        .expect("Hyperliquid [execution] block must produce an NT exec config");
    assert_eq!(execution.factory.name(), "HYPERLIQUID");
    assert_eq!(
        execution.factory.config_type(),
        "HyperliquidExecFactoryConfig"
    );

    let config = execution
        .config_as::<HyperliquidExecFactoryConfig>()
        .expect("Hyperliquid execution should downcast to NT HyperliquidExecFactoryConfig");
    let expected_secrets = fixture_hyperliquid_secrets();
    assert_eq!(config.trader_id.to_string(), "BOLT-001");
    assert_eq!(config.account_id.to_string(), "HYPERLIQUID-001");
    assert_eq!(
        config.config.private_key.as_deref(),
        Some(expected_secrets.private_key.as_str())
    );
    assert_eq!(
        config.config.account_address.as_deref(),
        Some(expected_secrets.account_address.as_str())
    );
    assert_eq!(config.config.vault_address, None);
    assert_eq!(
        config.config.base_url_ws.as_deref(),
        Some("wss://api.hyperliquid-testnet.xyz/ws")
    );
    assert_eq!(
        config.config.base_url_http.as_deref(),
        Some("https://api.hyperliquid-testnet.xyz/info")
    );
    assert_eq!(
        config.config.base_url_exchange.as_deref(),
        Some("https://api.hyperliquid-testnet.xyz/exchange")
    );
    assert_eq!(config.config.environment, NtHyperliquidEnvironment::Testnet);
    assert_eq!(config.config.http_timeout_secs, 60);
    assert_eq!(config.config.max_retries, 3);
    assert_eq!(config.config.retry_delay_initial_ms, 250);
    assert_eq!(config.config.retry_delay_max_ms, 2000);
    assert!(config.config.normalize_prices);
    assert_eq!(config.config.market_order_slippage_bps, 50);
    assert!(!config.config.include_builder_attribution);
    assert_eq!(config.config.transport_backend, TransportBackend::Sockudo);
    assert_eq!(config.config.ws_post_timeout_secs, 10);
    assert_eq!(config.config.outcome_settlement_poll_secs, 0);
}

#[test]
fn hyperliquid_hip4_accepts_updown_market_family_target_after_consumed_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip4();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_hip4_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let configs = bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_updown_target_plan(),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .expect("Hyperliquid should advertise support for the updown target family before provider mapping");

    assert!(
        configs
            .clients
            .get("hyperliquid_perps")
            .and_then(|client| client.execution.as_ref())
            .is_some(),
        "family support should allow the consumed Hyperliquid execution adapter to map"
    );
}

#[test]
fn hyperliquid_non_hip4_execution_rejects_updown_market_family_target() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_standard_perps_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let error =
        bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
            &loaded,
            &resolved,
            &hyperliquid_updown_target_plan(),
            fixed_market_clock(1_800_000_000),
            ProviderRuntimeApprovals {
                live_submit: Some(&approvals),
            },
        )
        .expect_err("updown targets must require a HIP-4 Hyperliquid execution surface");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant { field, message, .. } => {
            assert_eq!(field, "strategy.target.rotating_market_family");
            assert!(
                message.contains("updown") && message.contains("hip4_outcomes"),
                "failure should identify the family/surface mismatch: {message}"
            );
        }
        other => panic!("expected target-family validation error, got {other:?}"),
    }
}

#[test]
fn hyperliquid_hip4_accepts_outcome_group_target_after_consumed_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip4();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_hip4_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let configs = bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_outcome_group_target_plan(),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .expect("configured outcome_group targets should map only through a consumed HIP-4 execution approval");

    assert!(
        configs
            .clients
            .get("hyperliquid_perps")
            .and_then(|client| client.execution.as_ref())
            .is_some(),
        "outcome_group family support should allow the consumed HIP-4 execution adapter to map"
    );
}

#[test]
fn hyperliquid_non_hip4_execution_rejects_outcome_group_target() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_standard_perps_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let error =
        bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
            &loaded,
            &resolved,
            &hyperliquid_outcome_group_target_plan(),
            fixed_market_clock(1_800_000_000),
            ProviderRuntimeApprovals {
                live_submit: Some(&approvals),
            },
        )
        .expect_err("outcome_group targets must require a HIP-4 Hyperliquid execution surface");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant { field, message, .. } => {
            assert_eq!(field, "strategy.target.rotating_market_family");
            assert!(
                message.contains("outcome_group") && message.contains("hip4_outcomes"),
                "failure should identify the family/surface mismatch: {message}"
            );
        }
        other => panic!("expected outcome_group target-family validation error, got {other:?}"),
    }
}

#[test]
fn hyperliquid_standard_perps_accepts_static_instrument_target_after_consumed_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_standard_perps_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let configs = bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::StandardPerps,
            "BTC-PERP.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .expect("Hyperliquid should advertise support for static-instrument targets before provider mapping");

    assert!(
        configs
            .clients
            .get("hyperliquid_perps")
            .and_then(|client| client.execution.as_ref())
            .is_some(),
        "static-instrument family support should allow the consumed Hyperliquid execution adapter to map"
    );
}

#[test]
fn hyperliquid_static_instrument_target_surface_must_match_execution_surface() {
    let loaded = fixture_loaded_config_with_hyperliquid_standard_perps();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_standard_perps_approval();
    let mut approvals = bolt_v2::bolt_v3_providers::ProviderLiveSubmitApprovals::empty();
    approvals.insert(
        "hyperliquid_perps".to_string(),
        bolt_v2::bolt_v3_providers::ProviderLiveSubmitApproval::new(Box::new(consumed)),
    );

    let error =
        bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
            &loaded,
            &resolved,
            &hyperliquid_static_instrument_target_plan(
                ProductSurface::Spot,
                "BTC/USDC.HYPERLIQUID",
            ),
            fixed_market_clock(1_800_000_000),
            ProviderRuntimeApprovals {
                live_submit: Some(&approvals),
            },
        )
        .expect_err(
            "static Hyperliquid target product surface must match the execution client surface",
        );

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "strategy.target.product_surface");
            assert!(
                message.contains("spot"),
                "target surface must be named: {message}"
            );
            assert!(
                message.contains("execution.product_surfaces does not include it"),
                "configured execution surfaces must be identified as missing the target surface: {message}"
            );
        }
        other => panic!("expected Hyperliquid static-instrument surface invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_execution_rejects_multiple_active_product_surfaces() {
    let mut client = hyperliquid_standard_perps_client();
    set_hyperliquid_product_surfaces(&mut client, &["standard_perps", "spot"]);
    let loaded = fixture_loaded_config_with_hyperliquid_client(client);
    let resolved = fixture_resolved_hyperliquid_secrets();

    let error =
        bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
            &loaded,
            &resolved,
            &hyperliquid_multi_static_instrument_target_plan(),
            fixed_market_clock(1_800_000_000),
            ProviderRuntimeApprovals { live_submit: None },
        )
        .expect_err("one Hyperliquid execution client must not arm multiple active surfaces");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "strategy.target.product_surface");
            assert!(
                message.contains("multiple active product surfaces")
                    && message.contains("standard_perps")
                    && message.contains("spot"),
                "multiple-surface rejection should name the active surfaces: {message}"
            );
        }
        other => panic!("expected Hyperliquid multi-surface invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_execution_requires_live_submit_block_for_active_surface() {
    let mut client = hyperliquid_standard_perps_client();
    set_hyperliquid_product_surfaces(&mut client, &["standard_perps", "spot"]);
    let loaded = fixture_loaded_config_with_hyperliquid_client(client);
    let resolved = fixture_resolved_hyperliquid_secrets();

    let error =
        bolt_v2::bolt_v3_adapters::map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
            &loaded,
            &resolved,
            &hyperliquid_static_instrument_target_plan(
                ProductSurface::Spot,
                "BTC/USDC.HYPERLIQUID",
            ),
            fixed_market_clock(1_800_000_000),
            ProviderRuntimeApprovals { live_submit: None },
        )
        .expect_err("active Hyperliquid surface must have a matching live_submit block");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.live_submit");
            assert!(
                message.contains("spot") && message.contains("configured execution.live_submit"),
                "missing surface gate should name the active surface: {message}"
            );
        }
        other => panic!("expected Hyperliquid live-submit surface invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_spot_requires_consumed_surface_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_spot();
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "hyperliquid_perps".to_string(),
        Arc::new(fixture_hyperliquid_secrets()),
    );
    let resolved = ResolvedBoltV3Secrets { clients };

    let error = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(ProductSurface::Spot, "BTC/USDC.HYPERLIQUID"),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals::none(),
    )
    .expect_err("Hyperliquid spot live submit must fail without consumed approval");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.live_submit.approval_id");
            assert!(message.contains("consumed live-submit approval"));
        }
        other => panic!("expected Hyperliquid spot approval invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_spot_maps_to_nt_after_consumed_surface_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_spot();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_spot_approval();

    let configs = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(ProductSurface::Spot, "BTC/USDC.HYPERLIQUID"),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect("consumed spot approval should open the NT adapter path");

    let hyperliquid = configs
        .clients
        .get("hyperliquid_perps")
        .expect("Hyperliquid client must be present in mapper output");
    assert!(hyperliquid.data.is_none());
    let execution = hyperliquid
        .execution
        .as_ref()
        .expect("Hyperliquid spot [execution] block must produce an NT exec config");
    assert_eq!(execution.factory.name(), "HYPERLIQUID");

    let config = execution
        .config_as::<HyperliquidExecFactoryConfig>()
        .expect("Hyperliquid spot execution should downcast to NT HyperliquidExecFactoryConfig");
    assert_eq!(config.config.environment, NtHyperliquidEnvironment::Testnet);
    assert_eq!(
        config.config.base_url_exchange.as_deref(),
        Some("https://api.hyperliquid-testnet.xyz/exchange")
    );
}

#[test]
fn hyperliquid_hip3_maps_to_nt_after_consumed_surface_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip3();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_hip3_approval();

    let configs = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::Hip3BuilderPerps,
            "BTC-PERP.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect("consumed HIP-3 approval should open the NT adapter path");

    let execution = configs
        .clients
        .get("hyperliquid_perps")
        .and_then(|client| client.execution.as_ref())
        .expect("Hyperliquid HIP-3 execution config must map");
    assert_eq!(execution.factory.name(), "HYPERLIQUID");
    execution
        .config_as::<HyperliquidExecFactoryConfig>()
        .expect("Hyperliquid HIP-3 execution should downcast to NT config");
}

#[test]
fn hyperliquid_hip4_requires_positive_settlement_poll() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip4_without_settlement_poll();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_hip4_approval();

    let error = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::Hip4Outcomes,
            "BTC-YES.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect_err("HIP-4 execution must not map with disabled settlement polling");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.outcome_settlement_poll_secs");
            assert!(
                message.contains("HIP-4"),
                "HIP-4 settlement guard must be named: {message}"
            );
        }
        other => panic!("expected HIP-4 settlement poll invariant, got {other}"),
    }
}

#[test]
fn hyperliquid_hip4_maps_to_nt_after_consumed_surface_approval() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip4();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed = consumed_hyperliquid_hip4_approval();

    let configs = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::Hip4Outcomes,
            "BTC-YES.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect("consumed HIP-4 approval should open the NT adapter path");

    let execution = configs
        .clients
        .get("hyperliquid_perps")
        .and_then(|client| client.execution.as_ref())
        .expect("Hyperliquid HIP-4 execution config must map");
    assert_eq!(execution.factory.name(), "HYPERLIQUID");
    let config = execution
        .config_as::<HyperliquidExecFactoryConfig>()
        .expect("Hyperliquid HIP-4 execution should downcast to NT config");
    assert_eq!(
        config.config.outcome_settlement_poll_secs, 5,
        "HIP-4 settlement polling stays TOML-owned"
    );
}

#[test]
fn hyperliquid_surface_approval_cannot_authorize_different_surface() {
    let loaded = fixture_loaded_config_with_hyperliquid_hip3();
    let resolved = fixture_resolved_hyperliquid_secrets();
    let consumed =
        consumed_hyperliquid_approval(HyperliquidProductSurface::Spot, "hl-hip3-approval-001");

    let error = map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        &loaded,
        &resolved,
        &hyperliquid_static_instrument_target_plan(
            ProductSurface::Hip3BuilderPerps,
            "BTC-PERP.HYPERLIQUID",
        ),
        fixed_market_clock(1_800_000_000),
        ProviderRuntimeApprovals {
            live_submit: Some(&consumed),
        },
    )
    .expect_err("spot approval must not authorize HIP-3 execution");

    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "hyperliquid_perps");
            assert_eq!(field, "execution.product_surfaces");
            assert!(
                message.contains("product surface does not match"),
                "surface mismatch must be named: {message}"
            );
        }
        other => panic!("expected Hyperliquid surface mismatch invariant, got {other}"),
    }
}

#[test]
fn adapter_mapper_rejects_subscribe_new_markets_true_if_validation_was_bypassed() {
    // Root validation rejects this value. This test mutates an already
    // loaded config to ensure the adapter mapper also fails closed if a
    // programmatic caller bypasses the canonical validation path.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let polymarket_data = loaded
        .root
        .clients
        .get_mut("polymarket_main")
        .and_then(|client| client.data.as_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture polymarket data table should exist");
    polymarket_data.insert(
        "subscribe_new_markets".to_string(),
        toml::Value::Boolean(true),
    );

    let resolved = fixture_resolved_secrets();
    let error = map_bolt_v3_adapters(&loaded, &resolved)
        .expect_err("mapper must not forward subscribe_new_markets=true to NT");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key, field, ..
        } => {
            assert_eq!(client_key, "polymarket_main");
            assert_eq!(field, "data.subscribe_new_markets");
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn binance_data_client_config_plus_resolved_secrets_maps_to_nt_native_fields() {
    let loaded = fixture_loaded_config_with_binance_reference();
    let resolved = fixture_resolved_secrets();

    let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

    let binance = configs
        .clients
        .get("binance_reference")
        .expect("binance_reference must be present in mapper output");
    let data = binance
        .data
        .as_ref()
        .expect("binance [data] block must produce an NT data config")
        .config_as::<BinanceDataClientConfig>()
        .expect("binance [data] should downcast to NT BinanceDataClientConfig");

    assert_eq!(data.product_type, NtBinanceProductType::Spot);
    assert_eq!(data.environment, NtBinanceEnvironment::Live);
    assert_eq!(data.spot_market_data_mode, NtBinanceSpotMarketDataMode::Sbe);
    // The bolt-v3 binance data schema now requires explicit
    // base_url_http and base_url_ws so NT cannot silently fall back to
    // its compiled-in default Binance endpoints. Both must arrive at
    // NT as `Some(...)` carrying the configured fixture value.
    assert_eq!(
        data.base_url_http.as_deref(),
        Some("https://api.binance.com")
    );
    assert_eq!(
        data.base_url_ws.as_deref(),
        Some("wss://stream-sbe.binance.com/ws")
    );
    assert_eq!(
        data.api_key.as_deref(),
        Some(fixture_binance_secrets().api_key.as_str())
    );
    assert_eq!(
        data.api_secret.as_deref(),
        Some(fixture_binance_secrets().api_secret.as_str())
    );
    assert_eq!(data.instrument_status_poll_secs, 3600);
    assert_eq!(data.transport_backend, TransportBackend::Sockudo);
}

#[test]
fn missing_or_invalid_root_config_remains_caught_by_validation_not_mapper_defaults() {
    use bolt_v2::bolt_v3_validate::validate_root_only;

    // Missing [secrets] for polymarket execution client: the existing
    // validator must catch this *before* the mapper ever runs. The
    // mapper itself must not silently fall back to defaults.
    let toml_text = r#"
schema_version = 1
trader_id = "BOLT-001"
strategy_files = ["strategies/binary_oracle.toml"]

[runtime]
mode = "Live"
order_execution_mode = "live"

[nautilus]
load_state = true
save_state = true
shutdown_on_error = false
timeout_connection_secs = 30
timeout_reconciliation_secs = 60
timeout_portfolio_secs = 10
timeout_disconnection_secs = 10
delay_post_stop_secs = 5
timeout_shutdown_secs = 10

[nautilus.data_engine]
time_bars_build_with_no_updates = true
time_bars_timestamp_on_close = true
time_bars_skip_first_non_full_bar = false
time_bars_interval_type = "LEFT_OPEN"
time_bars_build_delay = 0
time_bars_origins = {}
validate_data_sequence = false
buffer_deltas = false
emit_quotes_from_book = false
emit_quotes_from_book_depths = false
external_clients = []
debug = false
qsize = 100000

[nautilus.exec_engine]
load_cache = true
snapshot_orders = false
snapshot_positions = false
snapshot_positions_interval_secs = 0
external_clients = []
debug = false
reconciliation = true
reconciliation_startup_delay_secs = 10
reconciliation_lookback_mins = 0
reconciliation_instrument_ids = []
filter_unclaimed_external_orders = false
filter_position_reports = false
filtered_client_order_ids = []
generate_missing_orders = true
inflight_check_interval_ms = 2000
inflight_check_threshold_ms = 5000
inflight_check_retries = 5
open_check_interval_secs = 0
open_check_lookback_mins = 60
open_check_threshold_ms = 5000
open_check_missing_retries = 5
open_check_open_only = true
max_single_order_queries_per_cycle = 10
single_order_query_delay_ms = 100
position_check_interval_secs = 0
position_check_lookback_mins = 60
position_check_threshold_ms = 5000
position_check_retries = 3
purge_closed_orders_interval_mins = 0
purge_closed_orders_buffer_mins = 0
purge_closed_positions_interval_mins = 0
purge_closed_positions_buffer_mins = 0
purge_account_events_interval_mins = 0
purge_account_events_lookback_mins = 0
purge_from_database = false
own_books_audit_interval_secs = 0
qsize = 100000
allow_overfills = false
manage_own_order_books = false

[risk]
default_max_notional_per_order = "10.00"

[risk.nautilus]
max_order_submit_rate = "33/00:01:00"
max_order_modify_rate = "33/00:01:00"
max_notional_per_order = {}
debug = false
qsize = 100000

[logging]
stdout_level = "INFO"
fileout_level = "INFO"

[persistence]
catalog_directory = "/var/lib/bolt/catalog"
runtime_capture_start_poll_interval_ms = 50
data_client_readiness_probe_poll_interval_ms = 50

[persistence.decision_evidence]
order_intents_relative_path = "bolt-v3/decision-evidence/order-intents.jsonl"

[persistence.streaming]
catalog_fs_protocol = "file"
flush_interval_ms = 1000
replace_existing = false
rotation_kind = "none"

[aws]
region = "eu-west-2"

[clients.polymarket_main]
venue = "POLYMARKET"

[clients.polymarket_main.execution]
account_id = "POLYMARKET-001"
signature_type = "poly_proxy"
funder = "0x1111111111111111111111111111111111111111"
base_url_http = "https://clob.polymarket.com"
base_url_ws = "wss://ws-subscriptions-clob.polymarket.com/ws/user"
base_url_data_api = "https://data-api.polymarket.com"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
ack_timeout_secs = 5
fee_cache_ttl_secs = 300
transport_backend = "sockudo"
"#;
    let root: BoltV3RootConfig =
        toml::from_str(toml_text).expect("polymarket-execution-only TOML should parse");
    let messages = validate_root_only(&root);
    assert!(
        messages
            .iter()
            .any(|m| m.contains("required [secrets] block")),
        "validator must reject missing [secrets] for polymarket execution: {messages:#?}"
    );

    // Construct a LoadedBoltV3Config that bypassed validation so we can
    // confirm the mapper itself does not silently fill in adapter
    // defaults for the missing [secrets]: it must surface as a mapping
    // error driven by the resolved-secrets gap, not a default.
    let loaded = LoadedBoltV3Config {
        root_path: support::repo_path("tests/fixtures/bolt_v3/root.toml"),
        config_bundle_checksum: String::new(),
        root,
        strategies: Vec::new(),
    };
    let empty_resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let error = map_bolt_v3_adapters(&loaded, &empty_resolved)
        .expect_err("mapper must not synthesize defaults for missing resolved secrets");
    let rendered = error.to_string();
    assert!(rendered.contains("(provider=POLYMARKET)"));
    assert!(!rendered.contains("(kind="));
    assert!(!rendered.contains("(venue="));
    match error {
        BoltV3AdapterMappingError::MissingResolvedSecrets {
            client_key,
            expected_provider_key,
        } => {
            assert_eq!(client_key, "polymarket_main");
            assert_eq!(expected_provider_key, polymarket::KEY);
        }
        other => panic!("expected MissingResolvedSecrets, got {other}"),
    }
}

#[test]
fn live_node_build_path_runs_adapter_mapping_after_secret_resolution() {
    // The fake resolver in `tests/support/mod.rs` returns a synthetic
    // PKCS8 Ed25519 secret for binance and placeholders for polymarket;
    // the mapper sits between secret resolution and LiveNode::build, so
    // a successful build proves the mapper accepted the resolved
    // secrets without the build path silently bypassing it.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let temp = support::TempCaseDir::new("bolt-v3-adapter-mapping-build-path");
    loaded.root.persistence.catalog_directory = temp.path().to_string_lossy().to_string();
    let _node = build_bolt_v3_live_node_with(&loaded, |_| false, support::fake_bolt_v3_resolver)
        .expect("v3 LiveNode should build through the adapter mapping boundary");
}

#[test]
fn live_node_build_path_propagates_adapter_mapping_failures() {
    // Inject a resolver that hands back an empty string for a polymarket
    // SSM path. Resolution itself succeeds (the resolver is the source
    // of truth for "I got a value"), and then the mapper boundary plumbs
    // the resolved secrets into PolymarketExecClientConfig where the
    // empty string round-trips into the NT-native field as Some("").
    //
    // This regression guards against future refactors that would skip
    // the adapter mapping step entirely. The current mapper does not
    // re-validate string shape; if future requirements need shape
    // checks at the mapper boundary, this test is the place to assert
    // that they fire.
    let loaded = fixture_loaded_config_with_binance_reference();

    // Force the polymarket_main api_secret resolution to fail; the live-node
    // builder must surface the error rather than silently skipping the
    // mapping step. polymarket_main is the strategy-bound execution client,
    // so the scoped trade build path resolves its secrets — making the
    // SecretResolution error surface through the mapping boundary. (binance
    // is an unbound broad-readiness probe client and is not resolved by the
    // scoped path, so failing its secret would surface nothing here.)
    let bad_resolver = |region: &str, path: &str| -> Result<String, &'static str> {
        if path == "/bolt/polymarket/api-secret" {
            Err("simulated SSM permissions denied")
        } else {
            support::fake_bolt_v3_resolver(region, path)
        }
    };
    let error = build_bolt_v3_live_node_with(&loaded, |_| false, bad_resolver)
        .expect_err("resolver failure must surface through the live-node build path");
    matches!(error, BoltV3LiveNodeError::SecretResolution(_))
        .then_some(())
        .expect("expected SecretResolution error variant from build_bolt_v3_live_node_with");
}

#[test]
fn adapter_mapper_module_remains_a_no_trade_boundary() {
    // The mapper boundary is enforced by source-level inspection so a
    // future regression that pulls a factory or LiveNode runner into
    // the adapter module would fail in CI rather than silently break
    // the no-trade contract. Forbidden tokens are kept here in the
    // integration test (not in the module's own source) to avoid the
    // assertion self-tripping when it scans its own definition file.
    let source = include_str!("../src/bolt_v3_adapters.rs");
    for forbidden in [
        "PolymarketDataClientFactory",
        "PolymarketExecutionClientFactory",
        "BinanceDataClientFactory",
        "add_data_client",
        "add_exec_client",
        "register_data_client",
        "register_exec_client",
        ".connect(",
        ".disconnect(",
        ".run(",
        "LiveNode::build",
        "LiveNode::new",
        "submit_order",
    ] {
        assert!(
            !source.contains(forbidden),
            "src/bolt_v3_adapters.rs must remain a no-trade boundary; \
             source unexpectedly references `{forbidden}`"
        );
    }
}
