#![cfg(test)]

use super::*;

#[test]
fn data_client_probe_config_keeps_only_selected_data_client() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let mut secondary = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    secondary.execution = None;
    secondary.secrets = None;
    loaded
        .root
        .clients
        .insert("secondary_data".to_string(), secondary);

    let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
        .expect("selected data client should produce a scoped probe config");

    assert!(
        probe_loaded.strategies.is_empty(),
        "adapter mapping must drop strategy targets that do not reference the selected probe client"
    );
    assert_eq!(probe_loaded.root_path, loaded.root_path);
    assert_eq!(
        probe_loaded.config_bundle_checksum,
        loaded.config_bundle_checksum
    );
    assert_eq!(probe_loaded.root.clients.len(), 1);
    assert!(probe_loaded.root.clients.contains_key("secondary_data"));
    assert!(
        loaded.root.clients.contains_key("polymarket_main"),
        "helper must not mutate the caller's full client bundle"
    );
}

#[test]
fn data_client_census_report_sorts_and_dedupes_instrument_ids() {
    let unsorted = data_client_census_report(
        "bybit_data",
        vec![
            "ETH/USDT.BYBIT".to_string(),
            "BTC/USDT.BYBIT".to_string(),
            "ETH/USDT.BYBIT".to_string(),
        ],
    )
    .expect("non-empty census should build");
    let sorted = data_client_census_report(
        "bybit_data",
        vec!["BTC/USDT.BYBIT".to_string(), "ETH/USDT.BYBIT".to_string()],
    )
    .expect("deduped census should build");

    assert_eq!(unsorted.cached_instrument_count, 2);
    assert_eq!(
        unsorted.cached_instrument_ids_sha256,
        sorted.cached_instrument_ids_sha256
    );
}

#[test]
fn data_client_census_report_rejects_empty_cache() {
    let error = data_client_census_report("bybit_data", Vec::new())
        .expect_err("empty instrument cache must fail closed");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason }
            if reason.contains("zero cached instruments")
    ));
}

#[test]
fn data_client_probe_adapter_mapping_drops_unrelated_strategy_targets() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let mut secondary = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    secondary.execution = None;
    secondary.secrets = None;
    loaded
        .root
        .clients
        .insert("secondary_data".to_string(), secondary);

    let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
        .expect("selected data client should produce a scoped probe config");

    assert!(
        probe_loaded.strategies.is_empty(),
        "probe mapping input must drop strategy targets that reference clients outside the scoped probe"
    );
    strategy_free_transport_adapter_configs(
        &probe_loaded,
        &crate::bolt_v3_secrets::ResolvedBoltV3Secrets {
            clients: Default::default(),
        },
    )
    .expect("scoped data-client adapter mapping must not fail on unrelated strategies");
}

#[test]
fn data_client_probe_runtime_clears_strategies_after_adapter_mapping() {
    let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");

    let probe_loaded = data_client_probe_loaded_config(&loaded, "polymarket_main")
        .expect("selected data client should produce a scoped probe config");
    let runtime_loaded = strategy_free_transport_loaded_config(&probe_loaded);

    assert!(
        !probe_loaded.strategies.is_empty(),
        "probe adapter mapping input must keep strategies for provider-owned data filters"
    );
    assert!(
        runtime_loaded.strategies.is_empty(),
        "strategy-free data-client probes must not register strategy actors"
    );
    assert_eq!(runtime_loaded.root.clients.len(), 1);
    assert!(runtime_loaded.root.clients.contains_key("polymarket_main"));
    assert!(
        !runtime_loaded.root.clients.contains_key("okx_data"),
        "strategy-free data-client probes must not pull in configured RV sources"
    );
}

#[test]
fn strategy_free_adapter_mapping_preserves_strategy_derived_market_filters() {
    use crate::{
        bolt_v3_providers::{
            binance::ResolvedBoltV3BinanceSecrets, chainlink::ResolvedBoltV3ChainlinkSecrets,
            chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets,
            polymarket::ResolvedBoltV3PolymarketSecrets,
            polyresearch::ResolvedBoltV3PolyResearchSecrets,
        },
        bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
    };
    use nautilus_polymarket::config::PolymarketDataClientConfig;
    use std::{collections::BTreeMap, sync::Arc};

    let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
    clients.insert(
        "polymarket_main".to_string(),
        Arc::new(ResolvedBoltV3PolymarketSecrets {
            private_key: zeroize::Zeroizing::new("fixture-poly-private-key".to_string()),
            api_key: zeroize::Zeroizing::new("fixture-poly-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-poly-api-secret".to_string()),
            passphrase: zeroize::Zeroizing::new("fixture-poly-passphrase".to_string()),
        }),
    );
    clients.insert(
        "binance_reference".to_string(),
        Arc::new(ResolvedBoltV3BinanceSecrets {
            api_key: zeroize::Zeroizing::new("fixture-binance-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-binance-api-secret".to_string()),
        }),
    );
    clients.insert(
        "chainlink_strike".to_string(),
        Arc::new(ResolvedBoltV3ChainlinkSecrets {
            api_key: zeroize::Zeroizing::new("fixture-chainlink-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-chainlink-api-secret".to_string()),
        }),
    );
    clients.insert(
        "chainlink_reference".to_string(),
        Arc::new(ResolvedBoltV3ChainlinkReferenceSecrets {
            api_key: zeroize::Zeroizing::new("fixture-chainlink-reference-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new(
                "fixture-chainlink-reference-api-secret".to_string(),
            ),
        }),
    );
    clients.insert(
        "polyresearch_reference".to_string(),
        Arc::new(ResolvedBoltV3PolyResearchSecrets {
            api_key: zeroize::Zeroizing::new("fixture-polyresearch-api-key".to_string()),
        }),
    );
    let resolved = ResolvedBoltV3Secrets { clients };

    let adapters = strategy_free_transport_adapter_configs(&loaded, &resolved)
        .expect("strategy-free adapter mapping should retain market identity filters");
    let polymarket = adapters
        .clients
        .get("polymarket_main")
        .expect("polymarket_main must be mapped");
    let data = polymarket
        .data
        .as_ref()
        .expect("polymarket data config must be mapped")
        .config_as::<PolymarketDataClientConfig>()
        .expect("polymarket data config should downcast");

    assert_eq!(
        data.filters.len(),
        1,
        "strategy-free adapter mapping must keep strategy-derived provider filters"
    );
    assert_eq!(
        data.filters[0]
            .market_slugs()
            .expect("strategy-free data config must keep configured target slug filters")
            .len(),
        2
    );
}
