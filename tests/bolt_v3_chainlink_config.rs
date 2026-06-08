//! Config-parse tests for the `CHAINLINK_DATA_STREAMS` provider binding.
//!
//! These guard the product boundary that the per-provider module
//! `bolt_v3_providers::chainlink` owns the concrete shape of a
//! `[clients.<id>.data]` Chainlink Data Streams strike block and its
//! `feed_bindings`, including:
//!   1. A well-formed `[data]` block (with `feed_bindings`) parses into the
//!      provider `ChainlinkDataConfig` and validates clean.
//!   2. Unknown / missing fields are rejected (`deny_unknown_fields` on the
//!      block, plus per-feed-binding field requirements through
//!      `validate_client`).
//!
//! Out of scope: live fetch, secret resolution, and NT registration — those are
//! covered by `tests/bolt_v3_chainlink_registration.rs` and the in-crate decode
//! / auth / strike-mapping unit tests.

use bolt_v2::{
    bolt_v3_config::{BoltV3RootConfig, ClientBlock},
    bolt_v3_providers::chainlink::{ChainlinkDataConfig, validate_client},
    bolt_v3_validate::validate_root_only,
};

const TEST_FEED_ID: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";

fn client_from_toml(value: &str) -> ClientBlock {
    toml::from_str(value).expect("chainlink test client block should parse")
}

fn well_formed_client_toml() -> String {
    format!(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5

[[data.feed_bindings]]
feed_id = "{TEST_FEED_ID}"
instrument_id = "BTC-USD-UP.BOLT"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2

[secrets]
api_key_ssm_parameter = "/bolt/chainlink_strike/api_key"
api_secret_ssm_parameter = "/bolt/chainlink_strike/api_secret"
"#
    )
}

fn fixture_root_with_client_feed_bindings_moved_to_shared_catalog() -> String {
    let mut root_value: toml::Value = toml::from_str(include_str!("fixtures/bolt_v3/root.toml"))
        .expect("fixture root TOML should parse as generic TOML");
    let root = root_value
        .as_table_mut()
        .expect("fixture root must be a TOML table");
    let feed_bindings = root
        .get_mut("clients")
        .and_then(toml::Value::as_table_mut)
        .and_then(|clients| clients.get_mut("chainlink_strike"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|client| client.get_mut("data"))
        .and_then(toml::Value::as_table_mut)
        .and_then(|data| data.remove("feed_bindings"))
        .expect("fixture must declare client-local chainlink feed_bindings");

    let mut catalog = toml::map::Map::new();
    catalog.insert("feed_bindings".to_string(), feed_bindings);
    root.insert(
        "chainlink_data_streams".to_string(),
        toml::Value::Table(catalog),
    );

    toml::to_string(&root_value).expect("mutated fixture should serialize")
}

#[test]
fn chainlink_reference_current_price_uses_shared_feed_catalog() {
    let root_toml = fixture_root_with_client_feed_bindings_moved_to_shared_catalog();
    let root: BoltV3RootConfig =
        toml::from_str(&root_toml).expect("root-owned Chainlink feed catalog should parse");

    let errors = validate_root_only(&root);

    assert!(
        errors.is_empty(),
        "root-owned Chainlink feed catalog should validate cleanly: {errors:?}"
    );
}

#[test]
fn well_formed_chainlink_data_block_parses_into_provider_config() {
    let client = client_from_toml(&well_formed_client_toml());
    let data = client
        .data
        .as_ref()
        .expect("fixture declares a [data] block");
    let parsed: ChainlinkDataConfig = data
        .clone()
        .try_into()
        .expect("well-formed chainlink [data] block must parse into ChainlinkDataConfig");

    assert_eq!(parsed.rest_base_url, "https://api.example.com/");
    assert_eq!(parsed.report_endpoint_path, "/api/v1/reports/bulk");
    assert_eq!(parsed.http_timeout_secs, 5);
    assert_eq!(
        parsed.feed_bindings.len(),
        1,
        "the single declared feed binding must be retained"
    );

    assert!(
        validate_client("chainlink_strike", &client).is_empty(),
        "a well-formed chainlink client must validate clean: {:?}",
        validate_client("chainlink_strike", &client)
    );
}

#[test]
fn unknown_data_field_is_rejected() {
    let client = client_from_toml(&format!(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5
not_a_real_field = true

[[data.feed_bindings]]
feed_id = "{TEST_FEED_ID}"
instrument_id = "BTC-USD-UP.BOLT"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2
"#
    ));
    let errors = validate_client("chainlink_strike", &client);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("not_a_real_field") || error.contains("unknown field")),
        "an unknown [data] field must be rejected by deny_unknown_fields; got {errors:?}"
    );
}

#[test]
fn missing_feed_bindings_field_is_rejected() {
    let client = client_from_toml(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5
"#,
    );
    let errors = validate_client("chainlink_strike", &client);
    assert!(
        !errors.is_empty(),
        "a [data] block missing the required feed_bindings array must be rejected"
    );
}

#[test]
fn feed_binding_missing_required_field_is_rejected() {
    // feed_bindings entry without `instrument_id` must fail per-binding parse.
    let client = client_from_toml(&format!(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5

[[data.feed_bindings]]
feed_id = "{TEST_FEED_ID}"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2
"#
    ));
    let errors = validate_client("chainlink_strike", &client);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("feed_bindings[0].instrument_id")),
        "a feed binding missing instrument_id must be rejected; got {errors:?}"
    );
}

#[test]
fn feed_binding_with_malformed_feed_id_is_rejected() {
    let client = client_from_toml(
        r#"
venue = "CHAINLINK_DATA_STREAMS"

[data]
rest_base_url = "https://api.example.com/"
report_endpoint_path = "/api/v1/reports/bulk"
http_timeout_secs = 5

[[data.feed_bindings]]
feed_id = "0xNOT_LOWERCASE_HEX"
instrument_id = "BTC-USD-UP.BOLT"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2
"#,
    );
    let errors = validate_client("chainlink_strike", &client);
    assert!(
        errors
            .iter()
            .any(|error| error.contains("feed_bindings[0].feed_id")),
        "a malformed feed_id must be rejected; got {errors:?}"
    );
}
