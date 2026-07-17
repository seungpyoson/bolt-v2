//! Capability presence: Binance Spot REST SBE schema 3:5 exchangeInfo decode.
//!
//! Live incident 2026-07-09/10: the prior pin demanded exact version 4 and
//! rejected the vendor's highest-compatible version 5 encoding. The fixture is
//! a real public capture decoded directly through the pinned NT parser.

use nautilus_binance::spot::http::parse::decode_exchange_info;

const CAPTURED_EXCHANGE_INFO_SCHEMA_V5: &[u8] =
    include_bytes!("fixtures/binance_sbe/exchange_info_btc_usdt_schema_3_5.bin");

#[test]
fn captured_exchange_info_schema_v5_wire_header_is_schema_3_version_5() {
    assert!(
        CAPTURED_EXCHANGE_INFO_SCHEMA_V5.len() >= 8,
        "fixture must contain at least the SBE message header"
    );
    let schema_id = u16::from_le_bytes([
        CAPTURED_EXCHANGE_INFO_SCHEMA_V5[4],
        CAPTURED_EXCHANGE_INFO_SCHEMA_V5[5],
    ]);
    let version = u16::from_le_bytes([
        CAPTURED_EXCHANGE_INFO_SCHEMA_V5[6],
        CAPTURED_EXCHANGE_INFO_SCHEMA_V5[7],
    ]);
    assert_eq!(schema_id, 3);
    assert_eq!(version, 5);
}

#[test]
fn captured_exchange_info_schema_v5_decodes_with_official_pin() {
    let exchange_info = decode_exchange_info(CAPTURED_EXCHANGE_INFO_SCHEMA_V5)
        .expect("official pin past upstream PR #4474 must decode schema 3:5");
    assert!(
        exchange_info
            .symbols
            .iter()
            .any(|symbol| symbol.symbol == "BTCUSDT"),
        "captured schema-3:5 exchange info must retain BTCUSDT"
    );
}
