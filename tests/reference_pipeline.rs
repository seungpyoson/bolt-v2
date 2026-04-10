use std::collections::BTreeMap;

use bolt_v2::{
    config::{ReferenceVenueEntry, ReferenceVenueKind},
    platform::{
        reference::{ReferenceObservation, VenueHealth, fuse_reference_snapshot},
        runtime::build_reference_data_client,
    },
};
use nautilus_binance::config::BinanceDataClientConfig;
use nautilus_bybit::config::BybitDataClientConfig;
use nautilus_deribit::config::DeribitDataClientConfig;
use nautilus_hyperliquid::config::HyperliquidDataClientConfig;
use nautilus_kraken::config::KrakenDataClientConfig;
use nautilus_okx::config::OKXDataClientConfig;
use nautilus_system::factories::ClientConfig;

fn venue(kind: ReferenceVenueKind) -> ReferenceVenueEntry {
    ReferenceVenueEntry {
        name: "TEST".into(),
        kind,
        instrument_id: "BTCUSDT.TEST".into(),
        base_weight: 0.5,
        stale_after_ms: 1_500,
        disable_after_ms: 5_000,
    }
}

fn assert_wrapper<C: ClientConfig + 'static>(
    kind: ReferenceVenueKind,
    expected_factory_name: &str,
    expected_config_type: &str,
) {
    let (factory, config) =
        build_reference_data_client(&venue(kind)).expect("wrapper should build successfully");

    assert_eq!(factory.name(), expected_factory_name);
    assert_eq!(factory.config_type(), expected_config_type);
    assert!(
        config.as_any().is::<C>(),
        "expected config type {expected_config_type}, got different concrete type"
    );
}

#[test]
fn builds_reference_data_client_wrappers_for_supported_kinds() {
    assert_wrapper::<BinanceDataClientConfig>(
        ReferenceVenueKind::Binance,
        "BINANCE",
        "BinanceDataClientConfig",
    );
    assert_wrapper::<BybitDataClientConfig>(
        ReferenceVenueKind::Bybit,
        "BYBIT",
        "BybitDataClientConfig",
    );
    assert_wrapper::<DeribitDataClientConfig>(
        ReferenceVenueKind::Deribit,
        "DERIBIT",
        "DeribitDataClientConfig",
    );
    assert_wrapper::<HyperliquidDataClientConfig>(
        ReferenceVenueKind::Hyperliquid,
        "HYPERLIQUID",
        "HyperliquidDataClientConfig",
    );
    assert_wrapper::<KrakenDataClientConfig>(
        ReferenceVenueKind::Kraken,
        "KRAKEN",
        "KrakenDataClientConfig",
    );
    assert_wrapper::<OKXDataClientConfig>(ReferenceVenueKind::Okx, "OKX", "OKXDataClientConfig");
}

fn reference_venue(
    name: &str,
    kind: ReferenceVenueKind,
    base_weight: f64,
    stale_after_ms: u64,
) -> ReferenceVenueEntry {
    ReferenceVenueEntry {
        name: name.into(),
        kind,
        instrument_id: format!("{name}.TEST"),
        base_weight,
        stale_after_ms,
        disable_after_ms: 5_000,
    }
}

fn orderbook(venue_name: &str, bid: f64, ask: f64, ts_ms: u64) -> ReferenceObservation {
    ReferenceObservation::Orderbook {
        venue_name: venue_name.into(),
        instrument_id: format!("{venue_name}.TEST"),
        bid,
        ask,
        ts_ms,
    }
}

fn oracle(venue_name: &str, price: f64, ts_ms: u64) -> ReferenceObservation {
    ReferenceObservation::Oracle {
        venue_name: venue_name.into(),
        instrument_id: format!("{venue_name}.TEST"),
        price,
        ts_ms,
    }
}

#[test]
fn stale_venue_weight_goes_to_zero() {
    let venues = vec![reference_venue(
        "BINANCE",
        ReferenceVenueKind::Binance,
        0.5,
        1_000,
    )];
    let latest = BTreeMap::from([(
        "BINANCE".to_string(),
        orderbook("BINANCE", 99.0, 101.0, 1_000),
    )]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        2_500,
        &venues,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, None);
    assert_eq!(snapshot.confidence, 0.0);
    assert_eq!(snapshot.venues.len(), 1);
    assert!(snapshot.venues[0].stale);
    assert_eq!(snapshot.venues[0].effective_weight, 0.0);
    assert_eq!(snapshot.venues[0].health, VenueHealth::Healthy);
}

#[test]
fn unhealthy_venue_is_excluded_from_fused_price() {
    let venues = vec![
        reference_venue("BINANCE", ReferenceVenueKind::Binance, 0.4, 1_000),
        reference_venue("BYBIT", ReferenceVenueKind::Bybit, 0.6, 1_000),
    ];
    let latest = BTreeMap::from([
        (
            "BINANCE".to_string(),
            orderbook("BINANCE", 99.0, 101.0, 1_000),
        ),
        ("BYBIT".to_string(), orderbook("BYBIT", 109.0, 111.0, 1_000)),
    ]);
    let disabled = BTreeMap::from([("BYBIT".to_string(), "manual disable".to_string())]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_500,
        &venues,
        &latest,
        &disabled,
    );

    assert_eq!(snapshot.fair_value, Some(100.0));
    assert_eq!(snapshot.confidence, 0.4);
    assert_eq!(
        snapshot.venues[1].health,
        VenueHealth::Disabled {
            reason: "manual disable".into()
        }
    );
    assert_eq!(snapshot.venues[1].effective_weight, 0.0);
}

#[test]
fn confidence_is_ratio_of_effective_to_configured_weight() {
    let venues = vec![
        reference_venue("BINANCE", ReferenceVenueKind::Binance, 0.25, 1_000),
        reference_venue("BYBIT", ReferenceVenueKind::Bybit, 0.75, 1_000),
    ];
    let latest = BTreeMap::from([
        (
            "BINANCE".to_string(),
            orderbook("BINANCE", 99.0, 101.0, 1_000),
        ),
        ("BYBIT".to_string(), orderbook("BYBIT", 199.0, 201.0, 1_000)),
    ]);
    let disabled = BTreeMap::from([("BYBIT".to_string(), "venue unhealthy".to_string())]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_100,
        &venues,
        &latest,
        &disabled,
    );

    assert_eq!(snapshot.confidence, 0.25);
}

#[test]
fn fused_price_is_weighted_mean_of_enabled_prices() {
    let venues = vec![
        reference_venue("BINANCE", ReferenceVenueKind::Binance, 2.0, 1_000),
        reference_venue("BYBIT", ReferenceVenueKind::Bybit, 1.0, 1_000),
    ];
    let latest = BTreeMap::from([
        (
            "BINANCE".to_string(),
            orderbook("BINANCE", 99.0, 101.0, 1_000),
        ),
        ("BYBIT".to_string(), orderbook("BYBIT", 119.0, 121.0, 1_000)),
    ]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_100,
        &venues,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, Some(106.66666666666667));
    assert_eq!(snapshot.confidence, 1.0);
}

#[test]
fn all_venues_stale_returns_none_fair_value() {
    let venues = vec![
        reference_venue("BINANCE", ReferenceVenueKind::Binance, 0.5, 100),
        reference_venue("BYBIT", ReferenceVenueKind::Bybit, 0.5, 100),
    ];
    let latest = BTreeMap::from([
        (
            "BINANCE".to_string(),
            orderbook("BINANCE", 99.0, 101.0, 1_000),
        ),
        ("BYBIT".to_string(), orderbook("BYBIT", 109.0, 111.0, 1_000)),
    ]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_500,
        &venues,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, None);
    assert_eq!(snapshot.confidence, 0.0);
    assert!(snapshot.venues.iter().all(|venue| venue.stale));
}

#[test]
fn empty_venues_returns_none_fair_value() {
    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_000,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, None);
    assert_eq!(snapshot.confidence, 0.0);
    assert!(snapshot.venues.is_empty());
}

#[test]
fn single_oracle_observation_uses_direct_price() {
    let venues = vec![reference_venue(
        "CHAINLINK",
        ReferenceVenueKind::Chainlink,
        1.0,
        1_000,
    )];
    let latest = BTreeMap::from([("CHAINLINK".to_string(), oracle("CHAINLINK", 104.25, 1_000))]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_100,
        &venues,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, Some(104.25));
    assert_eq!(snapshot.confidence, 1.0);
    assert_eq!(snapshot.venues[0].observed_price, Some(104.25));
}

#[test]
fn mismatched_observation_identity_is_ignored() {
    let venues = vec![ReferenceVenueEntry {
        name: "BINANCE".into(),
        kind: ReferenceVenueKind::Binance,
        instrument_id: "BTCUSDT.BINANCE".into(),
        base_weight: 1.0,
        stale_after_ms: 1_000,
        disable_after_ms: 5_000,
    }];
    let latest = BTreeMap::from([(
        "BINANCE".to_string(),
        ReferenceObservation::Orderbook {
            venue_name: "BINANCE".into(),
            instrument_id: "ETHUSDT.BINANCE".into(),
            bid: 199.0,
            ask: 201.0,
            ts_ms: 1_000,
        },
    )]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        1_100,
        &venues,
        &latest,
        &BTreeMap::new(),
    );

    assert_eq!(snapshot.fair_value, None);
    assert_eq!(snapshot.confidence, 0.0);
    assert_eq!(snapshot.venues[0].observed_price, None);
    assert_eq!(snapshot.venues[0].effective_weight, 0.0);
}

#[test]
fn venue_becomes_stale_before_it_is_auto_disabled() {
    let venues = vec![ReferenceVenueEntry {
        name: "BINANCE".into(),
        kind: ReferenceVenueKind::Binance,
        instrument_id: "BINANCE.TEST".into(),
        base_weight: 1.0,
        stale_after_ms: 1_000,
        disable_after_ms: 2_000,
    }];
    let latest = BTreeMap::from([(
        "BINANCE".to_string(),
        orderbook("BINANCE", 99.0, 101.0, 1_000),
    )]);

    let stale_snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        2_500,
        &venues,
        &latest,
        &BTreeMap::new(),
    );
    assert!(stale_snapshot.venues[0].stale);
    assert_eq!(stale_snapshot.venues[0].health, VenueHealth::Healthy);
    assert_eq!(stale_snapshot.venues[0].effective_weight, 0.0);

    let disabled_snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        3_001,
        &venues,
        &latest,
        &BTreeMap::new(),
    );
    assert!(disabled_snapshot.venues[0].stale);
    assert_eq!(disabled_snapshot.venues[0].effective_weight, 0.0);
    assert_eq!(
        disabled_snapshot.venues[0].health,
        VenueHealth::Disabled {
            reason: "auto-disabled after 2001ms without a fresh reference update".into(),
        }
    );
}

#[test]
fn manual_disable_reason_overrides_age_based_disable() {
    let venues = vec![ReferenceVenueEntry {
        name: "BYBIT".into(),
        kind: ReferenceVenueKind::Bybit,
        instrument_id: "BYBIT.TEST".into(),
        base_weight: 1.0,
        stale_after_ms: 1_000,
        disable_after_ms: 2_000,
    }];
    let latest = BTreeMap::from([("BYBIT".to_string(), orderbook("BYBIT", 109.0, 111.0, 1_000))]);
    let disabled = BTreeMap::from([("BYBIT".to_string(), "manual disable".to_string())]);

    let snapshot = fuse_reference_snapshot(
        "platform.reference.default",
        3_500,
        &venues,
        &latest,
        &disabled,
    );

    assert!(snapshot.venues[0].stale);
    assert_eq!(snapshot.venues[0].effective_weight, 0.0);
    assert_eq!(
        snapshot.venues[0].health,
        VenueHealth::Disabled {
            reason: "manual disable".into(),
        }
    );
}
