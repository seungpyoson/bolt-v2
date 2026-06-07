use bolt_v2::{
    bolt_v3_config::ClientBlock,
    bolt_v3_providers::{
        binding_for_provider_key, chainlink_reference, polyresearch, validate_client_block,
    },
};

fn client_from_toml(toml: &str) -> ClientBlock {
    toml::from_str(toml).expect("test client block should parse")
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
websocket_endpoint = "wss://streams.chain.link/reference"
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
websocket_endpoint = "wss://streams.chain.link/reference"
transport_backend = "sockudo"
heartbeat_secs = 5
heartbeat_message = "ping"
reconnect_timeout_ms = 5000
reconnect_delay_initial_ms = 250
reconnect_delay_max_ms = 5000
reconnect_backoff_factor = 1.5
reconnect_jitter_ms = 100
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
