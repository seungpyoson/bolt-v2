use crate::support;

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
    root_fixture_for_asset("BTC")
}

fn root_fixture_for_asset(asset: &str) -> BoltV3RootConfig {
    let fixture = support::repo_text("tests/fixtures/bolt_v3/root.toml");
    let reference_price_fixture_additions = r#"
[[chainlink_data_streams.feed_bindings]]
feed_id = "0x00057da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439"
instrument_id = "BTC-USD.CHAINLINK"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 8

[[chainlink_data_streams.feed_bindings]]
feed_id = "0x00047da06d56d083fe599397a4769a042d63aa73dc4ef57709d31e9971a5b439"
instrument_id = "ADA-USD.CHAINLINK"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 8
"#;
    let mut root: BoltV3RootConfig =
        toml::from_str(&format!("{fixture}\n{reference_price_fixture_additions}"))
            .expect("root fixture with reference clients should parse");
    root.realized_volatility_surfaces
        .as_mut()
        .and_then(|surfaces| surfaces.get_mut("configured_rv_surface"))
        .expect("root fixture should include configured RV surface")
        .canonical_base_asset = asset.to_string();
    root
}

fn set_target_underlying_asset(strategy: &mut BoltV3StrategyConfig, asset: &str) {
    strategy
        .target
        .as_table_mut()
        .expect("strategy target should be a table")
        .insert(
            "underlying_asset".to_string(),
            toml::Value::String(asset.to_string()),
        );
}

fn loaded_strategy(strategy: BoltV3StrategyConfig) -> Vec<LoadedStrategy> {
    vec![LoadedStrategy {
        config_path: support::repo_path("tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        relative_path: "strategies/binary_oracle.toml".to_string(),
        config: strategy,
    }]
}

fn parse_strategy(reference_current_price: &str) -> BoltV3StrategyConfig {
    try_parse_strategy(reference_current_price).expect("strategy fixture should parse")
}

fn try_parse_strategy(
    reference_current_price: &str,
) -> Result<BoltV3StrategyConfig, toml::de::Error> {
    toml::from_str(&strategy_with_reference_current_price(
        reference_current_price,
    ))
}

fn validate_reference_current_price(reference_current_price: &str) -> Vec<String> {
    validate_reference_current_price_for_asset(reference_current_price, "BTC")
}

fn validate_reference_current_price_for_asset(
    reference_current_price: &str,
    asset: &str,
) -> Vec<String> {
    let mut strategy = parse_strategy(reference_current_price);
    set_target_underlying_asset(&mut strategy, asset);
    validate_strategies(&root_fixture_for_asset(asset), &loaded_strategy(strategy))
}

#[test]
fn binary_oracle_edge_taker_requires_reference_current_price_block() {
    let strategy: BoltV3StrategyConfig =
        toml::from_str(&strategy_without_reference_current_price())
            .expect("strategy without reference_current_price should still parse");
    let messages = validate_strategies(&root_fixture(), &loaded_strategy(strategy));

    assert!(
        messages.iter().any(|message| {
            message.contains("binary_oracle_edge_taker")
                && message.contains("reference_current_price")
                && message.contains("requires")
        }),
        "missing reference_current_price block should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_source_age_must_fit_inside_forced_flat_grace() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
min_valid_sources = 1
selection_policy = "first_valid_per_interval"
max_source_age_ms = 295001
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
            message.contains("reference_current_price.max_source_age_ms")
                && message.contains("forced_flat_stale_reference_ms")
                && message.contains("retry_interval")
        }),
        "stale-reference grace relation should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_asset_must_match_target_underlying_asset() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "ADA"
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
instrument_id = "ADA-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.asset")
                && message.contains("target.underlying_asset")
                && message.contains("ADA")
                && message.contains("BTC")
        }),
        "reference_current_price.asset should match target.underlying_asset, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_duplicate_enabled_physical_sources() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary", "chainlink_alias"]
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

[reference_current_price.source.chainlink_alias]
provider = "chainlink_ws"
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_alias")
                && message.contains("chainlink_primary")
                && message.contains("same physical reference feed")
                && message.contains("BTC-USD.CHAINLINK")
        }),
        "duplicate enabled physical source should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_requires_explicit_min_valid_sources() {
    let err = try_parse_strategy(
        r#"
[reference_current_price]
asset = "BTC"
sources = ["chainlink_primary"]
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
    )
    .expect_err("reference_current_price.min_valid_sources must be explicit TOML");

    assert!(
        err.to_string().contains("min_valid_sources"),
        "missing min_valid_sources should be the parse error, got: {err}"
    );
}

#[test]
fn strategy_rejects_legacy_reference_data_block() {
    let err = try_parse_strategy(
        r#"
[reference_data]
data_client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

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
"#,
    )
    .expect_err("legacy reference_data must fail closed at strategy schema parse");
    assert!(
        err.to_string().contains("reference_data"),
        "legacy reference_data rejection should name the removed block, got: {err}"
    );
}

#[test]
fn reference_current_price_rejects_malformed_provider_key_at_parse() {
    let err = try_parse_strategy(
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
provider = "chainlink_ws "
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    )
    .expect_err("malformed reference_current_price provider key must fail schema parse");
    assert!(
        err.to_string()
            .contains("reference_price provider is invalid"),
        "malformed provider rejection should come from ReferencePriceProvider validation, got: {err}"
    );
}

#[test]
fn reference_current_price_requires_explicit_source_table() {
    let err = try_parse_strategy(
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
"#,
    )
    .expect_err("reference_current_price.source must be explicit TOML");

    assert!(
        err.to_string().contains("source"),
        "missing source table should be the parse error, got: {err}"
    );
}

#[test]
fn reference_current_price_sources_require_explicit_enabled_and_required() {
    let err = try_parse_strategy(
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
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    )
    .expect_err("reference_current_price source enabled/required flags must be explicit TOML");

    let message = err.to_string();
    assert!(
        message.contains("enabled") || message.contains("required"),
        "missing enabled/required should be the parse error, got: {message}"
    );
}

#[test]
fn reference_current_price_config_parses_ordered_sources_with_explicit_fields() {
    let strategy = parse_strategy(
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
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "BTC/USD"
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
    assert_eq!(polyresearch.symbol.as_deref(), Some("BTC/USD"));
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
fn unlisted_source_does_not_reduce_listed_source_quorum() {
    let messages = validate_reference_current_price_for_asset(
        r#"
[reference_current_price]
asset = "ADA"
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
instrument_id = "ADA-USD.CHAINLINK"

[reference_current_price.source.unlisted_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "ADA/USD"
"#,
        "ADA",
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("[reference_current_price.source.unlisted_backup]")
                && message.contains("not listed")
        }),
        "unlisted source section should fail validation, got: {messages:#?}"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.contains("cannot be met by 0 enabled supported source")),
        "unlisted source must not reduce the listed-source quorum, got: {messages:#?}"
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
fn reference_current_price_validation_rejects_disabled_source_with_missing_client() {
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
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = false
required = false
client_id = "missing_polyresearch_reference"
symbol = "BTC/USD"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.polyresearch_backup.client_id")
                && message.contains("missing_polyresearch_reference")
        }),
        "disabled reference_current_price.source with missing client should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_disabled_source_with_missing_identifier() {
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
enabled = true
required = false
client_id = "chainlink_reference"
instrument_id = "BTC-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = false
required = false
client_id = "polyresearch_reference"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.polyresearch_backup.symbol")
                && message.contains("required")
                && message.contains("polyresearch_ws")
        }),
        "disabled reference_current_price.source with missing symbol should fail validation, got: {messages:#?}"
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
fn reference_current_price_validation_rejects_lowercase_asset_with_precise_message() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "btc"
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
            message.contains("reference_current_price.asset")
                && message.contains("uppercase")
                && message.contains("ASCII")
        }),
        "lowercase reference_current_price.asset should fail with a precise validation message, got: {messages:#?}"
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
fn reference_current_price_validation_rejects_chainlink_instrument_missing_from_shared_catalog() {
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
required = true
client_id = "chainlink_reference"
instrument_id = "BTC-USD.UNKNOWN"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_primary.instrument_id")
                && message.contains("provider catalog")
                && !message.contains("chainlink_data_streams.feed_bindings")
        }),
        "catalog-missing reference instrument should fail with provider-agnostic validation text, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_accepts_polyresearch_feed_symbol_for_asset() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
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
symbol = "BTC/USD"
"#,
    );

    assert!(
        messages.is_empty(),
        "polyresearch_ws provider feed symbol for asset should validate cleanly, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_polyresearch_feed_symbol_for_wrong_asset() {
    let messages = validate_reference_current_price(
        r#"
[reference_current_price]
asset = "BTC"
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
symbol = "ETH/USD"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.polyresearch_primary.symbol")
                && message.contains("BTC")
                && message.contains("ETH/USD")
        }),
        "wrong-asset polyresearch_ws feed symbol should fail validation, got: {messages:#?}"
    );
}

#[test]
fn reference_current_price_validation_rejects_provider_client_venue_mismatch() {
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
required = true
client_id = "chainlink_strike"
instrument_id = "BTC-USD.CHAINLINK"
"#,
    );

    assert!(
        messages.iter().any(|message| {
            message.contains("reference_current_price.source.chainlink_primary.client_id")
                && message.contains("chainlink_strike")
                && message.contains("CHAINLINK_REFERENCE_PRICE")
        }),
        "reference_current_price source provider/client venue mismatch should fail validation, got: {messages:#?}"
    );
}

#[test]
fn polyresearch_source_for_unlisted_asset_is_supported_by_configured_symbol() {
    let messages = validate_reference_current_price_for_asset(
        r#"
[reference_current_price]
asset = "ADA"
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
instrument_id = "ADA-USD.CHAINLINK"

[reference_current_price.source.polyresearch_backup]
provider = "polyresearch_ws"
enabled = true
required = false
client_id = "polyresearch_reference"
symbol = "ADA/USD"
"#,
        "ADA",
    );

    assert!(
        !messages.iter().any(|message| {
            message.contains("reference_current_price.source.polyresearch_backup")
                && message.contains("ADA")
                && message.contains("polyresearch_ws")
                && message.contains("unsupported")
        }),
        "polyresearch_ws asset support should come from the configured symbol, got: {messages:#?}"
    );
    assert!(
        !messages.iter().any(|message| {
            message.contains("reference_current_price.min_valid_sources")
                && message.contains("cannot be met")
        }),
        "supported source quorum should still be considered met, got: {messages:#?}"
    );
}

#[test]
fn required_polyresearch_source_for_unlisted_asset_validates_when_symbol_matches() {
    let messages = validate_reference_current_price_for_asset(
        r#"
[reference_current_price]
asset = "ADA"
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
symbol = "ADA/USD"
"#,
        "ADA",
    );

    assert!(
        messages.is_empty(),
        "required polyresearch_ws source should validate when its symbol matches the configured asset, got: {messages:#?}"
    );
}
