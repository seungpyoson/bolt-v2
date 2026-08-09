use crate::{
    bolt_v3_economics_runtime::{
        AuthoritativeEconomicsInputStore, AuthoritativeValuationObservation,
        bind_execution_economics,
    },
    bolt_v3_order_execution::BoltV3OrderEconomicsHandle,
    bolt_v3_providers::polymarket::{
        PolymarketMarketInfoSnapshot, PolymarketSnapshotMetadata, authoritative_economics_input,
    },
    economics::{CurrencyId, SnapshotId, SourceIdentity},
};

pub(crate) fn fixture_order_economics() -> BoltV3OrderEconomicsHandle {
    fixture_order_economics_for("POLYMARKET")
}

pub(crate) fn fixture_order_economics_for(execution_client_id: &str) -> BoltV3OrderEconomicsHandle {
    let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
        "tests/fixtures/bolt_v3/root.toml",
    ))
    .expect("bolt-v3 fixture root should load for economics authority");
    let client = loaded
        .root
        .clients
        .get("polymarket_main")
        .cloned()
        .expect("fixture should declare the Polymarket execution client");
    loaded
        .root
        .clients
        .insert(execution_client_id.to_string(), client);

    let snapshot = PolymarketMarketInfoSnapshot::from_json(
        PolymarketSnapshotMetadata {
            snapshot_id: SnapshotId::try_new("edge-taker-fixture-market-info".to_string())
                .expect("fixture snapshot id should be canonical"),
            source_at_ns: 0,
            fetched_at_ns: 0,
            valid_until_ns: u64::MAX,
            builder_attachment_id: None,
        },
        r#"{
            "r": {},
            "t": [
                { "t": "condition-MKT-1-MKT-1-UP", "o": "Up" },
                { "t": "condition-MKT-1-MKT-1-DOWN", "o": "Down" },
                { "t": "condition-MKT-2-MKT-2-UP", "o": "Up" },
                { "t": "condition-MKT-2-MKT-2-DOWN", "o": "Down" }
            ],
            "c": "fixture-condition",
            "mos": 5,
            "mts": 0.01,
            "mbf": 50,
            "tbf": 100,
            "fd": { "r": 0.03, "e": 1, "to": true }
        }"#,
    )
    .expect("fixture market-info snapshot should parse");

    let valuation = || AuthoritativeValuationObservation::ProviderExactConversion {
        source_id: SourceIdentity::try_new("fixture-collateral".to_string())
            .expect("fixture valuation source should be canonical"),
        from_unit: CurrencyId::try_new("pUSD".to_string())
            .expect("fixture collateral currency should be canonical"),
        to_unit: CurrencyId::try_new("USD".to_string())
            .expect("fixture reporting currency should be canonical"),
        snapshot_id: SnapshotId::try_new("edge-taker-fixture-collateral-conversion".to_string())
            .expect("fixture valuation snapshot should be canonical"),
        observed_at_ns: 0,
        fetched_at_ns: 0,
        valid_until_ns: u64::MAX,
    };
    let instruments = [
        (
            "condition-MKT-1-MKT-1-UP.POLYMARKET",
            "condition-MKT-1-MKT-1-UP",
        ),
        (
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            "condition-MKT-1-MKT-1-DOWN",
        ),
        (
            "condition-MKT-2-MKT-2-UP.POLYMARKET",
            "condition-MKT-2-MKT-2-UP",
        ),
        (
            "condition-MKT-2-MKT-2-DOWN.POLYMARKET",
            "condition-MKT-2-MKT-2-DOWN",
        ),
        ("YES.RUNTIME", "condition-MKT-1-MKT-1-UP"),
        ("NO.RUNTIME", "condition-MKT-1-MKT-1-DOWN"),
        ("YES.INSTRUMENT", "condition-MKT-1-MKT-1-UP"),
        ("NO.INSTRUMENT", "condition-MKT-1-MKT-1-DOWN"),
        ("INSTRUMENT.SOURCE", "condition-MKT-1-MKT-1-UP"),
        ("instrument-yes.VENUE-A", "condition-MKT-1-MKT-1-UP"),
    ];
    let inputs = instruments
        .into_iter()
        .map(|(instrument_id, provider_instrument_id)| {
            authoritative_economics_input(
                execution_client_id,
                instrument_id,
                "binary_outcome",
                provider_instrument_id,
                snapshot.clone(),
            )
            .expect("fixture economics scope should match its market-info snapshot")
            .with_valuation_observations([valuation()])
        });
    let inputs = AuthoritativeEconomicsInputStore::try_new(inputs)
        .expect("fixture economics scopes should be unique");
    let bound = bind_execution_economics(&loaded, execution_client_id, &inputs)
        .expect("fixture execution economics should bind");
    BoltV3OrderEconomicsHandle::new(bound)
}
