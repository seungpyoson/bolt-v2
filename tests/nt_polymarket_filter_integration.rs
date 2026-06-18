use std::sync::Arc;

use bolt_v2::bolt_v3_market_families::updown::{updown_market_slug, updown_period_pair};
use nautilus_network::retry::RetryConfig;
use nautilus_polymarket::{
    filters::MarketSlugFilter, http::gamma::PolymarketGammaHttpClient,
    providers::PolymarketInstrumentProvider,
};

// Pin-surface test: drive the production slug formatter so a future
// change to `updown_market_slug` or `updown_period_pair` fails this
// test loudly. The helper takes `cadence_slug_token` explicitly so the
// caller — not the helper — owns the TOML-derived token (T060).
fn first_live_updown_slugs(
    underlying_asset: &str,
    cadence_seconds: i64,
    cadence_slug_token: &str,
    now_unix_seconds: i64,
) -> Vec<String> {
    let (current_start, next_start) = updown_period_pair(cadence_seconds, now_unix_seconds)
        .expect("valid cadence and non-negative now_unix_seconds");
    vec![
        updown_market_slug(underlying_asset, cadence_slug_token, current_start),
        updown_market_slug(underlying_asset, cadence_slug_token, next_start),
    ]
}

#[test]
fn first_live_updown_slug_rule_matches_expected_shape() {
    let slugs = first_live_updown_slugs("CONFIGURED_ASSET", 300, "configuredwindow", 1_800);
    assert_eq!(
        slugs,
        vec![
            "configured_asset-updown-configuredwindow-1800".to_string(),
            "configured_asset-updown-configuredwindow-2100".to_string(),
        ]
    );
}

#[test]
fn pinned_polymarket_provider_accepts_market_slug_filters() {
    let slugs = first_live_updown_slugs("CONFIGURED_ASSET", 300, "configuredwindow", 1_800);
    let filter = MarketSlugFilter::from_slugs(slugs);
    let http_client = PolymarketGammaHttpClient::new(
        Some("https://gamma.test.invalid".to_string()),
        60,
        RetryConfig::default(),
    )
    .unwrap();
    let provider = PolymarketInstrumentProvider::with_filter(http_client, None, Arc::new(filter));

    assert_eq!(provider.filters().len(), 1);
}
