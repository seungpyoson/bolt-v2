mod support;

use bolt_v2::bolt_v3_config::{
    BoltV3RootConfig, BoltV3StrategyConfig, LoadedStrategy, ReferencePriceDriftPolicy,
    ReferencePriceSelectionPolicy, ReferencePriceStalePolicy,
};
use bolt_v2::bolt_v3_validate::validate_strategies;

fn strategy_without_reference_current_price() -> String {
    let fixture = support::repo_text("tests/fixtures/bolt_v3/strategies/binary_oracle.toml");
    let mut value: toml::Value = toml::from_str(&fixture).expect("strategy fixture should parse");
    value
        .as_table_mut()
        .expect("strategy fixture should be a TOML table")
        .remove("reference_current_price");
    toml::to_string(&value).expect("strategy fixture without reference_current_price should render")
}

fn strategy_with_reference_current_price(reference_current_price: &str) -> String {
    let fixture = strategy_without_reference_current_price();
    format!("{fixture}\n{reference_current_price}")
}

fn root_fixture() -> BoltV3RootConfig {
    let fixture = support::repo_text("tests/fixtures/bolt_v3/root.toml");
    let reference_clients = r#"
[clients.chainlink_reference]
venue = "CHAINLINK_REFERENCE_PRICE"

[clients.chainlink_reference.data]
websocket_endpoint = "wss://example.chain.link/reference"
transport_backend = "sockudo"

[clients.chainlink_reference.secrets]
api_key_ssm_parameter = "/bolt/testnet/chainlink/api-key"
api_secret_ssm_parameter = "/bolt/testnet/chainlink/api-secret"

[clients.polyresearch_reference]
venue = "POLYRESEARCH_REFERENCE_PRICE"

[clients.polyresearch_reference.data]
websocket_endpoint = "wss://example.polyresearch.com/reference"
transport_backend = "sockudo"

[clients.polyresearch_reference.secrets]
api_key_ssm_parameter = "/bolt/polyresearch/api-key"
"#;
    toml::from_str(&format!("{fixture}\n{reference_clients}"))
        .expect("root fixture with reference clients should parse")
}

fn loaded_strategy(strategy: BoltV3StrategyConfig) -> Vec<LoadedStrategy> {
    vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }]
}

fn parse_strategy(reference_current_price: &str) -> BoltV3StrategyConfig {
    toml::from_str(&strategy_with_reference_current_price(
        reference_current_price,
    ))
    .expect("strategy fixture should parse")
}

fn validate_reference_current_price(reference_current_price: &str) -> Vec<String> {
    validate_strategies(
        &root_fixture(),
        &loaded_strategy(parse_strategy(reference_current_price)),
    )
}

#[test]
fn strategy_scoped_reference_current_price_config_parses_ordered_sources_with_defaults() {
    let strategy = parse_strategy(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "polyresearch_backup"]
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
client_id = "polyresearch_reference"
symbol = "BTC"
"#,
    );

    let reference_current_price = strategy
        .reference_current_price
        .expect("reference_current_price block should be present");
    assert_eq!(reference_current_price.asset, "BTC");
    assert_eq!(
        reference_current_price.source_order,
        ["chainlink_primary", "polyresearch_backup"]
    );
    assert_eq!(reference_current_price.min_valid_sources, 1);
    assert_eq!(
        reference_current_price.selection_policy,
        ReferencePriceSelectionPolicy::FirstValidPerInterval
    );
    assert_eq!(reference_current_price.max_source_age_ms, 2000);
    assert_eq!(reference_current_price.max_source_drift_bps, 25);
    assert_eq!(
        reference_current_price.drift_policy,
        ReferencePriceDriftPolicy::Observe
    );
    assert_eq!(
        reference_current_price.stale_policy,
        ReferencePriceStalePolicy::Block
    );

    let chainlink = reference_current_price
        .sources
        .get("chainlink_primary")
        .expect("chainlink source config should be keyed by source id");
    assert_eq!(chainlink.provider.as_str(), "chainlink_ws");
    assert!(chainlink.enabled);
    assert!(!chainlink.required);
    assert_eq!(chainlink.client_id.as_str(), "chainlink_reference");
    assert_eq!(
        chainlink.instrument_id.as_deref(),
        Some("BTC-USD.CHAINLINK")
    );
    assert_eq!(chainlink.symbol.as_deref(), None);

    let polyresearch = reference_current_price
        .sources
        .get("polyresearch_backup")
        .expect("polyresearch source config should be keyed by source id");
    assert_eq!(polyresearch.provider.as_str(), "polyresearch_ws");
    assert!(polyresearch.enabled);
    assert!(!polyresearch.required);
    assert_eq!(polyresearch.client_id.as_str(), "polyresearch_reference");
    assert_eq!(polyresearch.symbol.as_deref(), Some("BTC"));
    assert_eq!(polyresearch.instrument_id.as_deref(), None);
}

#[test]
fn reference_current_price_validation_rejects_duplicate_ordered_source_keys() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.sources")
                && message.contains("chainlink_primary")
                && message.contains("duplicate")
        }),
        "duplicate source keys should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_empty_source_list() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = []
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source]
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.sources") && message.contains("non-empty")
        }),
        "empty source list should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_missing_source_section() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "missing_backup"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.sources")
                && message.contains("missing_backup")
                && message.contains("missing [reference_current_price.source.missing_backup]")
        }),
        "missing source section should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_unlisted_source_section() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.unlisted_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "BTC"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("[reference_current_price.source.unlisted_backup]")
                && message.contains("not listed")
        }),
        "unlisted source section should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_min_valid_sources_above_enabled_source_count() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "polyresearch_backup"]
min_valid_sources = 2
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = false
required = false
client_id = "polyresearch_reference"
symbol = "BTC"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.min_valid_sources")
                && message.contains("2")
                && message.contains("enabled source count 1")
        }),
        "min_valid_sources above enabled source count should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_disabled_required_source() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "polyresearch_backup"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = false
required = true
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "BTC"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_primary")
                && message.contains("required")
                && message.contains("disabled")
        }),
        "disabled required reference_current_price.source should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_blank_asset() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = ""
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.asset") && message.contains("non-empty")
        }),
        "blank reference_current_price.asset should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_non_positive_thresholds() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 0
max_source_drift_bps = 0
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages
            .iter()
            .any(|message| message.contains("reference_current_price.max_source_age_ms")),
        "non-positive max_source_age_ms should fail validation, got: {messages:#?}"
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("reference_current_price.max_source_drift_bps")),
        "non-positive max_source_drift_bps should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_chainlink_source_without_instrument_id() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
symbol = "BTC"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_primary.instrument_id")
                && message.contains("chainlink_ws")
        }),
        "chainlink_ws source without instrument_id should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_source_identifier_for_wrong_asset() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "ETH-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_primary.instrument_id")
                && message.contains("BTC")
                && message.contains("ETH-USD.CHAINLINK")
        }),
        "wrong-asset provider identifier should fail validation, got: {messages:#?}"
    );
}

#[test]
fn optional_unsupported_polyresearch_source_accepts_when_quorum_can_be_met() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BNB"
sources = ["chainlink_primary", "polyresearch_backup"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.chainlink_primary]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BNB-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "BNB"
"#,
    );

    assert!(
        messages.is_empty(),
        "optional unsupported PRR source should not block when quorum can be met, got: {messages:#?}"
    );
}

#[test]
fn required_unsupported_polyresearch_source_rejects() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BNB"
sources = ["polyresearch_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 2000
max_source_drift_bps = 25
drift_policy = "observe"
stale_policy = "block"

[reference_current_price.source.polyresearch_primary]
provider = "polyresearch_ws"
enabled = true
required = true
client_id = "polyresearch_reference"
symbol = "BNB"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.polyresearch_primary")
                && message.contains("BNB")
                && message.contains("polyresearch_ws")
                && message.contains("unsupported")
        }),
        "required unsupported polyresearch_ws asset should fail validation, got: {messages:#?}"
    );
}
