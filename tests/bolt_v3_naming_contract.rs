mod support;

#[test]
fn bolt_v3_schema_doc_uses_nt_client_and_venue_terms_for_config_surface() {
    let schema = std::fs::read_to_string(support::repo_path(
        "docs/bolt-v3/2026-04-25-bolt-v3-schema.md",
    ))
    .expect("bolt-v3 schema doc should be readable");

    for required in [
        "[clients.polymarket_main]",
        "venue = \"POLYMARKET\"",
        "execution_client_id = \"polymarket_main\"",
        "data_client_id = \"binance_reference\"",
        "### `[clients.<client_id>]`",
    ] {
        assert!(
            schema.contains(required),
            "schema doc must contain current bolt-v3 config term `{required}`"
        );
    }

    for forbidden in [
        "[adapter_instances.polymarket_main]",
        "adapter_venue = \"polymarket\"",
        "adapter_instance = \"polymarket_main\"",
        "[venues.polymarket_main]",
        "kind = \"polymarket\"",
        "venue = \"polymarket_main\"",
        "venue = \"polymarket\"",
        "\nclient_id = \"polymarket_main\"",
        "### `[venues.<identifier>]`",
        "### `[adapter_instances.<identifier>]`",
        "one-venue-per-kind",
    ] {
        assert!(
            !schema.contains(forbidden),
            "schema doc must not contain legacy bolt-v3 config term `{forbidden}`"
        );
    }
}

#[test]
fn bolt_v3_runtime_contract_uses_nt_client_and_venue_terms_for_execution_leg() {
    let contract = std::fs::read_to_string(support::repo_path(
        "docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md",
    ))
    .expect("bolt-v3 runtime contract should be readable");

    for required in ["client_id", "venue"] {
        assert!(
            contract.contains(required),
            "runtime contract must contain current bolt-v3 field `{required}`"
        );
    }

    for forbidden in [
        "adapter_instance",
        "adapter_venue",
        "venue_config_key",
        "venue_kind",
    ] {
        assert!(
            !contract.contains(forbidden),
            "runtime contract must not contain legacy bolt-v3 field `{forbidden}`"
        );
    }
}

#[test]
fn bolt_v3_internal_names_use_nt_client_and_venue_terms_for_config_routing() {
    for path in [
        "src/bolt_v3_adapters.rs",
        "src/bolt_v3_market_families/updown.rs",
        "src/bolt_v3_providers/polymarket.rs",
        "src/bolt_v3_config.rs",
        "src/bolt_v3_client_registration.rs",
    ] {
        let source = std::fs::read_to_string(support::repo_path(path))
            .expect("bolt-v3 source file should be readable");
        for forbidden in [
            "venue_target_refs",
            "validate_market_identity_target_venues",
            "build_market_slug_filters_for_venue",
            "ResolvedVenueSecrets",
            "SecretKindMismatch",
            "provider_key",
            "adapter_instance",
            "adapter_instances",
            "AdapterInstance",
            "adapter_venue",
            "AdapterVenue",
        ] {
            assert!(
                !source.contains(forbidden),
                "{path} must not contain legacy config-routing name `{forbidden}`"
            );
        }
    }
}
