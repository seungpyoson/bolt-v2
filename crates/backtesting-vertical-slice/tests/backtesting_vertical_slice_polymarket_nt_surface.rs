use backtesting_vertical_slice::polymarket_nt_surface_proof::{
    BacktestInstrumentEpochReplaySupport, PolymarketNtSurface,
    prove_polymarket_dynamic_instrument_epoch_surfaces, prove_polymarket_nt_public_surfaces,
};
use nautilus_backtest::config::NautilusDataType;

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

    assert!(
        proof
            .live_tick_size_change_message_type
            .ends_with("MarketWsMessage")
    );
    assert!(
        proof
            .live_tick_size_rebuilder_type
            .contains("InstrumentAny")
    );
    assert!(
        proof
            .catalog_instrument_snapshot_type
            .ends_with("InstrumentAny")
    );
    assert_eq!(
        proof.catalog_auxiliary_status_close_data_types,
        [
            NautilusDataType::InstrumentStatus,
            NautilusDataType::InstrumentClose
        ]
    );
    assert!(!proof.backtest_data_config_instrument_definition_stream_supported);
    assert_eq!(
        proof.backtest_instrument_epoch_replay_support,
        BacktestInstrumentEpochReplaySupport::StaticCatalogInstrumentLoadOnly
    );
}
