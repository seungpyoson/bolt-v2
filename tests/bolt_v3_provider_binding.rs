//! Provider-binding tests for bolt-v3.
//!
//! These tests guard the product boundary that was articulated after
//! Slice 9: core market-identity in `bolt_v3_market_identity` is
//! provider-neutral, and translation of that neutral plan into
//! provider-shaped NT adapter values (today: a `MarketSlugFilter`
//! installed on `PolymarketDataClientConfig.filters`) is the sole
//! responsibility of the adapter / provider-binding layer.
//!
//! What these tests prove:
//!   1. The new market-identity-aware mapper installs exactly one
//!      provider filter per configured updown target on the matching
//!      venue, and the filter yields `[current_slug, next_slug]` for
//!      the injected fixed clock.
//!   2. Multi-target filter ordering follows declared strategy
//!      sequence and never reorders by an accidental sort key.
//!   3. The `subscribe_new_markets = true` validation invariant still
//!      fires through the market-identity entry point so the binding
//!      layer cannot be used to smuggle an "all markets" subscription.
//!   4. An empty market-identity plan installs no provider filter,
//!      preserving the previous default behaviour for non-rotating
//!      configurations.
//!
//! Out of scope: live `LiveNode` runtime, NT cache reads,
//! `request_instruments`, real wall-clock injection, dynamic market
//! selection, fused / reference price derivation, and any trade-action
//! construction. Those boundaries belong to later slices.

use crate::support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI64, Ordering},
    },
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3MarketClockFn, map_bolt_v3_adapters_with_market_identity,
    },
    bolt_v3_config::{ClientBlock, LoadedStrategy, load_bolt_v3_config},
    bolt_v3_market_families::{
        MarketIdentityPlan, market_identity_plan_from_config as plan_market_identity,
        outcome_group::OutcomeGroupTargetPlan, static_binary_event::StaticBinaryEventTargetPlan,
    },
    bolt_v3_outcome_group_sources::{
        GammaQueryBlock, OutcomeGroupSourceConfig, OutcomeGroupSourceKind as SourceConfigKind,
    },
    bolt_v3_providers::{
        ProviderArtifactReference, ProviderLiveSubmitApprovalContext,
        ProviderProductSubmitProofArtifactRequest,
        binance::ResolvedBoltV3BinanceSecrets,
        binding_for_provider_key,
        chainlink::ResolvedBoltV3ChainlinkSecrets,
        chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets,
        hyperliquid::{ResolvedBoltV3HyperliquidSecrets, load_live_submit_approval},
        hyperliquid_artifacts::read_hyperliquid_live_submit_approval_artifact,
        polymarket::ResolvedBoltV3PolymarketSecrets,
        polyresearch::ResolvedBoltV3PolyResearchSecrets,
        validate_client_block,
    },
    bolt_v3_secrets::{
        ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars_with, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_strategy_registration::{
        BoltV3IvQueryHandleRegistry, StrategyRegistrationContext, StrategyRuntimeCapabilities,
        assemble_strategy_build_context,
    },
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
};
use nautilus_model::identifiers::{ClientId, Venue};
use nautilus_polymarket::config::PolymarketDataClientConfig;
use sha2::{Digest, Sha256};

/// Mutate a single field in the strategy's raw `[target]` TOML
/// envelope. Mirrors the helper in `tests/bolt_v3_market_identity.rs`;
/// the strategy envelope keeps `target` as a generic raw-TOML
/// container so market-family-shaped fields live in the per-family
/// binding module.
fn set_target_field(strategy: &mut LoadedStrategy, key: &str, value: toml::Value) {
    strategy
        .config
        .target
        .as_table_mut()
        .expect("strategy [target] should be a TOML table")
        .insert(key.to_string(), value);
}

fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "polymarket_main".to_string(),
        // Shared strategy assembly eagerly builds the Polymarket fee provider,
        // which resolves these secrets through NT: `private_key` must be a valid
        // 32-byte (64 hex char) secp256k1 scalar, and NT's `Credential::new`
        // decodes `api_secret` with the padded URL-safe base64 engine, so the
        // value must be valid padded URL-safe base64.
        Arc::new(ResolvedBoltV3PolymarketSecrets {
            private_key: zeroize::Zeroizing::new(
                "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
            ),
            api_key: zeroize::Zeroizing::new("binding-poly-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("YmluZGluZy1wb2x5LWFwaS1zZWNyZXQ=".to_string()),
            passphrase: zeroize::Zeroizing::new("binding-poly-passphrase".to_string()),
        }),
    );
    clients.insert(
        "binance_reference".to_string(),
        Arc::new(ResolvedBoltV3BinanceSecrets {
            api_key: zeroize::Zeroizing::new("binding-binance-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("binding-binance-api-secret".to_string()),
        }),
    );
    for client_key in [
        "binance_spot_data",
        "binance_usdm_data",
        "binance_coinm_data",
    ] {
        clients.insert(
            client_key.to_string(),
            Arc::new(ResolvedBoltV3BinanceSecrets {
                api_key: zeroize::Zeroizing::new("binding-binance-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("binding-binance-api-secret".to_string()),
            }),
        );
    }
    clients.insert(
        "hyperliquid_perps".to_string(),
        Arc::new(ResolvedBoltV3HyperliquidSecrets {
            private_key: zeroize::Zeroizing::new(
                "0x1111111111111111111111111111111111111111111111111111111111111111".to_string(),
            ),
            account_address: zeroize::Zeroizing::new(
                "0x2222222222222222222222222222222222222222".to_string(),
            ),
            vault_address: None,
        }),
    );
    clients.insert(
        "chainlink_strike".to_string(),
        Arc::new(ResolvedBoltV3ChainlinkSecrets {
            api_key: zeroize::Zeroizing::new("binding-chainlink-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("binding-chainlink-api-secret".to_string()),
        }),
    );
    clients.insert(
        "chainlink_reference".to_string(),
        Arc::new(ResolvedBoltV3ChainlinkReferenceSecrets {
            api_key: zeroize::Zeroizing::new("binding-chainlink-reference-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new(
                "binding-chainlink-reference-api-secret".to_string(),
            ),
        }),
    );
    clients.insert(
        "polyresearch_reference".to_string(),
        Arc::new(ResolvedBoltV3PolyResearchSecrets {
            api_key: zeroize::Zeroizing::new("binding-polyresearch-api-key".to_string()),
        }),
    );
    ResolvedBoltV3Secrets { clients }
}

fn fixed_clock(now_unix_secs: i64) -> BoltV3MarketClockFn {
    Arc::new(move || now_unix_secs)
}

fn assembly_context<'a>(
    loaded: &'a bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    resolved: &'a ResolvedBoltV3Secrets,
    capabilities: StrategyRuntimeCapabilities,
) -> StrategyRegistrationContext<'a> {
    let decision_evidence: Arc<
        dyn bolt_v2::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter,
    > = Arc::new(support::RecordingDecisionEvidenceWriter::default());
    StrategyRegistrationContext {
        loaded,
        strategy: &loaded.strategies[0],
        strategy_kind: "test_strategy",
        capabilities,
        resolved,
        decision_evidence: decision_evidence.clone(),
        submit_admission: Arc::new(BoltV3SubmitAdmissionState::new(decision_evidence)),
        iv_query_handles: Arc::new(BoltV3IvQueryHandleRegistry::empty()),
        order_execution_policy:
            bolt_v2::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        realized_volatility_runtime: Arc::new(Mutex::new(
            bolt_v2::bolt_v3_realized_volatility_runtime::RealizedVolSurfaceRuntime::from_loaded_config(
                loaded,
            )
            .expect("test realized-volatility runtime should assemble"),
        )),
        settlement_runtime_sink: None,
        settlement_recovery: None,
        settlement_health_transition_emitter: None,
        economics_inputs:
            bolt_v2::bolt_v3_economics_runtime::AuthoritativeEconomicsInputStore::default(),
    }
}

#[test]
fn shared_strategy_assembly_installs_polymarket_rv_and_settlement_capabilities() {
    let loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 fixture should load");
    let resolved = fixture_resolved_secrets();
    let context = assembly_context(
        &loaded,
        &resolved,
        StrategyRuntimeCapabilities {
            realized_volatility: true,
            settlement: true,
        },
    );

    let assembled = assemble_strategy_build_context(&context)
        .expect("configured Polymarket client should assemble a strategy build context");

    assert_eq!(assembled.execution_venue(), Venue::from("POLYMARKET"));
    assert!(assembled.realized_volatility_capability().is_some());
    assert!(assembled.order_routing().is_ok());
    assert!(assembled.settlement_capability().is_some());
    assert_eq!(assembled.settlement_account_id(), Some("POLYMARKET-001"));
    assert_eq!(
        assembled.settlement_currency(),
        Some(nautilus_model::types::Currency::pUSD())
    );
}

#[test]
fn shared_strategy_assembly_supports_inline_hyperliquid_without_settlement_capability() {
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 fixture should load");
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        hyperliquid_execution_client(
            "/bolt/hyperliquid/master_api_wallet/private_key",
            "/bolt/hyperliquid/master_api_wallet/account_address",
        ),
    );
    loaded.strategies[0].config.execution_client_id = "hyperliquid_perps".into();
    let resolved = fixture_resolved_secrets();
    let context = assembly_context(
        &loaded,
        &resolved,
        StrategyRuntimeCapabilities {
            realized_volatility: true,
            settlement: false,
        },
    );

    let assembled = assemble_strategy_build_context(&context)
        .expect("inline Hyperliquid client should assemble through the shared boundary");

    assert_eq!(assembled.execution_venue(), Venue::from("HYPERLIQUID"));
    assert!(assembled.realized_volatility_capability().is_some());
    assert!(assembled.order_routing().is_ok());
    assert!(assembled.settlement_capability().is_none());
    assert_eq!(assembled.settlement_account_id(), None);
    assert_eq!(assembled.settlement_currency(), None);
}

#[test]
fn shared_strategy_assembly_fails_closed_when_execution_client_is_missing() {
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("bolt-v3 fixture should load");
    loaded.strategies[0].config.execution_client_id = "missing_execution".into();
    let resolved = fixture_resolved_secrets();
    let context = assembly_context(
        &loaded,
        &resolved,
        StrategyRuntimeCapabilities {
            realized_volatility: true,
            settlement: false,
        },
    );

    let error = assemble_strategy_build_context(&context)
        .err()
        .expect("missing execution client must fail closed during shared assembly");
    assert!(
        error.to_string().contains(
            "execution_client_id `missing_execution` is not present in loaded clients for execution-venue resolution"
        ),
        "{error}"
    );
}

fn data_only_client_from_toml(value: &str) -> ClientBlock {
    toml::from_str(value).expect("test data-only client block should parse")
}

fn hyperliquid_data_client(update_instruments_interval_mins: u64) -> ClientBlock {
    data_only_client_from_toml(&format!(
        r#"
venue = "HYPERLIQUID"

[data]
environment = "testnet"
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
ws_timeout_secs = 30
update_instruments_interval_mins = {update_instruments_interval_mins}
transport_backend = "sockudo"
"#
    ))
}

fn hyperliquid_execution_client(private_key_path: &str, account_address_path: &str) -> ClientBlock {
    hyperliquid_execution_client_with_secret_fields(&format!(
        r#"
private_key_ssm_path = "{private_key_path}"
account_address_ssm_path = "{account_address_path}"
"#
    ))
}

fn hyperliquid_execution_client_for_surface(
    product_surface: &str,
    private_key_path: &str,
    account_address_path: &str,
) -> ClientBlock {
    let mut client = hyperliquid_execution_client(private_key_path, account_address_path);
    let execution = client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table");
    execution.insert(
        stringify!(product_surfaces).to_string(),
        toml::Value::Array(vec![toml::Value::String(product_surface.to_string())]),
    );
    if product_surface == "hip4_outcomes" {
        execution.insert(
            stringify!(outcome_settlement_poll_secs).to_string(),
            toml::Value::Integer(30),
        );
    }
    client
}

fn add_hyperliquid_live_submit_approval(client: &mut ClientBlock) {
    let execution = client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table");
    let mut standard_perps = toml::map::Map::new();
    standard_perps.insert(
        "approval_id".to_string(),
        toml::Value::String("hl-standard-perps-approval-001".to_string()),
    );
    standard_perps.insert(
        "approval_artifact_path".to_string(),
        toml::Value::String("operator/hyperliquid-live-submit-approval.json".to_string()),
    );
    standard_perps.insert(
        "approval_artifact_max_bytes".to_string(),
        toml::Value::Integer(65536),
    );
    standard_perps.insert("max_order_count".to_string(), toml::Value::Integer(1));
    standard_perps.insert(
        "max_order_notional".to_string(),
        toml::Value::String("10.00".to_string()),
    );
    standard_perps.insert(
        "product_proof_artifact_path".to_string(),
        toml::Value::String("operator/hyperliquid-product-submit-proof.json".to_string()),
    );
    standard_perps.insert(
        "product_proof_artifact_sha256".to_string(),
        toml::Value::String("d".repeat(64)),
    );
    standard_perps.insert(
        "product_proof_artifact_max_bytes".to_string(),
        toml::Value::Integer(65536),
    );
    let mut live_submit = toml::map::Map::new();
    live_submit.insert(
        "standard_perps".to_string(),
        toml::Value::Table(standard_perps),
    );
    execution.insert("live_submit".to_string(), toml::Value::Table(live_submit));
}

fn hyperliquid_standard_perps_live_submit_mut(
    client: &mut ClientBlock,
) -> &mut toml::map::Map<String, toml::Value> {
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .get_mut("live_submit")
        .and_then(toml::Value::as_table_mut)
        .and_then(|live_submit| live_submit.get_mut("standard_perps"))
        .and_then(toml::Value::as_table_mut)
        .expect("test Hyperliquid live_submit.standard_perps should be a table")
}

fn hyperliquid_execution_client_with_secret_fields(secret_fields: &str) -> ClientBlock {
    data_only_client_from_toml(&format!(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
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

[execution.economics]
economics_slice = "quote_only"
reporting_policy = "primary-pnl"
quote_refresh_secs = 30
quote_max_age_secs = 60
quote_validity_ms = 30000
resting_order_refresh_margin_ms = 5000
carry_surfaces = ["standard_perps"]

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

[execution.economics.assets.settlement]
native_unit = "USD"
identity_kind = "currency"
evidence_fixture_id = "hyperliquid-settlement-fixture"

[execution.economics.carry]
holding_horizon_secs = 3600
component_id = "funding-carry"
formula_id = "funding-rate-bound"
point_rate_factor_id = "funding-point-rate"
bound_rate_factor_id = "funding-bound-rate"
risk_policy_id = "funding-risk-policy"
stress_fixture_id = "funding-standard-stress"

[execution.economics.edge_basis.primary]
resolver_id = "product-metadata"
product_metadata_source = "hyperliquid-meta"

[execution.economics.product_surface_policies]
standard_perps = "primary"

[execution.economics.valuation.routes]

[secrets]
{secret_fields}
"#
    ))
}

fn hyperliquid_execution_client_with_latency_profile() -> ClientBlock {
    data_only_client_from_toml(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
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
}

fn hyperliquid_hip4_client_without_settlement_poll() -> ClientBlock {
    data_only_client_from_toml(
        r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["hip4_outcomes"]
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

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
    )
}

fn add_requested_market_data_clients(loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config) {
    let clients: &[(&str, &str)] = &[
        (
            "binance_spot_data",
            r#"
venue = "BINANCE"

[data]
product_type = "spot"
environment = "mainnet"
base_url_http = "https://api.binance.com"
base_url_ws = "wss://stream-sbe.binance.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "binance_usdm_data",
            r#"
venue = "BINANCE"

[data]
product_type = "usd_m"
environment = "testnet"
base_url_http = "https://demo-fapi.binance.com"
base_url_ws = "wss://fstream.binancefuture.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "binance_coinm_data",
            r#"
venue = "BINANCE"

[data]
product_type = "coin_m"
environment = "testnet"
base_url_http = "https://testnet.binancefuture.com"
base_url_ws = "wss://dstream.binancefuture.com/ws"
spot_market_data_mode = "sbe"
instrument_status_poll_secs = 3600
transport_backend = "sockudo"

[secrets]
api_key_ssm_path = "/bolt/binance_reference/api_key"
api_secret_ssm_path = "/bolt/binance_reference/api_secret"
"#,
        ),
        (
            "bitmex_data",
            r#"
venue = "BITMEX"

[data]
environment = "testnet"
active_only = false
transport_backend = "sockudo"
"#,
        ),
        (
            "bybit_data",
            r#"
venue = "BYBIT"

[data]
product_types = ["spot", "linear", "inverse", "option"]
environment = "testnet"
transport_backend = "sockudo"
"#,
        ),
        (
            "coinbase_data",
            r#"
venue = "COINBASE"

[data]
environment = "Live"
transport_backend = "sockudo"
"#,
        ),
        (
            "deribit_data",
            r#"
venue = "DERIBIT"

[data]
product_types = ["future", "option", "spot", "future_combo", "option_combo"]
environment = "testnet"
transport_backend = "sockudo"
"#,
        ),
        (
            "okx_data",
            r#"
venue = "OKX"

[data]
instrument_types = ["SPOT", "MARGIN", "SWAP", "FUTURES", "EVENTS"]
contract_types = ["linear", "inverse"]
load_spreads = true
environment = "demo"
transport_backend = "sockudo"
"#,
        ),
        (
            "kraken_spot_data",
            r#"
venue = "KRAKEN"

[data]
product_type = "spot"
environment = "live"
transport_backend = "sockudo"
"#,
        ),
        (
            "kraken_futures_data",
            r#"
venue = "KRAKEN"

[data]
product_type = "futures"
environment = "demo"
transport_backend = "sockudo"
"#,
        ),
    ];

    for (client_key, client_toml) in clients {
        loaded.root.clients.insert(
            (*client_key).to_string(),
            data_only_client_from_toml(client_toml),
        );
    }
}

#[test]
fn nt_source_supported_rust_data_client_provider_bindings_are_registered() {
    for provider_key in [
        "BINANCE",
        "BITMEX",
        "BYBIT",
        "COINBASE",
        "DERIBIT",
        "HYPERLIQUID",
        "KRAKEN",
        "OKX",
        "POLYMARKET",
    ] {
        assert!(
            binding_for_provider_key(provider_key).is_some(),
            "{provider_key} is in the requested production-readiness data-client scope and must have a bolt-v3 provider binding"
        );
    }
    for provider_key in [
        "AX",
        "BETFAIR",
        "BLOCKCHAIN",
        "DATABENTO",
        "DYDX",
        "IB",
        "SANDBOX",
        "TARDIS",
    ] {
        assert!(
            binding_for_provider_key(provider_key).is_none(),
            "{provider_key} is outside today's requested active data-client binding scope"
        );
    }
}

#[test]
fn provider_binding_accepts_hyperliquid_execution_config_with_ssm_paths() {
    let client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );

    assert_eq!(
        validate_client_block("hyperliquid_perps", &client),
        Vec::<String>::new()
    );
}

#[test]
fn provider_binding_accepts_hyperliquid_data_config_without_secrets() {
    let client = hyperliquid_data_client(5);

    assert_eq!(
        validate_client_block("hyperliquid_market_data", &client),
        Vec::<String>::new()
    );
}

#[test]
fn provider_binding_rejects_hyperliquid_data_without_refresh_cadence() {
    let client = hyperliquid_data_client(0);
    let rendered = validate_client_block("hyperliquid_market_data", &client).join("\n");

    assert!(rendered.contains("clients.hyperliquid_market_data.data.update_instruments_interval_mins must be a positive integer"));
}

#[test]
fn provider_binding_accepts_hyperliquid_latency_profile_as_ops_metadata() {
    let client = hyperliquid_execution_client_with_latency_profile();

    assert_eq!(
        validate_client_block("hyperliquid_perps", &client),
        Vec::<String>::new()
    );
}

#[test]
fn provider_binding_rejects_hyperliquid_execution_without_secrets() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    client.secrets = None;

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(rendered.contains("missing the [secrets] block"));
}

#[test]
fn provider_binding_accepts_hyperliquid_live_submit_when_official_user_fees_weight_is_accounted() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);

    assert_eq!(
        validate_client_block("hyperliquid_perps", &client),
        Vec::<String>::new()
    );
}

#[test]
fn provider_binding_accepts_surface_scoped_hyperliquid_live_submit_with_multiple_product_surfaces()
{
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "product_surfaces".to_string(),
            toml::Value::Array(vec![
                toml::Value::String("standard_perps".to_string()),
                toml::Value::String("spot".to_string()),
            ]),
        );

    assert_eq!(
        validate_client_block("hyperliquid_perps", &client),
        Vec::<String>::new()
    );
}

#[test]
fn provider_binding_rejects_hyperliquid_live_submit_surface_not_enabled() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "product_surfaces".to_string(),
            toml::Value::Array(vec![toml::Value::String("spot".to_string())]),
        );

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(
        rendered.contains("execution.live_submit.standard_perps requires execution.product_surfaces to include standard_perps"),
        "surface-scoped live_submit blocks must be enabled by product_surfaces: {rendered}"
    );
}

#[test]
fn provider_binding_rejects_hyperliquid_empty_product_surfaces() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert("product_surfaces".to_string(), toml::Value::Array(vec![]));

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(
        rendered.contains(
            "execution.product_surfaces must select at least one Hyperliquid product surface"
        ),
        "empty product_surfaces must fail closed: {rendered}"
    );
}

#[test]
fn provider_binding_rejects_empty_hyperliquid_live_submit_table() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "live_submit".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(
        rendered.contains(
            "execution.live_submit must configure at least one Hyperliquid product surface"
        ),
        "empty live_submit table must fail closed: {rendered}"
    );
}

#[test]
fn provider_binding_rejects_hyperliquid_live_submit_zero_max_order_count() {
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client)
        .insert("max_order_count".to_string(), toml::Value::Integer(0));

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(
        rendered.contains("execution.live_submit.standard_perps.max_order_count must be positive"),
        "zero max_order_count must fail closed: {rendered}"
    );
}

#[test]
fn provider_binding_models_hyperliquid_egress_with_official_user_fees_weight() {
    let model = bolt_v2::bolt_v3_providers::venue_egress_model("HYPERLIQUID")
        .expect("Hyperliquid execution must have a REST egress model before live submit");

    assert_eq!(model.cap_per_minute, 1200);
    assert_eq!(model.max_rest_requests_per_order_command, 20);
}

#[test]
fn provider_binding_models_polymarket_market_quote_buy_egress() {
    let model = bolt_v2::bolt_v3_providers::venue_egress_model("POLYMARKET")
        .expect("Polymarket execution must have a REST egress model before live submit");

    assert_eq!(model.max_rest_requests_per_order_command, 3);
}

#[test]
fn provider_binding_rejects_hip4_execution_without_settlement_poll() {
    let client = hyperliquid_hip4_client_without_settlement_poll();
    let errors = validate_client_block("hyperliquid_hip4", &client);

    assert!(
        errors
            .iter()
            .any(|error| error.contains("execution.outcome_settlement_poll_secs")),
        "HIP-4 validation must require settlement polling: {errors:?}"
    );
}

#[test]
fn provider_binding_resolves_hyperliquid_execution_secrets_from_ssm_paths() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        hyperliquid_execution_client(
            "/bolt/hyperliquid/master_api_wallet/private_key",
            "/bolt/hyperliquid/master_api_wallet/account_address",
        ),
    );
    let mut requested_paths = Vec::new();

    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| {
        requested_paths.push(path.to_string());
        match path {
            "/bolt/hyperliquid/master_api_wallet/private_key" => Ok(
                "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
            ),
            "/bolt/hyperliquid/master_api_wallet/account_address" => {
                Ok("0x1111111111111111111111111111111111111111".to_string())
            }
            _ => Err("unexpected SSM path requested by Hyperliquid binding"),
        }
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");

    assert!(resolved.clients.contains_key("hyperliquid_perps"));
    assert_eq!(
        requested_paths,
        vec![
            "/bolt/hyperliquid/master_api_wallet/private_key",
            "/bolt/hyperliquid/master_api_wallet/account_address",
        ]
    );
}

#[test]
fn provider_binding_writes_hyperliquid_live_submit_approval_from_configured_runtime() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "approval_artifact_path".to_string(),
        toml::Value::String(
            approval_path
                .to_str()
                .expect("approval path should be utf-8")
                .to_string(),
        ),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let private_key = "0x4242424242424242424242424242424242424242424242424242424242424242";
    let account_address = "0x1111111111111111111111111111111111111111";
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => Ok(private_key.to_string()),
        "/bolt/hyperliquid/master_api_wallet/account_address" => Ok(account_address.to_string()),
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");

    let written = writer(
        ProviderLiveSubmitApprovalContext {
            loaded: &loaded,
            client_key: "hyperliquid_perps",
            client: loaded
                .root
                .clients
                .get("hyperliquid_perps")
                .expect("test client should exist"),
            resolved: &resolved,
            product_surface: Some("standard_perps"),
            now_unix_seconds,
            build_head_sha: &build_head_sha,
        },
        now_unix_seconds + 600,
    )
    .expect("configured Hyperliquid live-submit approval should write");

    assert_eq!(written.path, approval_path);
    let artifact = read_hyperliquid_live_submit_approval_artifact(&approval_path, 4096)
        .expect("written approval artifact should parse");
    assert_eq!(artifact.approval_id, "hl-standard-perps-approval-001");
    assert_eq!(artifact.provider_id, "hyperliquid_perps");
    assert_eq!(artifact.base_sha, build_head_sha);
    assert_eq!(artifact.toml_checksum, loaded.config_bundle_checksum);
    assert_eq!(artifact.expires_at, now_unix_seconds + 600);
    assert_eq!(artifact.used_at, None);
    assert_eq!(artifact.order_limits.max_order_count, 1);
    assert_eq!(artifact.order_limits.max_order_notional, "10.00");
    assert_eq!(
        artifact
            .product_submit_proof
            .as_ref()
            .expect("approval artifact should bind product submit proof")
            .artifact_path,
        "operator/hyperliquid-product-submit-proof.json"
    );
    assert_eq!(
        artifact
            .product_submit_proof
            .as_ref()
            .expect("approval artifact should bind product submit proof")
            .artifact_sha256,
        "d".repeat(64)
    );

    let artifact_text = fs::read_to_string(&approval_path).expect("artifact should read");
    assert!(!artifact_text.contains(private_key));
    assert!(!artifact_text.contains(account_address));
}

#[test]
fn provider_binding_rejects_operator_surface_not_enabled_by_product_surfaces() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "approval_artifact_path".to_string(),
        toml::Value::String(
            approval_path
                .to_str()
                .expect("approval path should be utf-8")
                .to_string(),
        ),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x4242424242424242424242424242424242424242424242424242424242424242".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");

    let error = writer(
        ProviderLiveSubmitApprovalContext {
            loaded: &loaded,
            client_key: "hyperliquid_perps",
            client: loaded
                .root
                .clients
                .get("hyperliquid_perps")
                .expect("test client should exist"),
            resolved: &resolved,
            product_surface: Some("spot"),
            now_unix_seconds,
            build_head_sha: &build_head_sha,
        },
        now_unix_seconds + 600,
    )
    .expect_err("operator surface outside execution.product_surfaces must fail closed");

    assert!(
        error
            .to_string()
            .contains("is not enabled by execution.product_surfaces"),
        "error should identify the disabled operator surface: {error}"
    );
    assert!(
        !approval_path.exists(),
        "no approval artifact may be written when the operator surface is not enabled"
    );
}

#[test]
fn provider_binding_rejects_unsupported_operator_product_surface_name() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "approval_artifact_path".to_string(),
        toml::Value::String(
            approval_path
                .to_str()
                .expect("approval path should be utf-8")
                .to_string(),
        ),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x4242424242424242424242424242424242424242424242424242424242424242".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");

    let error = writer(
        ProviderLiveSubmitApprovalContext {
            loaded: &loaded,
            client_key: "hyperliquid_perps",
            client: loaded
                .root
                .clients
                .get("hyperliquid_perps")
                .expect("test client should exist"),
            resolved: &resolved,
            product_surface: Some("perpetuals_that_do_not_exist"),
            now_unix_seconds,
            build_head_sha: &build_head_sha,
        },
        now_unix_seconds + 600,
    )
    .expect_err("an unsupported operator product surface name must fail closed");

    assert!(
        error
            .to_string()
            .contains("unsupported Hyperliquid product surface"),
        "error should reject the unsupported operator surface name: {error}"
    );
    assert!(
        !approval_path.exists(),
        "no approval artifact may be written for an unsupported product surface name"
    );
}

#[test]
fn provider_binding_rejects_operator_surface_without_matching_live_submit_block() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "product_surfaces".to_string(),
            toml::Value::Array(vec![
                toml::Value::String("standard_perps".to_string()),
                toml::Value::String("spot".to_string()),
            ]),
        );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "approval_artifact_path".to_string(),
        toml::Value::String(
            approval_path
                .to_str()
                .expect("approval path should be utf-8")
                .to_string(),
        ),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x4242424242424242424242424242424242424242424242424242424242424242".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");

    let error = writer(
        ProviderLiveSubmitApprovalContext {
            loaded: &loaded,
            client_key: "hyperliquid_perps",
            client: loaded
                .root
                .clients
                .get("hyperliquid_perps")
                .expect("test client should exist"),
            resolved: &resolved,
            product_surface: Some("spot"),
            now_unix_seconds,
            build_head_sha: &build_head_sha,
        },
        now_unix_seconds + 600,
    )
    .expect_err("a selected surface without a live_submit block must fail closed");

    assert!(
        error
            .to_string()
            .contains("no matching execution.live_submit block is configured"),
        "error should identify the missing per-surface live_submit block: {error}"
    );
    assert!(
        !approval_path.exists(),
        "no approval artifact may be written when the selected surface has no live_submit block"
    );
}

/// Differential guard for the deliberately removed "single-surface
/// fallback" in `selected_live_submit_surface_for_plan`. A Hyperliquid
/// execution client configured with exactly one `product_surfaces`
/// entry and a matching `live_submit` block, but with no strategy
/// target routing to it (`loaded.strategies` cleared), derives an empty
/// market-identity plan. With `product_surface = None`, surface
/// selection falls through to the plan-derived path, which must return
/// `Ok(None)` (zero active surfaces ⇒ nothing armed). `load_live_submit_approval`
/// therefore short-circuits at the `selected_live_submit_config_for_context`
/// guard and returns `Ok(None)` BEFORE reading or consuming any approval
/// artifact, leaving the (nonexistent) artifact path untouched.
///
/// If a single-surface fallback were reintroduced (e.g. selecting the
/// lone configured surface when the active set is empty), this function
/// would instead select `standard_perps`, then attempt to read the
/// missing approval artifact and return `Err`. Asserting `Ok(None)` here
/// is the load-bearing differential signal: it PASSES on the fail-closed
/// code and FAILS (`Err`) on the buggy fallback variant.
#[test]
fn provider_binding_does_not_arm_single_surface_hyperliquid_client_without_routed_target() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    // Approval path intentionally points at a file that does NOT exist:
    // the fail-closed contract must return Ok(None) before any read.
    let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    // Clearing strategies removes every target that could route to this
    // client, so the derived market-identity plan has zero active
    // surfaces for `hyperliquid_perps`.
    loaded.strategies.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    // Exactly ONE configured product surface — this is the precondition
    // the removed single-surface fallback would have keyed off of.
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "product_surfaces".to_string(),
            toml::Value::Array(vec![toml::Value::String("standard_perps".to_string())]),
        );
    // Keep only the standard_perps live_submit block so the config is
    // valid (every live_submit surface is in product_surfaces) and point
    // its approval artifact at the nonexistent temp path.
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "approval_artifact_path".to_string(),
        toml::Value::String(
            approval_path
                .to_str()
                .expect("approval path should be utf-8")
                .to_string(),
        ),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x4242424242424242424242424242424242424242424242424242424242424242".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect("Hyperliquid secrets should resolve from configured SSM paths");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);

    // `product_surface: None` is critical: passing `Some(..)` would
    // short-circuit surface selection before plan derivation and would
    // NOT exercise the removed fallback.
    let approval = load_live_submit_approval(ProviderLiveSubmitApprovalContext {
        loaded: &loaded,
        client_key: "hyperliquid_perps",
        client: loaded
            .root
            .clients
            .get("hyperliquid_perps")
            .expect("test client should exist"),
        resolved: &resolved,
        product_surface: None,
        now_unix_seconds,
        build_head_sha: &build_head_sha,
    })
    .expect(
        "a single configured surface with no routed target must not error (fail-closed Ok(None))",
    );

    assert!(
        approval.is_none(),
        "no surface may be armed when no strategy target routes to the client, even if exactly one product surface is configured; a reintroduced single-surface fallback would select `standard_perps` here"
    );
    assert!(
        !approval_path.exists(),
        "the fail-closed path must not read or write the approval artifact: selection returns Ok(None) before the artifact is ever touched"
    );
}

#[test]
fn provider_binding_preflights_hyperliquid_live_submit_arming_without_consuming_approval() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let operator_dir = temp.path().join("operator");
    fs::create_dir_all(&operator_dir).expect("operator dir should create");
    let product_proof_path = operator_dir.join("hyperliquid-product-submit-proof.json");
    let approval_path = operator_dir.join("hyperliquid-live-submit-approval.json");
    let root_path = temp.path().join("root.toml");
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture v3 config should load");
    loaded.root_path = root_path;
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let product_writer = binding
        .write_product_submit_proof_artifact
        .expect("Hyperliquid binding should expose product-submit proof materialization");
    let order_proof_sha256 = "a".repeat(64);
    let fill_proof_sha256 = "b".repeat(64);
    let rounding_proof_sha256 = "d".repeat(64);
    let fee_proof_sha256 = "e".repeat(64);
    let product_written = product_writer(ProviderProductSubmitProofArtifactRequest {
        provider_id: "hyperliquid_perps",
        product_surface: "standard_perps",
        toml_checksum: &loaded.config_bundle_checksum,
        order_proof: ProviderArtifactReference {
            artifact_path: "operator/order-proof.json",
            artifact_sha256: &order_proof_sha256,
        },
        fill_proof: ProviderArtifactReference {
            artifact_path: "operator/fill-proof.json",
            artifact_sha256: &fill_proof_sha256,
        },
        rounding_proof: ProviderArtifactReference {
            artifact_path: "operator/rounding-proof.json",
            artifact_sha256: &rounding_proof_sha256,
        },
        fee_proof: ProviderArtifactReference {
            artifact_path: "operator/fee-proof.json",
            artifact_sha256: &fee_proof_sha256,
        },
        settlement_proof: None,
        output_path: &product_proof_path,
    })
    .expect("product-submit proof should write");
    assert_eq!(
        product_written.sha256,
        hex::encode(Sha256::digest(
            fs::read(&product_proof_path).expect("product proof should read")
        ))
    );
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    hyperliquid_standard_perps_live_submit_mut(&mut client).insert(
        "product_proof_artifact_sha256".to_string(),
        toml::Value::String(product_written.sha256),
    );
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = fixture_resolved_secrets();
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let context = ProviderLiveSubmitApprovalContext {
        loaded: &loaded,
        client_key: "hyperliquid_perps",
        client: loaded
            .root
            .clients
            .get("hyperliquid_perps")
            .expect("test client should exist"),
        resolved: &resolved,
        product_surface: Some("standard_perps"),
        now_unix_seconds,
        build_head_sha: &build_head_sha,
    };
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");
    writer(context, now_unix_seconds + 600)
        .expect("configured Hyperliquid live-submit approval should write");
    let before = fs::read(&approval_path).expect("approval artifact should read before preflight");
    let preflight = binding
        .preflight_live_submit_arming
        .expect("Hyperliquid binding should expose live-submit arming preflight");

    let report = preflight(context)
        .expect("configured Hyperliquid live-submit arming should preflight")
        .expect("armed Hyperliquid client should return a preflight report");

    let after = fs::read(&approval_path).expect("approval artifact should read after preflight");
    assert_eq!(
        before, after,
        "preflight must not consume or rewrite the one-time approval artifact"
    );
    let approval = read_hyperliquid_live_submit_approval_artifact(&approval_path, 65536)
        .expect("approval artifact should remain readable");
    assert_eq!(approval.used_at, None);
    assert_eq!(report.provider_key, "HYPERLIQUID");
    assert_eq!(report.client_key, "hyperliquid_perps");
    assert_eq!(report.product_surface, "standard_perps");
    assert_eq!(
        report.approval_artifact_path,
        "operator/hyperliquid-live-submit-approval.json"
    );
    assert_eq!(
        report.product_submit_proof_artifact_path,
        "operator/hyperliquid-product-submit-proof.json"
    );
    assert_eq!(report.max_order_count, 1);
    assert_eq!(report.max_order_notional, "10.00");
}

#[test]
fn provider_binding_preflight_rejects_missing_product_submit_proof_without_consuming_approval() {
    let temp = tempfile::tempdir().expect("tempdir should create");
    let operator_dir = temp.path().join("operator");
    fs::create_dir_all(&operator_dir).expect("operator dir should create");
    let approval_path = operator_dir.join("hyperliquid-live-submit-approval.json");
    let root_path = temp.path().join("root.toml");
    let mut loaded = load_bolt_v3_config(&support::repo_path("tests/fixtures/bolt_v3/root.toml"))
        .expect("fixture v3 config should load");
    loaded.root_path = root_path;
    loaded.config_bundle_checksum = "c".repeat(64);
    loaded.root.clients.clear();
    let mut client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    add_hyperliquid_live_submit_approval(&mut client);
    loaded
        .root
        .clients
        .insert("hyperliquid_perps".to_string(), client);
    let resolved = fixture_resolved_secrets();
    let binding =
        binding_for_provider_key("HYPERLIQUID").expect("Hyperliquid binding should register");
    let now_unix_seconds = 1_800_000_000;
    let build_head_sha = "a".repeat(40);
    let context = ProviderLiveSubmitApprovalContext {
        loaded: &loaded,
        client_key: "hyperliquid_perps",
        client: loaded
            .root
            .clients
            .get("hyperliquid_perps")
            .expect("test client should exist"),
        resolved: &resolved,
        product_surface: Some("standard_perps"),
        now_unix_seconds,
        build_head_sha: &build_head_sha,
    };
    let writer = binding
        .write_live_submit_approval_artifact
        .expect("Hyperliquid binding should expose live-submit approval materialization");
    writer(context, now_unix_seconds + 600)
        .expect("configured Hyperliquid live-submit approval should write");
    let before = fs::read(&approval_path).expect("approval artifact should read before preflight");
    let preflight = binding
        .preflight_live_submit_arming
        .expect("Hyperliquid binding should expose live-submit arming preflight");

    let err = preflight(context).expect_err("missing product proof should fail preflight");

    let rendered = err.to_string();
    assert!(rendered.contains("product_submit_proof.artifact_path"));
    let after = fs::read(&approval_path).expect("approval artifact should read after preflight");
    assert_eq!(
        before, after,
        "failed preflight must not consume or rewrite the one-time approval artifact"
    );
    let approval = read_hyperliquid_live_submit_approval_artifact(&approval_path, 65536)
        .expect("approval artifact should remain readable");
    assert_eq!(approval.used_at, None);
}

#[test]
fn provider_binding_rejects_hyperliquid_raw_secret_material_in_toml() {
    let raw_private_key = "0x4444444444444444444444444444444444444444444444444444444444444444";
    let client = hyperliquid_execution_client_with_secret_fields(&format!(
        r#"
private_key = "{raw_private_key}"
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#
    ));

    let rendered = validate_client_block("hyperliquid_perps", &client).join("\n");

    assert!(rendered.contains("raw secret material"));
    assert!(rendered.contains("private_key"));
    assert!(!rendered.contains(raw_private_key));
}

#[test]
fn provider_binding_rejects_hyperliquid_env_fallback_vars() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_perps".to_string(),
        hyperliquid_execution_client(
            "/bolt/hyperliquid/master_api_wallet/private_key",
            "/bolt/hyperliquid/master_api_wallet/account_address",
        ),
    );

    let error = check_no_forbidden_credential_env_vars_with(&loaded.root, |var| {
        matches!(
            var,
            "HYPERLIQUID_TESTNET_PK" | "HYPERLIQUID_ACCOUNT_ADDRESS"
        )
    })
    .expect_err("Hyperliquid env fallback variables must fail startup validation");

    assert_eq!(error.findings.len(), 2);
    assert!(error.findings.iter().all(|finding| {
        finding.client_key == "hyperliquid_perps" && finding.provider_key == "HYPERLIQUID"
    }));
}

#[test]
fn provider_binding_rejects_duplicate_hyperliquid_signer_owner() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    for (client_key, wallet_key, account_address_path) in [
        (
            "hyperliquid_perps_a",
            "api_wallet_a",
            "/bolt/hyperliquid/api_wallet_a/account_address",
        ),
        (
            "hyperliquid_perps_b",
            "api_wallet_b",
            "/bolt/hyperliquid/api_wallet_b/account_address",
        ),
    ] {
        loaded.root.clients.insert(
            client_key.to_string(),
            hyperliquid_execution_client(
                &format!("/bolt/hyperliquid/{wallet_key}/private_key"),
                account_address_path,
            ),
        );
    }

    let duplicate_private_key_hex =
        "4343434343434343434343434343434343434343434343434343434343434343";
    let error = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/api_wallet_a/private_key" => {
            Ok(format!("0x{duplicate_private_key_hex}"))
        }
        "/bolt/hyperliquid/api_wallet_b/private_key" => Ok(duplicate_private_key_hex.to_string()),
        "/bolt/hyperliquid/api_wallet_a/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        "/bolt/hyperliquid/api_wallet_b/account_address" => {
            Ok("0x2222222222222222222222222222222222222222".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect_err("duplicate Hyperliquid signer/API-wallet owner must fail closed");
    let rendered = error.to_string();

    assert!(rendered.contains("signer/API-wallet owner"));
    assert!(rendered.contains("hyperliquid_perps_a"));
    assert!(!rendered.contains(duplicate_private_key_hex));
    assert!(!rendered.contains("0x1111111111111111111111111111111111111111"));
    assert!(!rendered.contains("0x2222222222222222222222222222222222222222"));
}

#[test]
fn provider_binding_rejects_duplicate_hyperliquid_signer_owner_when_paths_match() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.clients.clear();
    for (client_key, surface) in [
        ("hyperliquid_standard_perps", "standard_perps"),
        ("hyperliquid_spot", "spot"),
    ] {
        loaded.root.clients.insert(
            client_key.to_string(),
            hyperliquid_execution_client_for_surface(
                surface,
                "/bolt/hyperliquid/master_api_wallet/private_key",
                "/bolt/hyperliquid/master_api_wallet/account_address",
            ),
        );
    }

    let error = resolve_bolt_v3_secrets_with(&loaded, |_region, path| match path {
        "/bolt/hyperliquid/master_api_wallet/private_key" => {
            Ok("0x5656565656565656565656565656565656565656565656565656565656565656".to_string())
        }
        "/bolt/hyperliquid/master_api_wallet/account_address" => {
            Ok("0x1111111111111111111111111111111111111111".to_string())
        }
        _ => Err("unexpected SSM path requested by Hyperliquid binding"),
    })
    .expect_err(
        "multiple Hyperliquid clients must not share one signer after the single-client collapse",
    );

    assert!(error.to_string().contains("signer/API-wallet owner"));
}

#[test]
fn requested_market_data_clients_map_as_data_only_and_execution_stays_config_owned() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    add_requested_market_data_clients(&mut loaded);
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(601);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("requested market data clients should map cleanly");

    for client_key in [
        "binance_spot_data",
        "binance_usdm_data",
        "binance_coinm_data",
        "bitmex_data",
        "bybit_data",
        "coinbase_data",
        "deribit_data",
        "okx_data",
        "kraken_spot_data",
        "kraken_futures_data",
    ] {
        let mapped = configs
            .clients
            .get(client_key)
            .unwrap_or_else(|| panic!("{client_key} must be present in mapper output"));
        assert!(
            mapped.data.is_some(),
            "{client_key} must produce an NT data client config"
        );
        assert!(
            mapped.execution.is_none(),
            "{client_key} must stay data-only in this scope"
        );
    }

    let expected_execution_clients: BTreeSet<&str> = loaded
        .root
        .clients
        .iter()
        .filter_map(|(client_key, client)| client.execution.as_ref().map(|_| client_key.as_str()))
        .collect();
    let mapped_execution_clients: BTreeSet<&str> = configs
        .clients
        .iter()
        .filter_map(|(client_key, client)| client.execution.as_ref().map(|_| client_key.as_str()))
        .collect();
    assert_eq!(
        mapped_execution_clients, expected_execution_clients,
        "adapter mapping must preserve exactly the execution clients declared by TOML [execution] blocks"
    );
}

#[test]
fn requested_market_data_clients_reject_nt_ignored_fields_at_mapper_boundary() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    add_requested_market_data_clients(&mut loaded);
    loaded
        .root
        .clients
        .get_mut("bybit_data")
        .expect("bybit_data should be configured")
        .data
        .as_mut()
        .expect("bybit_data should include [data]")
        .as_table_mut()
        .expect("bybit_data [data] should be a table")
        .insert(
            "ws_reconnect_delay_secs".to_string(),
            toml::Value::Integer(5),
        );
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(601);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("unknown NT data fields must not be ignored by the mapper");
    let rendered = error.to_string();
    assert!(
        rendered.contains("bybit_data.data"),
        "expected error to cite the client data block, got: {rendered}"
    );
    assert!(
        rendered.contains("ws_reconnect_delay_secs"),
        "expected error to cite the unknown field, got: {rendered}"
    );
    assert!(
        rendered.contains("unknown NT field"),
        "expected NT-field vocabulary, got: {rendered}"
    );
}

#[test]
fn provider_binding_installs_polymarket_filter_for_updown_target_at_fixed_time() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    // Fixed `now_unix_secs = 601` puts the planner inside the
    // configured window [600, 900): current=600 and next=900. The
    // provider binding's filter must surface those slugs in
    // `[current, next]` order on every `market_slugs()` call.
    let clock = fixed_clock(601);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping with market identity should succeed");

    let polymarket = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present in mapper output");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert_eq!(
        data.filters.len(),
        1,
        "exactly one provider filter should be installed for the single updown target"
    );
    let slugs = data.filters[0]
        .market_slugs()
        .expect("provider filter must yield Some(slugs) when bound to an updown target");
    assert_eq!(
        slugs,
        vec![
            "configured_asset-updown-5m-600".to_string(),
            "configured_asset-updown-5m-900".to_string(),
        ],
        "provider filter slug ordering must be [current, next]"
    );
}

#[test]
fn provider_binding_preserves_declaration_order_across_multiple_updown_targets() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    // Build three strategies whose declaration sequence is deliberately
    // NON-MONOTONIC across every likely accidental sort key
    // (strategy_instance_id, configured_target_id, underlying_asset,
    // cadence_secs, cadence_slug_token). Any accidental `sort_by`
    // inside the binding layer would re-order at least one index and
    // trip a per-index slug assertion below.
    let mut second = loaded.strategies[0].clone();
    let mut third = loaded.strategies[0].clone();
    {
        let first = &mut loaded.strategies[0];
        first.config.strategy_instance_id = "zeta_strategy_main".to_string();
        set_target_field(
            first,
            "configured_target_id",
            toml::Value::String("zeta_target".to_string()),
        );
        set_target_field(
            first,
            "underlying_asset",
            toml::Value::String("ZETA".to_string()),
        );
        set_target_field(first, "cadence_secs", toml::Value::Integer(900));
        set_target_field(
            first,
            "cadence_slug_token",
            toml::Value::String("15m".to_string()),
        );
    }
    second.config.strategy_instance_id = "alpha_strategy_main".to_string();
    set_target_field(
        &mut second,
        "configured_target_id",
        toml::Value::String("alpha_target".to_string()),
    );
    set_target_field(
        &mut second,
        "underlying_asset",
        toml::Value::String("ALPHA".to_string()),
    );
    set_target_field(&mut second, "cadence_secs", toml::Value::Integer(300));
    set_target_field(
        &mut second,
        "cadence_slug_token",
        toml::Value::String("5m".to_string()),
    );

    third.config.strategy_instance_id = "mike_strategy_main".to_string();
    set_target_field(
        &mut third,
        "configured_target_id",
        toml::Value::String("mike_target".to_string()),
    );
    set_target_field(
        &mut third,
        "underlying_asset",
        toml::Value::String("MIKE".to_string()),
    );
    set_target_field(&mut third, "cadence_secs", toml::Value::Integer(3600));
    set_target_field(
        &mut third,
        "cadence_slug_token",
        toml::Value::String("1h".to_string()),
    );

    loaded.strategies.push(second);
    loaded.strategies.push(third);

    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    // Pick now=7300:
    //   cadence 900  -> floor(7300/900)*900 = 7200, next = 8100
    //   cadence 300  -> floor(7300/300)*300 = 7200, next = 7500
    //   cadence 3600 -> floor(7300/3600)*3600 = 7200, next = 10800
    let clock = fixed_clock(7300);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping should succeed");

    let polymarket = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");

    assert_eq!(
        data.filters.len(),
        3,
        "three updown targets must produce three provider filters"
    );

    assert_eq!(
        data.filters[0].market_slugs(),
        Some(vec![
            "zeta-updown-15m-7200".to_string(),
            "zeta-updown-15m-8100".to_string(),
        ]),
        "filters[0] must correspond to declared strategy [0] (zeta)"
    );
    assert_eq!(
        data.filters[1].market_slugs(),
        Some(vec![
            "alpha-updown-5m-7200".to_string(),
            "alpha-updown-5m-7500".to_string(),
        ]),
        "filters[1] must correspond to declared strategy [1] (alpha)"
    );
    assert_eq!(
        data.filters[2].market_slugs(),
        Some(vec![
            "mike-updown-1h-7200".to_string(),
            "mike-updown-1h-10800".to_string(),
        ]),
        "filters[2] must correspond to declared strategy [2] (mike)"
    );
}

#[test]
fn market_identity_path_still_rejects_subscribe_new_markets_true() {
    // The previous mapper boundary refused to forward
    // `subscribe_new_markets = true` to NT (which would otherwise cause
    // pinned NT to subscribe to every Polymarket market). The new
    // market-identity-aware entry point must preserve that invariant
    // so the provider-binding layer cannot be used to smuggle a broad
    // subscription path under the cover of "we have a filter now".
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
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
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
fn empty_market_identity_plan_installs_no_provider_filter() {
    // A configuration with no rotating-market targets must produce no
    // provider filter installation. This pins down the "filter only
    // when configured identity exists" half of the binding contract;
    // accidentally always-installing a filter would otherwise be
    // invisible to the single-target test above.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();

    let empty_plan = MarketIdentityPlan::empty();
    let clock = fixed_clock(0);

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &empty_plan, clock)
        .expect("mapping should succeed");

    let polymarket = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");
    assert!(
        data.filters.is_empty(),
        "an empty market-identity plan must not install any provider filter"
    );
    assert!(
        data.new_market_filter.is_none(),
        "no `new_market_filter` should be smuggled in via the binding layer"
    );
}

#[test]
fn provider_binding_projects_only_referenced_polymarket_outcome_group_sources() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    configure_outcome_group_strategy(
        &mut loaded,
        vec![
            "poly_event_source".to_string(),
            "poly_market_source".to_string(),
        ],
    );
    loaded.root.outcome_group_sources = Some(vec![
        outcome_event_source("poly_event_source", &["world-cup-final"], Some(3)),
        outcome_market_slug_source("poly_market_source", &["home-market", "draw-market"]),
        outcome_market_slug_source("unreferenced_poly_source", &["unreferenced-market"]),
    ]);
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("outcome-group plan should derive cleanly");

    let configs =
        map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, fixed_clock(601))
            .expect("Polymarket should support configured outcome_group targets");
    let data = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present")
        .data
        .as_ref()
        .expect("polymarket data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(
        data.filters.len(),
        2,
        "only the two target.group_sources entries should project into NT filters"
    );
    let event_queries = data.filters[0]
        .event_queries()
        .expect("bounded event source should use NT EventQueryFilter");
    assert_eq!(event_queries.len(), 1);
    assert_eq!(event_queries[0].0, "world-cup-final");
    assert_eq!(
        event_queries[0].1.max_markets,
        Some(3),
        "configured cap must reach NT Gamma market query params"
    );
    assert_eq!(
        data.filters[1].market_slugs(),
        Some(vec!["home-market".to_string(), "draw-market".to_string()]),
        "market-slug outcome sources should map to NT MarketSlugFilter in target order"
    );
    assert!(
        data.new_market_filter.is_none(),
        "outcome-group discovery must not enable broad new-market subscriptions"
    );
}

#[test]
fn provider_binding_deduplicates_repeated_polymarket_outcome_group_sources() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    configure_outcome_group_strategy(
        &mut loaded,
        vec![
            "poly_market_source".to_string(),
            "poly_market_source".to_string(),
        ],
    );
    loaded.root.outcome_group_sources = Some(vec![outcome_market_slug_source(
        "poly_market_source",
        &["home-market", "draw-market"],
    )]);
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("outcome-group plan should derive cleanly");

    let configs =
        map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, fixed_clock(601))
            .expect("Polymarket should support duplicate target source references");
    let data = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present")
        .data
        .as_ref()
        .expect("polymarket data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(
        data.filters.len(),
        1,
        "duplicate group_sources entries must not produce duplicate NT filters"
    );
    assert_eq!(
        data.filters[0].market_slugs(),
        Some(vec!["home-market".to_string(), "draw-market".to_string()])
    );
}

#[test]
fn provider_binding_composes_updown_outcome_group_and_static_filters_for_same_client() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    loaded.root.outcome_group_sources = Some(vec![outcome_market_slug_source(
        "poly_market_source",
        &["home-market", "draw-market"],
    )]);
    let mut plan =
        plan_market_identity(&loaded).expect("fixture updown plan should derive cleanly");
    plan.push_target(OutcomeGroupTargetPlan {
        strategy_instance_id: "complete-set-sample".to_string(),
        configured_target_id: "complete-set-target".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        group_sources: vec!["poly_market_source".to_string()],
    });
    plan.push_target(StaticBinaryEventTargetPlan {
        strategy_instance_id: "static-sample".to_string(),
        configured_target_id: "static-target".to_string(),
        execution_client_id: "polymarket_main".to_string(),
        event_key: "sample_event_2026".to_string(),
        market_slug: "will-sample-static-resolve-yes".to_string(),
        condition_id: Some("condition-sample-static".to_string()),
        yes_outcome: "Yes".to_string(),
        no_outcome: "No".to_string(),
    });
    let resolved = fixture_resolved_secrets();

    let configs =
        map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, fixed_clock(601))
            .expect("Polymarket should compose all fetch-only filter families");
    let data = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present")
        .data
        .as_ref()
        .expect("polymarket data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(
        data.filters.len(),
        3,
        "same-client updown, outcome-group, and static targets should each install one fetch filter"
    );
    assert_eq!(
        data.filters[0].market_slugs(),
        Some(vec![
            "configured_asset-updown-5m-600".to_string(),
            "configured_asset-updown-5m-900".to_string(),
        ]),
        "updown filters must stay first"
    );
    assert_eq!(
        data.filters[1].market_slugs(),
        Some(vec!["home-market".to_string(), "draw-market".to_string()]),
        "outcome-group filters must stay after updown filters"
    );
    assert_eq!(
        data.filters[2].market_slugs(),
        Some(vec!["will-sample-static-resolve-yes".to_string()]),
        "static binary-event filters must stay after outcome-group filters"
    );
    assert!(
        data.new_market_filter.is_none(),
        "composed target filters must not enable broad new-market subscriptions"
    );
}

#[test]
fn provider_binding_projects_bounded_polymarket_gamma_query_source() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    configure_outcome_group_strategy(&mut loaded, vec!["poly_query_source".to_string()]);
    loaded.root.outcome_group_sources = Some(vec![outcome_gamma_query_source("poly_query_source")]);
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("outcome-group plan should derive cleanly");

    let configs =
        map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, fixed_clock(601))
            .expect("Polymarket should support configured outcome_group targets");
    let data = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present")
        .data
        .as_ref()
        .expect("polymarket data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(data.filters.len(), 1);
    let params = data.filters[0]
        .query_params()
        .expect("Gamma query source should use NT GammaQueryFilter");
    assert_eq!(params.tag_id, Some("sports-tag".to_string()));
    assert_eq!(params.sports_market_types, Some("moneyline".to_string()));
    assert_eq!(params.max_markets, Some(3));
    assert!(
        data.new_market_filter.is_none(),
        "Gamma query sources must not use NT new_market_filter"
    );
}

#[test]
fn provider_binding_accepts_polymarket_gamma_event_query_without_tag() {
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    configure_outcome_group_strategy(&mut loaded, vec!["poly_query_source".to_string()]);
    let mut source = outcome_gamma_query_source("poly_query_source");
    let query = source
        .gamma_query
        .as_mut()
        .expect("gamma query source fixture");
    query.event_query = Some("world cup".to_string());
    query.tag_id = None;
    loaded.root.outcome_group_sources = Some(vec![source]);
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("outcome-group plan should derive cleanly");

    let configs =
        map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, fixed_clock(601))
            .expect("Polymarket should support event-query bounded outcome_group targets");
    let data = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present")
        .data
        .as_ref()
        .expect("polymarket data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(data.filters.len(), 1);
    let event_queries = data.filters[0]
        .event_queries()
        .expect("event_query source should use NT EventQueryFilter");
    assert_eq!(event_queries.len(), 1);
    assert_eq!(event_queries[0].0, "world cup");
    assert_eq!(event_queries[0].1.tag_id, None);
    assert_eq!(
        event_queries[0].1.sports_market_types,
        Some("moneyline".to_string())
    );
    assert_eq!(event_queries[0].1.max_markets, Some(3));
    assert!(
        data.new_market_filter.is_none(),
        "Gamma event-query sources must not use NT new_market_filter"
    );
}

#[test]
fn provider_binding_filter_recomputes_slug_pair_each_call_against_advancing_clock() {
    // The pinned NT contract for `MarketSlugFilter::new` re-evaluates
    // the closure on every `load_all` cycle so the slug pair rolls
    // forward with cadence. This test pins that dynamic re-evaluation
    // by injecting an `AtomicI64`-backed clock, advancing it by one
    // full cadence between two `market_slugs()` calls, and asserting
    // the filter surfaces the rolled-forward `[current, next]` pair.
    // A future regression that wraps the closure result in a `OnceCell`
    // (or otherwise memoises the slug list) would fail the second
    // assertion below.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");
    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");

    let counter = Arc::new(AtomicI64::new(601));
    let clock_handle = counter.clone();
    let clock: BoltV3MarketClockFn = Arc::new(move || clock_handle.load(Ordering::Relaxed));

    let configs = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect("mapping should succeed");

    let polymarket = configs
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be present");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket [data] block must produce an NT data config")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast to NT PolymarketDataClientConfig");
    let filter = &data.filters[0];

    // First cycle at counter=601: configured window [600, 900).
    assert_eq!(
        filter.market_slugs(),
        Some(vec![
            "configured_asset-updown-5m-600".to_string(),
            "configured_asset-updown-5m-900".to_string(),
        ]),
        "first market_slugs() call must reflect counter=601"
    );

    // Advance the clock by one full cadence; the filter MUST recompute.
    counter.store(901, Ordering::Relaxed);

    assert_eq!(
        filter.market_slugs(),
        Some(vec![
            "configured_asset-updown-5m-900".to_string(),
            "configured_asset-updown-5m-1200".to_string(),
        ]),
        "second market_slugs() call must reflect counter=901; \
         caching the slug list would fail this assertion"
    );
}

#[test]
fn provider_binding_rejects_updown_target_bound_to_non_polymarket_client() {
    // The binding layer must fail loud if a configured rotating-market
    // target points at a non-Polymarket client. Without this guard the
    // target would be silently dropped, because filter installation
    // only runs on the Polymarket branch of the client iteration.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    loaded.root.clients.insert(
        "unsupported_execution_client".to_string(),
        ClientBlock {
            venue: Venue::from("BINANCE"),
            data: None,
            execution: None,
            secrets: None,
            readiness_probe: None,
        },
    );
    loaded.strategies[0].config.execution_client_id =
        nautilus_model::identifiers::ClientId::from("unsupported_execution_client");

    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("non-polymarket client binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "unsupported_execution_client");
            assert_eq!(field, "strategy.execution_client_id");
            assert!(
                message.contains("does not support that market family"),
                "error message should explain the family/provider compatibility boundary: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

#[test]
fn provider_binding_rejects_updown_target_bound_to_unknown_client() {
    // A target whose `execution_client_id` does not appear under
    // `[clients]` is also a misconfiguration the binding layer must
    // reject explicitly rather than silently produce no filter.
    let root_path = support::repo_path("tests/fixtures/bolt_v3/root.toml");
    let mut loaded = load_bolt_v3_config(&root_path).expect("fixture v3 config should load");

    loaded.strategies[0].config.execution_client_id =
        nautilus_model::identifiers::ClientId::from("client_does_not_exist");

    let resolved = fixture_resolved_secrets();
    let plan = plan_market_identity(&loaded).expect("plan should derive cleanly");
    let clock = fixed_clock(0);

    let error = map_bolt_v3_adapters_with_market_identity(&loaded, &resolved, &plan, clock)
        .expect_err("unknown client binding must fail loud at the adapter boundary");
    match error {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key,
            field,
            message,
        } => {
            assert_eq!(client_key, "client_does_not_exist");
            assert_eq!(field, "strategy.execution_client_id");
            assert!(
                message.contains("unknown client"),
                "error message should describe the unknown-client case: {message}"
            );
        }
        other => panic!("expected ValidationInvariant, got {other}"),
    }
}

fn configure_outcome_group_strategy(
    loaded: &mut bolt_v2::bolt_v3_config::LoadedBoltV3Config,
    group_sources: Vec<String>,
) {
    let strategy = &mut loaded.strategies[0];
    strategy.config.strategy_archetype = toml::Value::String("complete_set_arbitrage".to_string())
        .try_into()
        .expect("complete-set archetype key should parse");
    strategy.config.execution_client_id = ClientId::from("polymarket_main");
    strategy.config.target = toml::toml! {
        configured_target_id = "complete_set_target"
        kind = "static_outcome_group"
        rotating_market_family = "outcome_group"
        group_sources = group_sources
    }
    .into();
}

fn outcome_event_source(
    source_id: &str,
    event_slugs: &[&str],
    max_markets: Option<usize>,
) -> OutcomeGroupSourceConfig {
    let mut source = outcome_source_base(source_id, SourceConfigKind::GammaEvent);
    source.event_slugs = Some(event_slugs.iter().map(|value| value.to_string()).collect());
    source.max_markets = max_markets;
    source
}

fn outcome_market_slug_source(source_id: &str, market_slugs: &[&str]) -> OutcomeGroupSourceConfig {
    let mut source = outcome_source_base(source_id, SourceConfigKind::GammaMarketSlug);
    source.market_slugs = Some(market_slugs.iter().map(|value| value.to_string()).collect());
    source
}

fn outcome_gamma_query_source(source_id: &str) -> OutcomeGroupSourceConfig {
    let mut source = outcome_source_base(source_id, SourceConfigKind::GammaQuery);
    source.gamma_query = Some(GammaQueryBlock {
        search: None,
        event_query: None,
        market_query: None,
        tag_id: Some("sports-tag".to_string()),
        sports_market_types: Some(vec!["moneyline".to_string()]),
        max_events: None,
        max_markets: 3,
    });
    source
}

fn outcome_source_base(source_id: &str, kind: SourceConfigKind) -> OutcomeGroupSourceConfig {
    OutcomeGroupSourceConfig {
        source_id: source_id.to_string(),
        client_id: ClientId::from("polymarket_main"),
        kind,
        event_slugs: None,
        market_slugs: None,
        sports_market_types: None,
        gamma_query: None,
        question: None,
        expected_neg_risk_market_id: Some("neg-risk-123".to_string()),
        terminal_state_labels: Some(vec!["home".to_string(), "draw".to_string()]),
        max_markets: None,
        max_groups: None,
        enabled: true,
        freshness: None,
        order_constraints: None,
        role_bindings: None,
        settlement_rules: None,
    }
}
