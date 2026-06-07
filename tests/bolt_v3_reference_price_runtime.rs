use std::collections::BTreeMap;

use bolt_v2::bolt_v3_config::{ReferencePriceDriftPolicy, ReferencePriceProvider};
use bolt_v2::bolt_v3_reference_price::{
    ReferencePriceSelection, ReferencePriceSelector, ReferencePriceSourceSpec,
    ReferencePriceUpdate, ReferenceQuote, ReferenceQuoteProvenance,
};

const CHAINLINK_REFERENCE_PROVIDER: &str = "chainlink_ws";

fn reference_provider(key: &str) -> ReferencePriceProvider {
    ReferencePriceProvider::new(key).expect("test provider key should be valid")
}

fn quote(source_id: &str, price: f64, observed_ts_ms: u64, received_ts_ms: u64) -> ReferenceQuote {
    ReferenceQuote::try_new(
        "BTC",
        source_id,
        reference_provider(CHAINLINK_REFERENCE_PROVIDER),
        price,
        None,
        None,
        observed_ts_ms,
        received_ts_ms,
    )
    .expect("test quote should be valid")
}

#[test]
fn reference_quote_accepts_positive_price_with_optional_spread() {
    let provenance = provenance([("frame_type", "ticker"), ("sequence", "12345")]);
    let quote = ReferenceQuote::try_new(
        "BTC",
        "chainlink_primary",
        reference_provider(CHAINLINK_REFERENCE_PROVIDER),
        66300.25,
        Some(66299.0),
        Some(66301.0),
        1774672089000,
        1774672089123,
    )
    .expect("valid quote should construct");

    assert_eq!(quote.asset(), "BTC");
    assert_eq!(quote.source_id(), "chainlink_primary");
    assert_eq!(quote.provider().as_str(), CHAINLINK_REFERENCE_PROVIDER);
    assert_eq!(quote.price(), 66300.25);
    assert_eq!(quote.bid(), Some(66299.0));
    assert_eq!(quote.ask(), Some(66301.0));
    assert_eq!(quote.observed_ts_ms(), 1774672089000);
    assert_eq!(quote.received_ts_ms(), 1774672089123);
    assert_eq!(quote.provider_instrument(), "chainlink_primary");
    assert!(quote.provenance().fields().is_empty());

    let quote = ReferenceQuote::try_new_with_provenance(
        "BTC",
        "chainlink_primary",
        reference_provider(CHAINLINK_REFERENCE_PROVIDER),
        "BTC-USD.CHAINLINK_REFERENCE",
        66300.25,
        Some(66299.0),
        Some(66301.0),
        1774672089000,
        1774672089123,
        provenance.clone(),
    )
    .expect("valid quote with provenance should construct");

    assert_eq!(quote.provider_instrument(), "BTC-USD.CHAINLINK_REFERENCE");
    assert_eq!(quote.provenance(), &provenance);
}

#[test]
fn reference_quote_rejects_non_positive_price() {
    let error = ReferenceQuote::try_new(
        "BTC",
        "chainlink_primary",
        reference_provider(CHAINLINK_REFERENCE_PROVIDER),
        0.0,
        None,
        None,
        1774672089000,
        1774672089123,
    )
    .expect_err("non-positive reference price must be rejected");

    assert!(
        error.contains("price"),
        "invalid quote error should identify price, got: {error}"
    );
}

#[test]
fn selector_fails_over_once_without_flipback_until_next_interval() {
    let mut selector = ReferencePriceSelector::new(
        "BTC",
        [
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        1,
        2000,
        25,
    )
    .expect("selector config should be valid");

    let first = selector
        .select(
            1774672089000,
            1774672389000,
            1774672089500,
            &[
                quote("chainlink_primary", 66300.25, 1774672089200, 1774672089300),
                quote(
                    "polyresearch_backup",
                    66301.00,
                    1774672089200,
                    1774672089300,
                ),
            ],
        )
        .expect("first interval selection should choose a source");
    assert_eq!(
        first,
        ReferencePriceSelection::selected("chainlink_primary", 66300.25, false)
    );

    let failed_over = selector
        .select(
            1774672089000,
            1774672389000,
            1774672092500,
            &[quote(
                "polyresearch_backup",
                66302.00,
                1774672092400,
                1774672092450,
            )],
        )
        .expect("stale primary should fail over to backup");
    assert_eq!(
        failed_over,
        ReferencePriceSelection::selected("polyresearch_backup", 66302.00, true)
    );

    let no_flipback = selector
        .select(
            1774672089000,
            1774672389000,
            1774672093000,
            &[
                quote("chainlink_primary", 66305.00, 1774672092950, 1774672092960),
                quote(
                    "polyresearch_backup",
                    66302.50,
                    1774672092950,
                    1774672092960,
                ),
            ],
        )
        .expect("winner should remain backup for the interval");
    assert_eq!(
        no_flipback,
        ReferencePriceSelection::selected("polyresearch_backup", 66302.50, true)
    );

    let next_interval = selector
        .select(
            1774672389000,
            1774672689000,
            1774672389500,
            &[
                quote("chainlink_primary", 66310.00, 1774672389400, 1774672389450),
                quote(
                    "polyresearch_backup",
                    66311.00,
                    1774672389400,
                    1774672389450,
                ),
            ],
        )
        .expect("next interval should reset source selection");
    assert_eq!(
        next_interval,
        ReferencePriceSelection::selected("chainlink_primary", 66310.00, false)
    );
}

#[test]
fn selector_requires_min_valid_sources_before_selection() {
    let mut selector = ReferencePriceSelector::new(
        "BTC",
        [
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        2,
        2000,
        25,
    )
    .expect("selector config should be valid");

    let selected = selector.select(
        1774672089000,
        1774672389000,
        1774672089500,
        &[quote(
            "chainlink_primary",
            66300.25,
            1774672089200,
            1774672089300,
        )],
    );

    assert_eq!(selected, None);
}

#[test]
fn selector_required_source_unavailable_blocks_selection() {
    let mut selector = ReferencePriceSelector::new_with_source_specs(
        "BTC",
        [
            ReferencePriceSourceSpec::required("chainlink_primary"),
            ReferencePriceSourceSpec::optional("polyresearch_backup"),
        ],
        1,
        2000,
        25,
    )
    .expect("selector config should be valid");

    let selected = selector.select(
        1774672089000,
        1774672389000,
        1774672089500,
        &[quote(
            "polyresearch_backup",
            66300.25,
            1774672089200,
            1774672089300,
        )],
    );

    assert_eq!(selected, None);
}

#[test]
fn selector_optional_source_unavailable_does_not_block_when_quorum_remains() {
    let mut selector = ReferencePriceSelector::new_with_source_specs(
        "BTC",
        [
            ReferencePriceSourceSpec::optional("chainlink_primary"),
            ReferencePriceSourceSpec::optional("polyresearch_backup"),
        ],
        1,
        2000,
        25,
    )
    .expect("selector config should be valid");

    let selected = selector
        .select(
            1774672089000,
            1774672389000,
            1774672089500,
            &[quote(
                "polyresearch_backup",
                66300.25,
                1774672089200,
                1774672089300,
            )],
        )
        .expect("optional missing source should not block while quorum remains");

    assert_eq!(
        selected,
        ReferencePriceSelection::selected("polyresearch_backup", 66300.25, false)
    );
}

#[test]
fn selector_rejects_quote_observed_after_interval_end() {
    let mut selector =
        ReferencePriceSelector::new("BTC", ["chainlink_primary".to_string()], 1, 2000, 25)
            .expect("selector config should be valid");

    let selected = selector.select(
        1774672089000,
        1774672389000,
        1774672389100,
        &[quote(
            "chainlink_primary",
            66300.25,
            1774672389001,
            1774672389050,
        )],
    );

    assert_eq!(selected, None);
}

#[test]
fn selector_observes_cross_source_drift_without_blocking_by_default() {
    let mut selector = ReferencePriceSelector::new(
        "BTC",
        [
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        2,
        2000,
        25,
    )
    .expect("selector config should be valid");

    let selected = selector.select(
        1774672089000,
        1774672389000,
        1774672089500,
        &[
            quote("chainlink_primary", 100.0, 1774672089200, 1774672089300),
            quote("polyresearch_backup", 101.0, 1774672089200, 1774672089300),
        ],
    );

    assert_eq!(
        selected,
        Some(ReferencePriceSelection::selected(
            "chainlink_primary",
            100.0,
            false
        ))
    );
    assert_eq!(selector.last_cross_source_drift_bps(), Some(100.0));
}

#[test]
fn selector_rejects_cross_source_drift_above_threshold_when_policy_blocks() {
    let mut selector = ReferencePriceSelector::new_with_drift_policy(
        "BTC",
        [
            "chainlink_primary".to_string(),
            "polyresearch_backup".to_string(),
        ],
        2,
        2000,
        25,
        ReferencePriceDriftPolicy::Block,
    )
    .expect("selector config should be valid");

    let selected = selector.select(
        1774672089000,
        1774672389000,
        1774672089500,
        &[
            quote("chainlink_primary", 100.0, 1774672089200, 1774672089300),
            quote("polyresearch_backup", 101.0, 1774672089200, 1774672089300),
        ],
    );

    assert_eq!(selected, None);
    assert_eq!(selector.last_cross_source_drift_bps(), Some(100.0));
}

#[test]
fn reference_price_update_data_type_includes_asset_source_and_provider() {
    let data_type = ReferencePriceUpdate::data_type_for("BTC", "chainlink_primary", "chainlink_ws")
        .expect("valid source identity should produce a custom data type");

    assert_eq!(data_type.type_name(), "BoltV3ReferencePriceUpdate");
    assert_eq!(data_type.identifier(), Some("BTC"));
    let metadata = data_type
        .metadata()
        .expect("reference price data type should carry metadata");
    assert_eq!(metadata.get_str("asset"), Some("BTC"));
    assert_eq!(metadata.get_str("source_key"), Some("chainlink_primary"));
    assert_eq!(metadata.get_str("provider"), Some("chainlink_ws"));
}

#[test]
fn reference_price_update_round_trips_through_custom_data() {
    let provenance = provenance([("frame_type", "ticker"), ("sequence", "12345")]);
    let update = ReferencePriceUpdate::try_new_with_provenance(
        "BTC",
        "chainlink_primary",
        "chainlink_ws",
        "BTC-USD.CHAINLINK_REFERENCE",
        66300.25,
        Some(66299.0),
        Some(66301.0),
        1774672089200,
        1774672089300,
        provenance.clone(),
    )
    .expect("valid reference price update should construct");

    let custom = update.to_custom_data();
    assert_eq!(
        custom.data_type,
        ReferencePriceUpdate::data_type_for("BTC", "chainlink_primary", "chainlink_ws")
            .expect("valid source identity should produce data type")
    );

    let decoded = ReferencePriceUpdate::from_custom_data(&custom)
        .expect("custom data should downcast to reference price update");
    assert_eq!(decoded, &update);
    assert_eq!(decoded.provider_instrument(), "BTC-USD.CHAINLINK_REFERENCE");
    assert_eq!(decoded.provenance(), &provenance);
    let quote = decoded
        .to_reference_quote()
        .expect("custom update should convert to normalized quote");
    assert_eq!(quote.provider_instrument(), "BTC-USD.CHAINLINK_REFERENCE");
    assert_eq!(quote.provenance(), &provenance);
}

fn provenance<const N: usize>(entries: [(&str, &str); N]) -> ReferenceQuoteProvenance {
    ReferenceQuoteProvenance::try_from_fields(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect::<BTreeMap<_, _>>(),
    )
    .expect("test provenance should be valid")
}
