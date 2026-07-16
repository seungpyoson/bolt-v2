//! Capability absence: Binance Spot REST SBE schema 3:5 exchangeInfo decode.
//!
//! Live incident 2026-07-09/10: pinned NT decoder demanded exact version 4 and
//! rejected vendor highest-compatible encoding (version 5) returned for a
//! deprecated `X-MBX-SBE: 3:4` request. Fixture is a real public capture.

use nautilus_binance::spot::http::{SbeDecodeError, parse::decode_exchange_info};

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
fn captured_exchange_info_schema_v5_is_rejected_by_official_pin() {
    let error = decode_exchange_info(CAPTURED_EXCHANGE_INFO_SCHEMA_V5)
        .expect_err("official v1.230.0 pin must not claim schema-3:5 decode capability");
    assert!(matches!(
        error,
        SbeDecodeError::VersionMismatch {
            expected: 4,
            actual: 5,
        }
    ));
}
