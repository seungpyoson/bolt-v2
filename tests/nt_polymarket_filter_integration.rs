use std::sync::Arc;

use nautilus_network::retry::RetryConfig;
use nautilus_polymarket::{
    filters::MarketSlugFilter, http::gamma::PolymarketGammaHttpClient,
    providers::PolymarketInstrumentProvider,
};

// This is a pin-surface test, not a bolt-v3 runtime implementation.
// The documented slug contract is mirrored here only to prove the pinned NT filter accepts it.
fn first_live_updown_slugs(
    underlying_asset: &str,
    cadence_seconds: u64,
    now_unix_seconds: u64,
) -> Vec<String> {
    let period_start = (now_unix_seconds / cadence_seconds) * cadence_seconds;
    let next_period_start = period_start + cadence_seconds;
    let cadence_minutes = cadence_seconds / 60;
    let asset = underlying_asset.to_lowercase();

    vec![
        format!("{asset}-updown-{cadence_minutes}m-{period_start}"),
        format!("{asset}-updown-{cadence_minutes}m-{next_period_start}"),
    ]
}

#[test]
fn first_live_updown_slug_rule_matches_expected_shape() {
    let slugs = first_live_updown_slugs("BTC", 300, 1_800);
    assert_eq!(
        slugs,
        vec![
            "btc-updown-5m-1800".to_string(),
            "btc-updown-5m-2100".to_string(),
        ]
    );
}

#[test]
fn pinned_polymarket_provider_accepts_market_slug_filters() {
    let slugs = first_live_updown_slugs("BTC", 300, 1_800);
    let filter = MarketSlugFilter::from_slugs(slugs);
    let http_client = PolymarketGammaHttpClient::new(None, 60, RetryConfig::default()).unwrap();
    let provider = PolymarketInstrumentProvider::with_filter(http_client, Arc::new(filter));

    assert_eq!(provider.filters().len(), 1);
}
