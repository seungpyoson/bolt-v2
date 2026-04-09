mod config {
    pub use bolt_v2::config::{ReferenceVenueEntry, ReferenceVenueKind};
}

mod clients {
    pub use bolt_v2::clients::*;
}

#[path = "../src/platform/mod.rs"]
mod platform;

use crate::config::{ReferenceVenueEntry, ReferenceVenueKind};

#[test]
fn builds_reference_data_client_wrappers_for_supported_kinds() {
    let venues = [
        ReferenceVenueEntry {
            name: "BINANCE-BTC".to_string(),
            kind: ReferenceVenueKind::Binance,
            instrument_id: "BTCUSDT.BINANCE".to_string(),
            base_weight: 0.35,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
        ReferenceVenueEntry {
            name: "BYBIT-BTC".to_string(),
            kind: ReferenceVenueKind::Bybit,
            instrument_id: "BTCUSDT.BYBIT".to_string(),
            base_weight: 0.30,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
        ReferenceVenueEntry {
            name: "DERIBIT-BTC".to_string(),
            kind: ReferenceVenueKind::Deribit,
            instrument_id: "BTC-PERPETUAL.DERIBIT".to_string(),
            base_weight: 0.10,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
        ReferenceVenueEntry {
            name: "HYPERLIQUID-BTC".to_string(),
            kind: ReferenceVenueKind::Hyperliquid,
            instrument_id: "BTC-USD.HYPERLIQUID".to_string(),
            base_weight: 0.10,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
        ReferenceVenueEntry {
            name: "KRAKEN-BTC".to_string(),
            kind: ReferenceVenueKind::Kraken,
            instrument_id: "XBT/USD.KRAKEN".to_string(),
            base_weight: 0.10,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
        ReferenceVenueEntry {
            name: "OKX-BTC".to_string(),
            kind: ReferenceVenueKind::Okx,
            instrument_id: "BTC-USDT.OKX".to_string(),
            base_weight: 0.05,
            stale_after_ms: 1_500,
            disable_after_ms: 5_000,
        },
    ];

    for venue in venues {
        assert!(crate::platform::runtime::build_reference_data_client(&venue).is_ok());
    }
}
