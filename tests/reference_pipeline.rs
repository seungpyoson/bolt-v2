use bolt_v2::{
    config::{ReferenceVenueEntry, ReferenceVenueKind},
    platform::runtime::build_reference_data_client,
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
    assert_wrapper::<OKXDataClientConfig>(
        ReferenceVenueKind::Okx,
        "OKX",
        "OKXDataClientConfig",
    );
}
