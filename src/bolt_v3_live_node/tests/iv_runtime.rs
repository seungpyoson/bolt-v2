#![cfg(test)]

use super::*;

#[test]
fn live_node_startup_applies_iv_subscription_plans_to_runtime_source_health() {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.clear();
    insert_configured_data_client(&mut loaded);
    loaded.root.nautilus.data_engine.external_clients = vec![ClientId::from("configured-client")];
    loaded.root.iv = Some(
        toml::from_str(
            r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
        )
        .expect("configured IV profile should parse"),
    );
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let adapters = BoltV3AdapterConfigs {
        clients: BTreeMap::new(),
    };

    let (runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
        &loaded,
        &resolved,
        adapters,
        BTreeMap::new(),
    )
    .expect("configured external IV source should build without live transport");

    assert!(runtime.has_iv_runtime());
    let health = runtime
        .iv_source_health("configured-profile", "configured-greeks-source")
        .expect("startup should apply IV source health");
    assert_eq!(
        health.subscription_state,
        crate::bolt_v3_iv::health::IvSourceHealthState::Subscribing
    );
    assert_eq!(health.subscription_generation, 7);
}

#[test]
fn live_node_startup_rejects_unknown_iv_data_client() {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.clear();
    loaded.root.nautilus.data_engine.external_clients.clear();
    loaded.root.iv = Some(
        toml::from_str(
            r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "missing-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
        )
        .expect("configured IV profile should parse"),
    );
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let adapters = BoltV3AdapterConfigs {
        clients: BTreeMap::new(),
    };

    let error = build_live_node_with_clients_and_submit_approval_limits(
        &loaded,
        &resolved,
        adapters,
        BTreeMap::new(),
    )
    .expect_err("unknown IV source client must reject before live-node build");

    assert!(format!("{error:?}").contains("missing-client"));
}

#[test]
fn iv_option_greeks_identifier_list_rejects_before_runtime_commands() {
    let ids = vec![
        "BTC-20240101-50000-C.DERIBIT".to_string(),
        "configured-invalid-option-instrument".to_string(),
    ];

    let error = parse_option_greeks_instrument_ids(&ids).expect_err("invalid ID should reject");

    assert!(error.contains("invalid NT option-greeks instrument_id"));
    assert!(error.contains("configured-invalid-option-instrument"));
}

#[test]
fn iv_option_chain_identifier_list_rejects_before_runtime_commands() {
    let ids = vec![
        "DERIBIT:BTC:BTC:2024-01-01".to_string(),
        "configured-invalid-option-series".to_string(),
    ];

    let error = parse_option_chain_series_ids(&ids).expect_err("invalid ID should reject");

    assert!(error.contains("invalid NT option-chain series_id"));
    assert!(error.contains("configured-invalid-option-series"));
}

#[test]
fn iv_option_greeks_start_plan_translates_to_runtime_data_command() {
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeOptionGreeks,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
            nt_params: toml::Value::Table(toml::map::Map::new()),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };

    let commands = iv_runtime_data_commands_for_plan(&plan)
        .expect("valid option-greeks plan should translate to an NT data command");

    assert_eq!(commands.len(), 1);
    match &commands[0] {
        nautilus_common::messages::data::DataCommand::Subscribe(
            SubscribeCommand::OptionGreeks(command),
        ) => {
            assert_eq!(
                command.instrument_id,
                InstrumentId::from("BTC-20240101-50000-C.DERIBIT")
            );
            assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
        }
        other => panic!("expected option-greeks subscribe command, got {other:?}"),
    }
}

#[test]
fn iv_remove_source_plan_translates_to_no_runtime_data_commands() {
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::SourceRemoval,
        operation: IvRuntimeOperation::RemoveSource,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
            nt_params: toml::Value::Table(toml::map::Map::new()),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };

    let commands = iv_runtime_data_commands_for_plan(&plan)
        .expect("source removal should not require NT data commands");

    assert!(commands.is_empty());
}

#[test]
fn iv_option_chain_start_plan_translates_parseable_strike_range_to_runtime_data_command() {
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-chain-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeOptionChain,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionChain,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceOptionChain {
            series_ids: vec!["DERIBIT:BTC:BTC:2024-01-01T00:00:00Z".to_string()],
            strike_range_policy: "atm_relative:1:1".to_string(),
            nt_params: toml::toml! {
                snapshot_interval_ms = 250
            }
            .into(),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };

    let commands = iv_runtime_data_commands_for_plan(&plan)
        .expect("valid option-chain plan should translate to an NT data command");

    assert_eq!(commands.len(), 1);
    match &commands[0] {
        nautilus_common::messages::data::DataCommand::Subscribe(SubscribeCommand::OptionChain(
            command,
        )) => {
            assert_eq!(
                command.series_id,
                OptionSeriesId::from_str("DERIBIT:BTC:BTC:2024-01-01T00:00:00Z").unwrap()
            );
            assert_eq!(
                command.strike_range,
                StrikeRange::AtmRelative {
                    strikes_above: 1,
                    strikes_below: 1,
                }
            );
            assert_eq!(command.snapshot_interval_ms, Some(250));
            assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
        }
        other => panic!("expected option-chain subscribe command, got {other:?}"),
    }
}

#[test]
fn iv_custom_iv_start_plan_translates_to_runtime_custom_data_command() {
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-custom-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeCustomData,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::CustomData,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceCustomImpliedVolatility {
            custom_iv_data_type: "ConfiguredCustomIvEvent".to_string(),
            custom_iv_data_fields: vec!["configured_iv".to_string()],
            nt_params: toml::toml! {
                configured_selector_param = "selector-value"
            }
            .into(),
        },
        params: toml::toml! {
            configured_source_param = "source-value"
        }
        .into(),
        subscription_generation: 7,
    };

    let commands = iv_runtime_data_commands_for_plan(&plan)
        .expect("valid custom-IV plan should translate to an NT custom-data command");

    assert_eq!(commands.len(), 1);
    match &commands[0] {
        nautilus_common::messages::data::DataCommand::Subscribe(SubscribeCommand::Data(
            command,
        )) => {
            assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
            assert_eq!(command.data_type.type_name(), "ConfiguredCustomIvEvent");
            assert_eq!(
                command.data_type.identifier(),
                Some("configured-custom-source")
            );
            let metadata = command
                .data_type
                .metadata()
                .expect("custom-IV data type should carry merged params");
            assert_eq!(
                metadata.get("configured_source_param"),
                Some(&serde_json::Value::String("source-value".to_string()))
            );
            assert_eq!(
                metadata.get("configured_selector_param"),
                Some(&serde_json::Value::String("selector-value".to_string()))
            );
            assert_eq!(command.params.as_ref(), Some(metadata));
        }
        other => panic!("expected custom-IV data subscribe command, got {other:?}"),
    }
}

#[test]
fn iv_aggregate_greeks_start_plan_translates_to_runtime_custom_data_command() {
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-aggregate-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeAggregateGreeks,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::AggregateGreeksTopic,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceAggregateGreeks {
            aggregate_key: "ConfiguredAggregateGreeksEvent".to_string(),
            underlying_selectors: vec!["configured-underlying-selector".to_string()],
            delta_field: "configured_delta".to_string(),
            gamma_field: "configured_gamma".to_string(),
            vega_field: "configured_vega".to_string(),
            theta_field: "configured_theta".to_string(),
            rho_field: "configured_rho".to_string(),
            iv_field: Some("configured_iv".to_string()),
            iv_basis: None,
            iv_convention: None,
            nt_params: toml::toml! {
                configured_selector_param = "selector-value"
            }
            .into(),
        },
        params: toml::toml! {
            configured_source_param = "source-value"
        }
        .into(),
        subscription_generation: 7,
    };

    let commands = iv_runtime_data_commands_for_plan(&plan)
        .expect("valid aggregate-greeks plan should translate to an NT custom-data command");

    assert_eq!(commands.len(), 1);
    match &commands[0] {
        nautilus_common::messages::data::DataCommand::Subscribe(SubscribeCommand::Data(
            command,
        )) => {
            assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
            assert_eq!(
                command.data_type.type_name(),
                "ConfiguredAggregateGreeksEvent"
            );
            assert_eq!(
                command.data_type.identifier(),
                Some("configured-aggregate-source")
            );
            let metadata = command
                .data_type
                .metadata()
                .expect("aggregate-greeks data type should carry merged params");
            assert_eq!(
                metadata.get("underlying_selectors"),
                Some(&serde_json::Value::Array(vec![serde_json::Value::String(
                    "configured-underlying-selector".to_string()
                )]))
            );
            assert_eq!(
                metadata.get("configured_source_param"),
                Some(&serde_json::Value::String("source-value".to_string()))
            );
            assert_eq!(
                metadata.get("configured_selector_param"),
                Some(&serde_json::Value::String("selector-value".to_string()))
            );
            assert_eq!(command.params.as_ref(), Some(metadata));
        }
        other => panic!("expected aggregate-greeks data subscribe command, got {other:?}"),
    }
}

#[derive(Debug)]
struct RecordingDataCommandSender {
    commands: std::sync::Arc<std::sync::Mutex<Vec<DataCommand>>>,
}

impl nautilus_common::runner::DataCommandSender for RecordingDataCommandSender {
    fn execute(&self, command: DataCommand) {
        self.commands
            .lock()
            .expect("recording data command sender lock should not be poisoned")
            .push(command);
    }
}

struct DataCommandSenderRestore;

impl Drop for DataCommandSenderRestore {
    fn drop(&mut self) {
        nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
            nautilus_common::runner::SyncDataCommandSender,
        ));
    }
}

#[test]
fn iv_runtime_command_sender_adapter_queues_start_plan_after_runner_sender_is_bound() {
    let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
        RecordingDataCommandSender {
            commands: commands.clone(),
        },
    ));
    let _restore_sender = DataCommandSenderRestore;
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeOptionGreeks,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
            nt_params: toml::Value::Table(toml::map::Map::new()),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };
    let mut adapter =
        NtIvRuntimeCommandSenderAdapter::new(&[ClientId::from("configured-client")], &[]);

    adapter
        .apply_subscription_plan(&plan)
        .expect("valid runtime start plan should be queued");

    let commands = commands
        .lock()
        .expect("recording data command sender lock should not be poisoned");
    assert_eq!(commands.len(), 1);
    assert!(matches!(
        &commands[0],
        DataCommand::Subscribe(SubscribeCommand::OptionGreeks(_))
    ));
}

#[test]
fn iv_runtime_command_sender_adapter_rejects_unknown_start_client_without_queueing() {
    let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
        RecordingDataCommandSender {
            commands: commands.clone(),
        },
    ));
    let _restore_sender = DataCommandSenderRestore;
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeOptionGreeks,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
        client_id: "missing-client".to_string(),
        selector: IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
            nt_params: toml::Value::Table(toml::map::Map::new()),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };
    let mut adapter = NtIvRuntimeCommandSenderAdapter::new(&[], &[]);

    let error = adapter
        .apply_subscription_plan(&plan)
        .expect_err("unknown runtime start client should reject before queueing");

    assert_eq!(error.reason, IvRejectReason::SubscriptionFailed);
    assert!(error.message.contains("not registered"));
    assert!(
        commands
            .lock()
            .expect("recording data command sender lock should not be poisoned")
            .is_empty(),
        "invalid start client must not enqueue a data command"
    );
}

#[test]
fn iv_runtime_command_sender_adapter_skips_external_start_client_without_queueing() {
    let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
        RecordingDataCommandSender {
            commands: commands.clone(),
        },
    ));
    let _restore_sender = DataCommandSenderRestore;
    let plan = IvSubscriptionPlan {
        profile_id: "configured-profile".to_string(),
        source_id: "configured-greeks-source".to_string(),
        lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
        operation: IvRuntimeOperation::SubscribeOptionGreeks,
        nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
        client_id: "configured-client".to_string(),
        selector: IvSelector::SourceOptionGreeks {
            instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
            nt_params: toml::Value::Table(toml::map::Map::new()),
        },
        params: toml::Value::Table(toml::map::Map::new()),
        subscription_generation: 7,
    };
    let mut adapter =
        NtIvRuntimeCommandSenderAdapter::new(&[], &[ClientId::from("configured-client")]);

    adapter
        .apply_subscription_plan(&plan)
        .expect("external start client should be accepted without NT queueing");

    assert!(
        commands
            .lock()
            .expect("recording data command sender lock should not be poisoned")
            .is_empty(),
        "external start client is managed outside the runtime sender"
    );
}

#[test]
fn live_node_runtime_stop_applies_iv_unsubscribe_lifecycle() {
    let loaded = fixture_loaded_config_with_external_option_greeks_iv();
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let adapters = BoltV3AdapterConfigs {
        clients: BTreeMap::new(),
    };

    let (mut runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
        &loaded,
        &resolved,
        adapters,
        BTreeMap::new(),
    )
    .expect("configured external IV source should build without live transport");
    assert!(
        runtime.has_iv_event_bindings(),
        "startup should install IV receive-side bindings"
    );

    runtime
        .stop_iv_engine_lifecycle(&loaded.root)
        .expect("IV stop lifecycle should apply unsubscribe plans");
    assert!(
        !runtime.has_iv_event_bindings(),
        "stop should drop IV receive-side bindings"
    );
    assert!(
        !runtime.has_iv_runtime(),
        "stop should clear the IV runtime after applying unsubscribe outcomes"
    );
    assert!(
        runtime
            .iv_source_health("configured-profile", "configured-greeks-source")
            .is_none(),
        "stopped live node should not expose IV source health through a retained runtime"
    );
}

#[test]
fn live_node_runtime_stop_planning_failure_keeps_iv_runtime() {
    let loaded = fixture_loaded_config_with_external_option_greeks_iv();
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let adapters = BoltV3AdapterConfigs {
        clients: BTreeMap::new(),
    };

    let (mut runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
        &loaded,
        &resolved,
        adapters,
        BTreeMap::new(),
    )
    .expect("configured external IV source should build without live transport");
    assert!(runtime.has_iv_runtime());
    assert!(runtime.has_iv_event_bindings());

    let mut invalid_stop_root = loaded.root.clone();
    let profile = invalid_stop_root
        .iv
        .as_mut()
        .and_then(|iv| iv.profiles.first_mut())
        .expect("fixture should include one IV profile");
    let duplicate_source = profile
        .sources
        .first()
        .expect("fixture should include one IV source")
        .clone();
    profile.sources.push(duplicate_source);

    let error = runtime
        .stop_iv_engine_lifecycle(&invalid_stop_root)
        .expect_err("invalid stop lifecycle planning should fail");

    assert!(
        error.to_string().contains("DuplicateSourceId"),
        "failure should identify duplicate-source stop planning: {error}"
    );
    assert!(
        runtime.has_iv_runtime(),
        "failed stop planning must not drop the IV runtime"
    );
    assert!(
        runtime.has_iv_event_bindings(),
        "failed stop planning must not drop IV event bindings"
    );
}

#[test]
fn live_node_startup_binds_aggregate_greeks_sources_through_nt_custom_data() {
    let mut loaded = fixture_loaded_config();
    loaded.root.clients.clear();
    insert_configured_data_client(&mut loaded);
    loaded.root.nautilus.data_engine.external_clients = vec![ClientId::from("configured-client")];
    loaded.root.iv = Some(
        toml::from_str(
            r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health", "aggregate_greeks"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["aggregate_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-aggregate-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health", "aggregate_greeks"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-aggregate-source"
selector_fingerprint = "configured-aggregate-selector"
source_kind = "aggregate_greeks"
client_id = "configured-client"
subscription_generation = 11
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredAggregateGreeks"

[profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "configured-aggregate-greeks-topic"
underlying_selectors = ["configured-underlying-selector"]
delta_field = "configured-delta-field"
gamma_field = "configured-gamma-field"
vega_field = "configured-vega-field"
theta_field = "configured-theta-field"
rho_field = "configured-rho-field"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
        )
        .expect("configured aggregate IV profile should parse"),
    );
    let resolved = ResolvedBoltV3Secrets {
        clients: BTreeMap::new(),
    };
    let adapters = BoltV3AdapterConfigs {
        clients: BTreeMap::new(),
    };

    let (runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
        &loaded,
        &resolved,
        adapters,
        BTreeMap::new(),
    )
    .expect("configured aggregate IV source should build without live transport");

    let health = runtime
        .iv_source_health("configured-profile", "configured-aggregate-source")
        .expect("startup should apply aggregate IV source health");
    assert_eq!(
        health.subscription_state,
        crate::bolt_v3_iv::health::IvSourceHealthState::Subscribing
    );
    assert_eq!(health.subscription_generation, 11);
}
