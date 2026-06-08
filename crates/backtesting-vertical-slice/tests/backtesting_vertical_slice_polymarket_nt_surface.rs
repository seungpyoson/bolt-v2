use backtesting_vertical_slice::polymarket_nt_surface_proof::{
    PolymarketNtSurface, prove_polymarket_nt_public_surfaces,
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
