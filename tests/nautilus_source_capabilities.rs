use bolt_v2::{
    bolt_v3_config::BoltV3RootConfig, nautilus_source_capabilities::NAUTILUS_SOURCE_CAPABILITIES,
};

const OFFICIAL_NAUTILUS_REVISION: &str = "df0f083ceca6077c7f8b0c9e728ac8304709ffaf";

#[test]
fn generated_registry_exposes_the_exact_official_source_facts() {
    assert_eq!(
        NAUTILUS_SOURCE_CAPABILITIES.revision,
        OFFICIAL_NAUTILUS_REVISION
    );
    assert!(NAUTILUS_SOURCE_CAPABILITIES.binance_spot_sbe_schema_3_5);
    assert!(NAUTILUS_SOURCE_CAPABILITIES.binance_adapter_receive_timestamps);
    assert!(NAUTILUS_SOURCE_CAPABILITIES.polymarket_reconciliation_rejects_unmapped_open_orders);
    assert!(
        NAUTILUS_SOURCE_CAPABILITIES.polymarket_reconciliation_rejects_unmapped_confirmed_fills
    );
    assert!(
        NAUTILUS_SOURCE_CAPABILITIES.polymarket_reconciliation_rejects_unrepresentable_positions
    );
}

#[test]
fn operator_toml_cannot_override_nautilus_source_capabilities() {
    let mut source = std::fs::read_to_string("config/root.toml")
        .expect("tracked production root config should exist");
    source.push_str(
        "\n[nautilus_source_capabilities]\n\
         binance_spot_sbe_schema_3_5 = true\n\
         binance_adapter_receive_timestamps = true\n",
    );

    let error = toml::from_str::<BoltV3RootConfig>(&source)
        .expect_err("operator source-capability overrides must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("unknown field") && rendered.contains("nautilus_source_capabilities"),
        "unexpected parse error: {rendered}"
    );
}
