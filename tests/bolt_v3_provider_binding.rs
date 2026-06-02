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

mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use bolt_v2::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3MarketClockFn, map_bolt_v3_adapters_with_market_identity,
    },
    bolt_v3_config::{ClientBlock, LoadedStrategy, load_bolt_v3_config},
    bolt_v3_market_families::{MarketIdentityPlan, updown::plan_market_identity},
    bolt_v3_providers::{
        ProviderLiveSubmitApprovalContext, binance::ResolvedBoltV3BinanceSecrets,
        binding_for_provider_key, hyperliquid::ResolvedBoltV3HyperliquidSecrets,
        hyperliquid_artifacts::read_hyperliquid_live_submit_approval_artifact,
        polymarket::ResolvedBoltV3PolymarketSecrets, validate_client_block,
    },
    bolt_v3_secrets::{
        ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars_with, resolve_bolt_v3_secrets_with,
    },
};
use nautilus_model::identifiers::{InstrumentId, Venue};
use nautilus_polymarket::config::PolymarketDataClientConfig;

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
        Arc::new(ResolvedBoltV3PolymarketSecrets {
            private_key: zeroize::Zeroizing::new("binding-poly-private-key".to_string()),
            api_key: zeroize::Zeroizing::new("binding-poly-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("binding-poly-api-secret".to_string()),
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
    ResolvedBoltV3Secrets { clients }
}

fn fixed_clock(now_unix_secs: i64) -> BoltV3MarketClockFn {
    Arc::new(move || now_unix_secs)
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

fn add_hyperliquid_live_submit_approval(client: &mut ClientBlock) {
    let execution = client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table");
    execution.insert(
        "live_submit_approval_id".to_string(),
        toml::Value::String("hl-standard-perps-approval-001".to_string()),
    );
    execution.insert(
        "live_submit_approval_artifact_path".to_string(),
        toml::Value::String("operator/hyperliquid-live-submit-approval.json".to_string()),
    );
    execution.insert(
        "live_submit_approval_artifact_max_bytes".to_string(),
        toml::Value::Integer(65536),
    );
    execution.insert(
        "live_submit_max_order_count".to_string(),
        toml::Value::Integer(1),
    );
    execution.insert(
        "live_submit_max_order_notional".to_string(),
        toml::Value::String("10.00".to_string()),
    );
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
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

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

fn hyperliquid_hip4_execution_client_without_settlement_poll() -> ClientBlock {
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
product_types = ["spot"]
environment = "mainnet"
base_url_http = "https://api.binance.com"
base_url_ws = "wss://stream-sbe.binance.com/ws"
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
product_types = ["usd_m"]
environment = "testnet"
base_url_http = "https://demo-fapi.binance.com"
base_url_ws = "wss://fstream.binancefuture.com/ws"
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
product_types = ["coin_m"]
environment = "testnet"
base_url_http = "https://testnet.binancefuture.com"
base_url_ws = "wss://dstream.binancefuture.com/ws"
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
fn provider_binding_models_hyperliquid_egress_with_official_user_fees_weight() {
    let model = bolt_v2::bolt_v3_providers::venue_egress_model("HYPERLIQUID")
        .expect("Hyperliquid execution must have a REST egress model before live submit");

    assert_eq!(model.cap_per_minute, 1200);
    assert_eq!(model.max_rest_requests_per_order_command, 20);
}

#[test]
fn provider_binding_builds_hyperliquid_fee_provider_that_fails_closed_without_fee_proof() {
    let client = hyperliquid_execution_client(
        "/bolt/hyperliquid/master_api_wallet/private_key",
        "/bolt/hyperliquid/master_api_wallet/account_address",
    );
    let binding = binding_for_provider_key("HYPERLIQUID")
        .expect("Hyperliquid provider binding should be registered");
    let build_fee_provider = binding
        .build_fee_provider
        .expect("Hyperliquid execution must resolve fees through the provider boundary");
    let provider = build_fee_provider("hyperliquid_perps", &client, &fixture_resolved_secrets())
        .expect("Hyperliquid fee provider should construct from provider-owned config");

    assert_eq!(
        provider.fee_bps(InstrumentId::from("BTC-PERP.HYPERLIQUID")),
        None,
        "Hyperliquid fee provider must fail closed until product fee proof is available"
    );
}

#[test]
fn provider_binding_rejects_hip4_execution_without_settlement_poll() {
    let client = hyperliquid_hip4_execution_client_without_settlement_poll();
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
    client
        .execution
        .as_mut()
        .expect("test Hyperliquid client should have execution")
        .as_table_mut()
        .expect("test Hyperliquid execution should be a table")
        .insert(
            "live_submit_approval_artifact_path".to_string(),
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

    let artifact_text = fs::read_to_string(&approval_path).expect("artifact should read");
    assert!(!artifact_text.contains(private_key));
    assert!(!artifact_text.contains(account_address));
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
            "configured_asset-updown-configuredwindow-600".to_string(),
            "configured_asset-updown-configuredwindow-900".to_string(),
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
            toml::Value::String("quarterhour".to_string()),
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
        toml::Value::String("shortwindow".to_string()),
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
        toml::Value::String("hourwindow".to_string()),
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
            "zeta-updown-quarterhour-7200".to_string(),
            "zeta-updown-quarterhour-8100".to_string(),
        ]),
        "filters[0] must correspond to declared strategy [0] (zeta)"
    );
    assert_eq!(
        data.filters[1].market_slugs(),
        Some(vec![
            "alpha-updown-shortwindow-7200".to_string(),
            "alpha-updown-shortwindow-7500".to_string(),
        ]),
        "filters[1] must correspond to declared strategy [1] (alpha)"
    );
    assert_eq!(
        data.filters[2].market_slugs(),
        Some(vec![
            "mike-updown-hourwindow-7200".to_string(),
            "mike-updown-hourwindow-10800".to_string(),
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
            "configured_asset-updown-configuredwindow-600".to_string(),
            "configured_asset-updown-configuredwindow-900".to_string(),
        ]),
        "first market_slugs() call must reflect counter=601"
    );

    // Advance the clock by one full cadence; the filter MUST recompute.
    counter.store(901, Ordering::Relaxed);

    assert_eq!(
        filter.market_slugs(),
        Some(vec![
            "configured_asset-updown-configuredwindow-900".to_string(),
            "configured_asset-updown-configuredwindow-1200".to_string(),
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
