use bolt_v2::bolt_v3_config::ReferencePriceProvider;
use bolt_v2::bolt_v3_providers::{
    ReferencePriceIdentifierKind, chainlink_reference, polyresearch,
    reference_price_provider_metadata,
};
use bolt_v2::bolt_v3_reference_price::{ReferencePriceSelector, ReferenceQuote};

fn provider(key: &str) -> ReferencePriceProvider {
    ReferencePriceProvider::new(key).expect("test provider key should be valid")
}

fn quote(source_id: &str, price: f64, observed_ts_ms: u64) -> ReferenceQuote {
    ReferenceQuote::try_new(
        "BTC",
        source_id,
        provider(chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY),
        price,
        None,
        None,
        observed_ts_ms,
        observed_ts_ms,
    )
    .expect("test quote should be valid")
}

#[test]
fn reference_price_provider_metadata_uses_provider_owned_keys() {
    let chainlink =
        reference_price_provider_metadata(chainlink_reference::REFERENCE_PRICE_PROVIDER_KEY)
            .expect("chainlink reference metadata should be registered");
    assert_eq!(chainlink.provider_key, "chainlink_ws");
    assert_eq!(
        chainlink.identifier_kind,
        ReferencePriceIdentifierKind::InstrumentId
    );

    let polyresearch =
        reference_price_provider_metadata(polyresearch::REFERENCE_PRICE_PROVIDER_KEY)
            .expect("polyresearch reference metadata should be registered");
    assert_eq!(polyresearch.provider_key, "polyresearch_ws");
    assert_eq!(
        polyresearch.identifier_kind,
        ReferencePriceIdentifierKind::Symbol
    );
    assert_eq!(
        polyresearch.supported_assets,
        ["BTC", "ETH", "SOL", "BNB", "XRP", "DOGE", "HYPE"]
    );
}

#[test]
fn reference_price_selector_uses_first_valid_source_for_interval() {
    let mut selector = ReferencePriceSelector::new(
        "BTC",
        ["primary".to_string(), "backup".to_string()],
        1,
        1500,
        10,
    )
    .expect("selector config should be valid");

    let selection = selector
        .select(
            1774672089000,
            1774672389000,
            1774672089100,
            &[
                quote("backup", 66301.0, 1774672089050),
                quote("primary", 66300.0, 1774672089050),
            ],
        )
        .expect("first configured valid source should select");

    assert_eq!(selection.source_id(), "primary");
    assert_eq!(selection.price(), 66300.0);
    assert!(!selection.failed_over());
}
