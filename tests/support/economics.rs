use bolt_v2::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_economics_runtime::AuthoritativeEconomicsInputStore,
    bolt_v3_providers::polymarket::{
        PolymarketMarketInfoSnapshot, PolymarketSnapshotMetadata, authoritative_economics_input,
    },
    economics::SnapshotId,
};

pub fn polymarket_inputs(loaded: &LoadedBoltV3Config) -> AuthoritativeEconomicsInputStore {
    let client = loaded
        .root
        .clients
        .get("polymarket_main")
        .expect("fixture Polymarket execution client should exist");
    client
        .execution
        .as_ref()
        .expect("fixture Polymarket execution block should exist");
    let snapshot = PolymarketMarketInfoSnapshot::from_json(
        PolymarketSnapshotMetadata {
            snapshot_id: SnapshotId::try_new("fixture-market-info")
                .expect("fixture snapshot id should be valid"),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 2_000,
            builder_attachment_id: None,
        },
        include_str!("../fixtures/economics/polymarket/fee_free.json"),
    )
    .expect("fixture market-info snapshot should parse");
    AuthoritativeEconomicsInputStore::try_new([authoritative_economics_input(
        "polymarket_main",
        "token-yes",
        "binary_outcome",
        snapshot,
    )
    .expect("fixture authority should match its token scope")])
    .expect("one fixture economics input should construct")
}
