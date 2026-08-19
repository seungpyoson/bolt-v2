#![cfg(test)]

use super::*;

#[test]
fn strategy_free_transport_config_preserves_identity_but_removes_strategy_instances() {
    let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    assert!(
        !loaded.strategies.is_empty(),
        "fixture must include strategy config to prove strategy-free transport strips it"
    );

    let strategy_free_loaded = strategy_free_transport_loaded_config(&loaded);

    assert!(
        strategy_free_loaded.strategies.is_empty(),
        "strategy-free transport runtime must not register strategy actors"
    );
    assert_eq!(strategy_free_loaded.root_path, loaded.root_path);
    assert_eq!(
        strategy_free_loaded.config_bundle_checksum,
        loaded.config_bundle_checksum
    );
    assert_eq!(
        strategy_free_loaded.root.strategy_files,
        loaded.root.strategy_files
    );
    assert!(
        !loaded.strategies.is_empty(),
        "helper must not mutate the caller's loaded config"
    );
}

#[test]
fn trade_transport_config_keeps_iv_only_source_clients() {
    let loaded = fixture_loaded_config_with_external_option_greeks_iv();

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("IV source client must stay in scope");

    assert_eq!(scoped.root.clients.len(), 1);
    assert!(scoped.root.clients.contains_key("configured-client"));
    assert!(loaded.root.clients.contains_key("configured-client"));
}

#[test]
fn trade_transport_config_prunes_unreferenced_hyperliquid_execution_clients_from_root() {
    let loaded =
        crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new("config/root.toml"))
            .expect("production root config should load");
    assert!(
        loaded.root.clients.contains_key("hyperliquid_execution"),
        "hyperliquid_execution should be configured in root.toml before transport pruning"
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("production trade transport scope should derive cleanly");

    assert!(
        !scoped.root.clients.contains_key("hyperliquid_execution"),
        "hyperliquid_execution is not strategy-referenced and must not reach live-node registration"
    );
}

#[test]
fn trade_transport_config_keeps_capital_admission_execution_client_without_strategy() {
    let mut loaded = fixture_loaded_config();
    loaded.strategies.clear();
    loaded
        .root
        .risk
        .capital_pools
        .as_mut()
        .expect("fixture should configure capital pools")[0]
        .enforce_submit_admission = true;

    let scoped = trade_transport_loaded_config(
        &loaded,
        RealizedVolatilityTransportScope::Subscribed,
    )
    .expect("capital admission provider collateral allowance requires the venue execution client");

    assert!(scoped.root.clients.contains_key("polymarket_main"));
}

#[test]
fn trade_transport_config_rejects_multiple_referenced_execution_clients_for_same_venue() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    assert!(
        !loaded.strategies.is_empty(),
        "fixture config should include a strategy for venue-cardinality coverage"
    );
    let cloned_strategy = loaded.strategies[0].clone();
    loaded.strategies.truncate(1);
    loaded.strategies.push(cloned_strategy);
    loaded.strategies[0].config.execution_client_id = ClientId::from("hyperliquid_a");
    loaded.strategies[1].config.execution_client_id = ClientId::from("hyperliquid_b");
    loaded.root.clients.insert(
        "hyperliquid_a".to_string(),
        test_execution_client("HYPERLIQUID"),
    );
    loaded.root.clients.insert(
        "hyperliquid_b".to_string(),
        test_execution_client("HYPERLIQUID"),
    );

    let error =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect_err("multiple active execution clients for one venue must fail closed");
    let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
        panic!("expected LiveTransportScope error for duplicate execution venue");
    };
    assert!(
        reason.contains("multiple execution clients share venue `HYPERLIQUID`")
            && reason.contains("hyperliquid_a")
            && reason.contains("hyperliquid_b"),
        "duplicate execution venue error should identify venue and clients: {reason}"
    );
}

#[test]
fn all_configured_mapping_rejects_multiple_execution_clients_for_same_venue_before_resolution() {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.clear();
    loaded.root.clients.insert(
        "hyperliquid_a".to_string(),
        test_execution_client("HYPERLIQUID"),
    );
    loaded.root.clients.insert(
        "hyperliquid_b".to_string(),
        test_execution_client("HYPERLIQUID"),
    );

    let error = match build_bolt_v3_all_configured_client_mapping_live_node_with_summary(
        &loaded,
        |_| false,
        |_, _| -> Result<String, String> {
            panic!("duplicate execution venue should fail before secret resolution")
        },
    ) {
        Ok(_) => panic!("all-configured mapping should reject duplicate execution venues"),
        Err(error) => error,
    };

    let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
        panic!("expected LiveTransportScope error for duplicate execution venue");
    };
    assert!(
        reason.contains("multiple execution clients share venue `HYPERLIQUID`")
            && reason.contains("hyperliquid_a")
            && reason.contains("hyperliquid_b"),
        "duplicate execution venue error should identify venue and clients: {reason}"
    );
}

#[test]
fn trade_transport_config_keeps_strategy_and_root_substrate_clients() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let mut signal_client = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    signal_client.execution = None;
    signal_client.secrets = None;
    let unrelated_client = signal_client.clone();
    loaded
        .root
        .clients
        .insert("signal_data".to_string(), signal_client);
    loaded
        .root
        .clients
        .insert("unrelated_data".to_string(), unrelated_client);
    {
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.signal_data.insert(
            "primary".to_string(),
            DataInstrumentBlock {
                data_client_id: ClientId::from("signal_data"),
                instrument_id: InstrumentId::from("SIGNAL.SOURCE"),
            },
        );
    }
    let mut rv_client = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    rv_client.execution = None;
    rv_client.secrets = None;
    loaded.root.clients.insert("rv_data".to_string(), rv_client);
    loaded
        .root
        .realized_volatility_surfaces
        .as_mut()
        .expect("fixture should include realized-volatility surfaces")
        .get_mut("configured_rv_surface")
        .expect("fixture should include configured RV surface")
        .sources
        .first_mut()
        .expect("fixture RV surface should include one source")
        .data_client_id = ClientId::from("rv_data");
    let mut gate_client = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    gate_client.execution = None;
    gate_client.secrets = None;
    loaded
        .root
        .clients
        .insert("gate_data".to_string(), gate_client);
    loaded
        .root
        .gate_providers
        .as_mut()
        .expect("fixture should include gate providers")
        .get_mut("resolution_oracle_primary")
        .expect("fixture should include target-referenced gate provider")
        .client_id = Some(ClientId::from("gate_data"));
    let mut outcome_group_client = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture client should exist")
        .clone();
    outcome_group_client.execution = None;
    outcome_group_client.secrets = None;
    loaded
        .root
        .clients
        .insert("outcome_group_data".to_string(), outcome_group_client);
    loaded.root.outcome_group_sources = Some(vec![
        crate::bolt_v3_outcome_group_sources::OutcomeGroupSourceConfig {
            source_id: "configured_group_source".to_string(),
            client_id: ClientId::from("outcome_group_data"),
            kind: crate::bolt_v3_outcome_group_sources::OutcomeGroupSourceKind::GammaQuery,
            event_slugs: None,
            market_slugs: None,
            sports_market_types: None,
            gamma_query: Some(crate::bolt_v3_outcome_group_sources::GammaQueryBlock {
                search: None,
                event_query: None,
                market_query: Some("configured outcome group".to_string()),
                tag_id: None,
                sports_market_types: None,
                max_events: None,
                max_markets: 1,
            }),
            question: None,
            expected_neg_risk_market_id: None,
            terminal_state_labels: None,
            max_markets: None,
            max_groups: None,
            enabled: true,
            freshness: None,
            order_constraints: None,
            role_bindings: None,
            settlement_rules: None,
        },
    ]);
    let mut outcome_group_strategy = loaded
        .strategies
        .first()
        .expect("fixture should include one strategy")
        .clone();
    outcome_group_strategy.config.strategy_instance_id = "configured_outcome_group".to_string();
    outcome_group_strategy.config.realized_volatility_surface_id = None;
    outcome_group_strategy.config.signal_data.clear();
    outcome_group_strategy.config.reference_current_price = None;
    outcome_group_strategy.config.resolution_data = None;
    outcome_group_strategy.config.target = toml::toml! {
        configured_target_id = "configured_outcome_group_target"
        kind = "static_outcome_group"
        rotating_market_family = "outcome_group"
        group_sources = ["configured_group_source"]
    }
    .into();
    loaded.strategies.push(outcome_group_strategy);

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("strategy-bound transport scope should be derived from config");

    assert_eq!(scoped.root.clients.len(), 7);
    assert!(scoped.root.clients.contains_key("polymarket_main"));
    assert!(scoped.root.clients.contains_key("signal_data"));
    assert!(scoped.root.clients.contains_key("rv_data"));
    assert!(scoped.root.clients.contains_key("gate_data"));
    assert!(scoped.root.clients.contains_key("outcome_group_data"));
    assert!(scoped.root.clients.contains_key("chainlink_reference"));
    assert!(scoped.root.clients.contains_key("polyresearch_reference"));
    assert!(
        !scoped.root.clients.contains_key("unrelated_data"),
        "unrelated configured data clients must not block the selected trade path"
    );
    assert_eq!(scoped.strategies.len(), loaded.strategies.len());
    assert!(
        loaded.root.clients.contains_key("unrelated_data"),
        "helper must not mutate the caller's full client bundle"
    );
}

#[test]
fn trade_transport_config_fails_closed_on_malformed_gate_subscription_target() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let strategy = loaded
        .strategies
        .first_mut()
        .expect("fixture should include one strategy");
    let resolution = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("resolution"))
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include resolution gate subscriptions");
    resolution.insert(
        "required".to_string(),
        toml::Value::String("not-a-bool".to_string()),
    );

    let error =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect_err("malformed gate subscription target must fail closed");
    let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
        panic!("expected LiveTransportScope error for malformed target");
    };
    assert!(
        reason.contains("gate_subscriptions") && reason.contains("not-a-bool"),
        "malformed target error should identify the gate subscription field: {reason}"
    );
}

#[test]
fn trade_transport_config_fails_closed_on_unknown_gate_provider_reference() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    let strategy = loaded
        .strategies
        .first_mut()
        .expect("fixture should include one strategy");
    let mapping = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|subscriptions| subscriptions.get_mut("resolution"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|resolution| resolution.get_mut("market_mappings"))
        .and_then(toml::Value::as_array_mut)
        .and_then(|mappings| mappings.first_mut())
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include a resolution gate mapping");
    mapping.insert(
        "provider_id".to_string(),
        toml::Value::String("missing_gate_provider".to_string()),
    );

    let error =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect_err("unknown gate provider reference must fail closed");
    let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
        panic!("expected LiveTransportScope error for missing gate provider");
    };
    assert!(
        reason.contains("missing_gate_provider")
            && reason.contains("[gate_providers.missing_gate_provider]"),
        "missing provider error should identify the unresolved provider: {reason}"
    );
}

#[test]
fn trade_transport_config_allows_no_provider_gate_subscription_without_gate_providers() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded.root.gate_providers = None;
    let strategy = loaded
        .strategies
        .first_mut()
        .expect("fixture should include one strategy");
    let subscriptions = strategy
        .config
        .target
        .as_table_mut()
        .and_then(|target| target.get_mut("gate_subscriptions"))
        .and_then(toml::Value::as_table_mut)
        .expect("fixture strategy should include gate subscriptions");
    subscriptions.clear();
    subscriptions.insert(
        "resolution".to_string(),
        toml::toml! {
            required = false
            allow_no_resolution = true
        }
        .into(),
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("provider-free gate subscription should not require [gate_providers]");
    assert!(
        !scoped.root.clients.contains_key("gate_data"),
        "provider-free gate subscription must not retain a gate provider data client"
    );
}

#[test]
fn trade_transport_subscribed_retains_enabled_rv_sources_from_unreferenced_surfaces() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("rv_only_data".to_string(), test_okx_data_client());
    insert_test_rv_surface(
        &mut loaded,
        "orphan_rv_surface",
        vec![test_rv_source(
            "orphan_midpoint",
            "rv_only_data",
            "CONFIGURED_ASSET-USDT-PERP.OKX",
            true,
        )],
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("RV-only source client must stay in trade transport scope");

    assert!(
        scoped.root.clients.contains_key("rv_only_data"),
        "trade transport must retain enabled RV sources even when no strategy names the surface"
    );
}

#[test]
fn trade_transport_subscribed_retains_union_of_enabled_rv_sources_across_surfaces() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("rv_union_a".to_string(), test_okx_data_client());
    loaded
        .root
        .clients
        .insert("rv_union_b".to_string(), test_okx_data_client());
    insert_test_rv_surface(
        &mut loaded,
        "union_rv_surface_a",
        vec![test_rv_source(
            "union_midpoint_a",
            "rv_union_a",
            "CONFIGURED_ASSET-USDT-UNION-A.OKX",
            true,
        )],
    );
    insert_test_rv_surface(
        &mut loaded,
        "union_rv_surface_b",
        vec![test_rv_source(
            "union_midpoint_b",
            "rv_union_b",
            "CONFIGURED_ASSET-USDT-UNION-B.OKX",
            true,
        )],
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("trade transport should retain every enabled RV source client");

    assert!(scoped.root.clients.contains_key("rv_union_a"));
    assert!(scoped.root.clients.contains_key("rv_union_b"));
}

#[test]
fn trade_transport_subscribed_dedupes_duplicate_rv_source_clients() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("shared_rv_data".to_string(), test_okx_data_client());
    insert_test_rv_surface(
        &mut loaded,
        "shared_rv_surface_a",
        vec![test_rv_source(
            "shared_midpoint_a",
            "shared_rv_data",
            "CONFIGURED_ASSET-USDT-SHARED-A.OKX",
            true,
        )],
    );
    insert_test_rv_surface(
        &mut loaded,
        "shared_rv_surface_b",
        vec![test_rv_source(
            "shared_midpoint_b",
            "shared_rv_data",
            "CONFIGURED_ASSET-USDT-SHARED-B.OKX",
            true,
        )],
    );

    let keys = trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
        .expect("duplicate RV source clients should still produce transport keys");

    assert_eq!(
        keys.iter()
            .filter(|client_key| client_key.as_str() == "shared_rv_data")
            .count(),
        1,
        "RV source client retention must be a set across surfaces"
    );
}

#[test]
fn trade_transport_subscribed_retains_enabled_unsupported_kind_rv_sources() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("mark_rv_data".to_string(), test_okx_data_client());
    let mut mark_source = test_rv_source(
        "mark_midpoint",
        "mark_rv_data",
        "CONFIGURED_ASSET-USDT-MARK.OKX",
        true,
    );
    mark_source.source_class = RealizedVolatilitySourceClassBlock::Mark;
    mark_source.sample_kind = RealizedVolatilitySampleKindBlock::Mark;
    insert_test_rv_surface(&mut loaded, "mark_rv_surface", vec![mark_source]);

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect(
                "transport retention must over-retain enabled RV sources before runtime validation",
            );

    assert!(
        scoped.root.clients.contains_key("mark_rv_data"),
        "transport retention must include every enabled RV source client, even if later validation rejects the source kind"
    );
}

#[test]
fn trade_transport_subscribed_excludes_disabled_rv_sources() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    loaded
        .root
        .clients
        .insert("disabled_rv_data".to_string(), test_okx_data_client());
    insert_test_rv_surface(
        &mut loaded,
        "disabled_rv_surface",
        vec![test_rv_source(
            "disabled_midpoint",
            "disabled_rv_data",
            "CONFIGURED_ASSET-USDT-DISABLED.OKX",
            false,
        )],
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("disabled RV sources must not affect transport derivation");

    assert!(
        !scoped.root.clients.contains_key("disabled_rv_data"),
        "disabled RV source clients must not be retained"
    );
}

#[test]
fn trade_transport_subscribed_zero_strategies_skips_broken_rv_clients() {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.remove("okx_data");

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("zero-strategy Subscribed transport must not validate or retain RV sources");

    assert!(
        !scoped.root.clients.contains_key("okx_data"),
        "zero-strategy transport must not pull in RV source clients"
    );
}

#[test]
fn trade_transport_not_subscribed_ignores_broken_rv_only_clients() {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("fixture config should load");
    insert_test_rv_surface(
        &mut loaded,
        "broken_rv_surface",
        vec![test_rv_source(
            "broken_midpoint",
            "missing_rv_data",
            "CONFIGURED_ASSET-USDT-MISSING.OKX",
            true,
        )],
    );

    let scoped =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
            .expect("strategy-free transport must not validate or retain RV-only sources");

    assert!(
        !scoped.root.clients.contains_key("missing_rv_data"),
        "NotSubscribed transport must not pull in RV source clients"
    );
}

#[test]
fn trade_transport_handles_absent_and_empty_rv_surface_maps() {
    let mut loaded = fixture_loaded_config();
    loaded.root.realized_volatility_surfaces = None;
    let none_keys =
        trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("absent RV surfaces should still derive transport keys");
    assert!(none_keys.is_empty());

    loaded.root.realized_volatility_surfaces = Some(BTreeMap::new());
    let empty_keys =
        trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
            .expect("empty RV surfaces should still derive transport keys");
    assert!(empty_keys.is_empty());
}

#[test]
fn registration_rejects_rv_source_missing_from_node_transport() {
    let loaded = loaded_config_with_rv_only_source();
    let pruned_loaded =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
            .expect("strategy-free transport should ignore RV-only sources");
    assert!(
        !pruned_loaded.root.clients.contains_key("rv_only_data"),
        "test setup must build a node transport that pruned the RV-only source"
    );
    let mut node = make_bolt_v3_live_node_builder(&pruned_loaded)
        .expect("test LiveNodeBuilder should construct")
        .build()
        .expect("test LiveNode should build");
    let writer: Arc<DecisionEvidenceRecorder> = Arc::new(DecisionEvidenceRecorder::recording());

    let error = register_bolt_v3_strategies_on_node_with_bindings(
        &mut node,
        &loaded,
        &[],
        test_registration_controls(writer.clone()),
        writer,
    )
    .expect_err("registration must fail before RV runtime subscribes a pruned client");

    assert!(matches!(
        error,
        BoltV3StrategyRegistrationError::RealizedVolatilityRuntime { message }
            if message.contains("not registered on this node's transport")
                && message.contains("rv_only_data")
    ));
}

#[test]
fn registration_with_iv_runtime_rejects_rv_source_missing_from_node_transport() {
    let loaded = loaded_config_with_rv_only_source();
    let pruned_loaded =
        trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
            .expect("strategy-free transport should ignore RV-only sources");
    let mut node = make_bolt_v3_live_node_builder(&pruned_loaded)
        .expect("test LiveNodeBuilder should construct")
        .build()
        .expect("test LiveNode should build");
    let writer: Arc<DecisionEvidenceRecorder> = Arc::new(DecisionEvidenceRecorder::recording());
    let iv_runtime = IvRuntimeEngine::from_iv_root(&IvRootConfig {
        schema_version: 1,
        profiles: Vec::new(),
    })
    .expect("empty IV runtime should construct");

    let error = register_bolt_v3_strategies_on_node_with_iv_runtime_bindings(
        &mut node,
        &loaded,
        &[],
        test_registration_controls(writer.clone()),
        writer,
        &iv_runtime,
    )
    .expect_err("IV-runtime registration must fail before RV subscribes a pruned client");

    assert!(matches!(
        error,
        BoltV3StrategyRegistrationError::RealizedVolatilityRuntime { message }
            if message.contains("not registered on this node's transport")
                && message.contains("rv_only_data")
    ));
}
