use backtesting_vertical_slice::seeded_level_set_deltas::{
    OrderCountPolicy, SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY, SeededLevelSetMappingConfig,
    SourceSequencePolicy,
};

#[test]
fn seeded_level_set_mapping_is_toml_driven_without_a_venue_discriminator() {
    let mapping: SeededLevelSetMappingConfig = toml::from_str(
        r#"
record_identity_path = ["data", "symbol"]
action_path = ["type"]
event_time_path = ["timestamp"]
event_time_unit = "milliseconds"
bids_path = ["data", "bids"]
asks_path = ["data", "asks"]
level_arity = 3
level_price_index = 0
level_size_index = 1
snapshot_action_values = ["snapshot"]
update_action_values = ["delta"]

[order_count]
kind = "validate_non_negative_and_drop"
index = 2

[source_sequence]
kind = "native"
path = ["data", "sequence"]

[output]
max_levels_per_event = 1000
max_active_levels_per_side = 500
max_selected_events = 10000
max_selected_delta_rows = 1000000
max_emitted_bytes = 1073741824
max_published_bytes = 2147483648
"#,
    )
    .expect("generic seeded level-set mapping parses");

    assert!(!SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY.is_empty());
    assert_eq!(
        mapping.record_identity_path,
        ["data".to_string(), "symbol".to_string()]
    );
    assert_eq!(
        mapping.source_sequence,
        SourceSequencePolicy::Native {
            path: vec!["data".to_string(), "sequence".to_string()]
        }
    );
    assert_eq!(
        mapping.order_count,
        OrderCountPolicy::ValidateNonNegativeAndDrop { index: 2 }
    );
}
