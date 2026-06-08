use backtesting_vertical_slice::polymarket_nt_surface_proof::{
    BacktestInstrumentEpochReplaySupport, PolymarketNtSurface,
    prove_polymarket_dynamic_instrument_epoch_surfaces, prove_polymarket_nt_public_surfaces,
};

#[test]
fn bte_uses_nt_polymarket_public_surfaces_for_one_off_pmxt_projection() {
    let proof = prove_polymarket_nt_public_surfaces();

    assert!(
        proof
            .provider_type
            .ends_with("PolymarketInstrumentProvider")
    );
    assert_eq!(
        proof.surfaces,
        vec![
            PolymarketNtSurface::InstrumentProviderTokenMap,
            PolymarketNtSurface::GammaQueryBuilder,
            PolymarketNtSurface::WebsocketBookSnapshotParser,
            PolymarketNtSurface::WebsocketBookDeltaParser,
            PolymarketNtSurface::WebsocketTradeParser,
            PolymarketNtSurface::WebsocketQuoteParser,
        ]
    );
}

#[test]
fn bte_records_nt_dynamic_tick_size_backtest_surface_boundary() {
    let proof = prove_polymarket_dynamic_instrument_epoch_surfaces();

    assert!(proof.live_tick_size_change_message_supported);
    assert!(proof.live_tick_size_rebuilds_instrument);
    assert!(proof.catalog_instrument_snapshot_storage_supported);
    assert!(proof.catalog_auxiliary_status_close_streams_supported);
    assert!(!proof.backtest_data_config_instrument_definition_stream_supported);
    assert_eq!(
        proof.backtest_instrument_epoch_replay_support,
        BacktestInstrumentEpochReplaySupport::StaticCatalogInstrumentLoadOnly
    );
    assert!(
        proof
            .nt_evidence_refs
            .iter()
            .any(|reference| reference.contains("backtest/src/node.rs:165"))
    );
    assert!(
        proof
            .nt_evidence_refs
            .iter()
            .any(|reference| reference.contains("model/src/data/mod.rs:97"))
    );
}
