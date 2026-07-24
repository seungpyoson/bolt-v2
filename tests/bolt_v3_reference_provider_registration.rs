use bolt_v2::{
    bolt_v3_boundary_registry::{BOUNDARY_REGISTRY, BoundaryEvidenceClass, BoundaryFeeder},
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        ReferencePriceIdentifierKind, ReferencePriceProviderMetadata, binding_for_provider_key,
        chainlink_reference, polyresearch, reference_price_provider_metadata_entries,
        validate_client_block,
    },
};

const REQUIRED_REFERENCE_PRICE_FEEDERS: [BoundaryFeeder; 2] = [
    BoundaryFeeder::ReferenceCurrentPriceHealth,
    BoundaryFeeder::ReferenceLiveProbe,
];

fn client_from_toml(toml: &str) -> ClientBlock {
    // These provider-block fixtures bypass root validation, so their inline 5000ms values do not satisfy the root startup-bound rule.
    toml::from_str(toml).expect("test client block should parse")
}

fn missing_websocket_frame_registry_rows(
    metadata_entries: &[ReferencePriceProviderMetadata],
) -> Vec<(&'static str, BoundaryFeeder)> {
    let mut missing = Vec::new();
    for metadata in metadata_entries {
        for feeder in REQUIRED_REFERENCE_PRICE_FEEDERS {
            let registered = BOUNDARY_REGISTRY.iter().any(|entry| {
                entry.adapter_id == metadata.client_venue_key
                    && entry.class == BoundaryEvidenceClass::WebSocketFrame
                    && entry.feeder == feeder
            });
            if !registered {
                missing.push((metadata.client_venue_key, feeder));
            }
        }
    }
    missing
}

#[test]
fn reference_price_providers_have_websocket_frame_boundary_registry_rows() {
    let missing =
        missing_websocket_frame_registry_rows(reference_price_provider_metadata_entries());
    assert!(
        missing.is_empty(),
        "reference price provider metadata missing WebSocketFrame registry row(s): {missing:?}"
    );
}

#[test]
fn polymarket_nt_reconciliation_has_provider_boundary_registry_row() {
    assert!(BOUNDARY_REGISTRY.iter().any(|entry| {
        entry.adapter_id == "PolymarketExecutionClient"
            && entry.class == BoundaryEvidenceClass::HttpResponseBody
            && entry.feeder == BoundaryFeeder::PolymarketNtExecutionReconciliation
    }));
}

#[test]
fn boundary_registry_completeness_rejects_string_literal_non_reference_provider_key() {
    let planted = [ReferencePriceProviderMetadata {
        provider_key: "pyth_ws",
        client_venue_key: "PYTH_REFERENCE_PRICE",
        identifier_kind: ReferencePriceIdentifierKind::Symbol,
        supported_assets: &[],
        emits_live_input_health: true,
    }];

    let missing = missing_websocket_frame_registry_rows(&planted);

    assert_eq!(
        missing,
        vec![
            (
                "PYTH_REFERENCE_PRICE",
                BoundaryFeeder::ReferenceCurrentPriceHealth
            ),
            ("PYTH_REFERENCE_PRICE", BoundaryFeeder::ReferenceLiveProbe),
        ]
    );
}

#[test]
fn chainlink_reference_provider_binding_is_registered_as_data_only() {
    let binding = binding_for_provider_key(chainlink_reference::KEY)
        .expect("chainlink reference provider binding should be registered");
    assert_eq!(binding.key, "CHAINLINK_REFERENCE_PRICE");
    assert!(binding.supported_market_families.is_empty());
    assert_eq!(binding.required_secret_blocks.len(), 1);

    let client = client_from_toml(
        r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    );
    assert_eq!(
        validate_client_block("chainlink_reference", &client),
        Vec::<String>::new()
    );

    let execution_client = client_from_toml(
        r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
idle_timeout_ms = 10000

[execution]

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    );
    let errors = validate_client_block("chainlink_reference", &execution_client);
    assert!(
        errors.iter().any(|message| {
            message.contains("CHAINLINK_REFERENCE_PRICE") && message.contains("data-only")
        }),
        "chainlink reference execution block should fail validation, got: {errors:#?}"
    );
}

#[test]
fn chainlink_reference_requires_provider_level_reconnect_budget_for_fresh_auth_headers() {
    let missing_reconnect_bound = client_from_toml(
        r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    );
    let errors = validate_client_block("chainlink_reference", &missing_reconnect_bound);
    assert!(
        errors.iter().any(
            |message| message.contains("reconnect_max_attempts") && message.contains("missing")
        ),
        "missing Chainlink reconnect_max_attempts should fail validation, got: {errors:#?}"
    );

    let zero_reconnect = client_from_toml(
        r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = 0
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    );
    let errors = validate_client_block("chainlink_reference", &zero_reconnect);
    assert!(
        errors.iter().any(|message| {
            message.contains("reconnect_max_attempts")
                && message.contains("positive")
                && message.contains("unlimited")
        }),
        "zero Chainlink reconnect_max_attempts should fail validation, got: {errors:#?}"
    );
}

#[test]
fn chainlink_reference_rejects_text_heartbeat_message() {
    let client = client_from_toml(
        r#"
venue = "CHAINLINK_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://streams.chain.link"
websocket_path = "/api/v1/ws"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"
"#,
    );
    let errors = validate_client_block("chainlink_reference", &client);
    assert!(
        errors.iter().any(|message| {
            message.contains("heartbeat_message")
                && message.contains("must be omitted")
                && message.contains("protocol Ping")
        }),
        "Chainlink text heartbeat should fail validation, got: {errors:#?}"
    );
}

#[test]
fn polyresearch_reference_provider_binding_is_registered_as_data_only() {
    let binding = binding_for_provider_key(polyresearch::KEY)
        .expect("polyresearch reference provider binding should be registered");
    assert_eq!(binding.key, "POLYRESEARCH_REFERENCE_PRICE");
    assert!(binding.supported_market_families.is_empty());
    assert_eq!(binding.required_secret_blocks.len(), 1);

    let client = client_from_toml(
        r#"
venue = "POLYRESEARCH_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://stream.polyresearch.example/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
subscribe_ack_timeout_ms = 2000
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#,
    );
    assert_eq!(
        validate_client_block("polyresearch_reference", &client),
        Vec::<String>::new()
    );

    let execution_client = client_from_toml(
        r#"
venue = "POLYRESEARCH_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://stream.polyresearch.example/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = "unlimited"
subscribe_ack_timeout_ms = 2000
idle_timeout_ms = 10000

[execution]

[secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#,
    );
    let errors = validate_client_block("polyresearch_reference", &execution_client);
    assert!(
        errors.iter().any(|message| {
            message.contains("POLYRESEARCH_REFERENCE_PRICE") && message.contains("data-only")
        }),
        "polyresearch reference execution block should fail validation, got: {errors:#?}"
    );
}

#[test]
fn polyresearch_reference_provider_requires_toml_owned_reconnect_max_attempts() {
    let client = client_from_toml(
        r#"
venue = "POLYRESEARCH_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://stream.polyresearch.example/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#,
    );
    let errors = validate_client_block("polyresearch_reference", &client);
    assert!(
        errors.iter().any(|message| {
            message.contains("reconnect_max_attempts") && message.contains("missing")
        }),
        "missing PolyResearch reconnect_max_attempts should fail validation, got: {errors:#?}"
    );
}

#[test]
fn polyresearch_reference_provider_rejects_zero_reconnect_max_attempts() {
    let client = client_from_toml(
        r#"
venue = "POLYRESEARCH_REFERENCE_PRICE"

[data]
websocket_endpoint = "wss://stream.polyresearch.example/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
reconnect_max_attempts = 0
subscribe_ack_timeout_ms = 2000
idle_timeout_ms = 10000

[secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#,
    );
    let errors = validate_client_block("polyresearch_reference", &client);
    assert!(
        errors.iter().any(|message| {
            message.contains("reconnect_max_attempts")
                && message.contains("positive")
                && message.contains("unlimited")
        }),
        "zero PolyResearch reconnect_max_attempts should fail validation, got: {errors:#?}"
    );
}
