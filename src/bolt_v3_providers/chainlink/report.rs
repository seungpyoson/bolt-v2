//! Chainlink Data Streams V3 report decoding core.
//!
//! Pure protocol logic moved out of `crate::bolt_v3_operator_artifacts`:
//! the V3 `fullReport` ABI layout, fixed-point scaling, two's-complement
//! i192 handling, feed-id shape validation, and the report wire types.
//! No config resolution and no filesystem collectors live here.

use serde::{Deserialize, Serialize};

use crate::bolt_v3_operator_artifacts::{
    BoltV3OperatorArtifactError, ENTRY_DECISION_ZERO_TIMESTAMP_MS,
    price_to_beat_report_provenance_invalid,
};

const CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3: u64 = 3;
const CHAINLINK_REPORT_ABI_WORD_BYTES: usize = 32;
const CHAINLINK_REPORT_ABI_U64_VALUE_BYTES: usize = std::mem::size_of::<u64>();
const CHAINLINK_REPORT_ABI_U32_VALUE_BYTES: usize = std::mem::size_of::<u32>();
const CHAINLINK_REPORT_ABI_I192_VALUE_BYTES: usize =
    CHAINLINK_REPORT_ABI_WORD_BYTES - CHAINLINK_REPORT_ABI_U64_VALUE_BYTES;
const CHAINLINK_REPORT_ABI_U64_PREFIX_BYTES: usize =
    CHAINLINK_REPORT_ABI_WORD_BYTES - CHAINLINK_REPORT_ABI_U64_VALUE_BYTES;
const CHAINLINK_REPORT_ABI_U32_PREFIX_BYTES: usize =
    CHAINLINK_REPORT_ABI_WORD_BYTES - CHAINLINK_REPORT_ABI_U32_VALUE_BYTES;
const CHAINLINK_REPORT_ABI_I192_PREFIX_BYTES: usize =
    CHAINLINK_REPORT_ABI_WORD_BYTES - CHAINLINK_REPORT_ABI_I192_VALUE_BYTES;
const CHAINLINK_REPORT_BLOB_OFFSET_WORD_INDEX: usize = 3;
const CHAINLINK_REPORT_CALLBACK_MIN_BYTES: usize = 4 * CHAINLINK_REPORT_ABI_WORD_BYTES;
const CHAINLINK_REPORT_V3_WORD_COUNT: usize = 9;
const CHAINLINK_REPORT_V3_FEED_ID_WORD_INDEX: usize = 0;
const CHAINLINK_REPORT_V3_VALID_FROM_WORD_INDEX: usize = 1;
const CHAINLINK_REPORT_V3_OBSERVATIONS_WORD_INDEX: usize = 2;
const CHAINLINK_REPORT_V3_BENCHMARK_PRICE_WORD_INDEX: usize = 6;
const CHAINLINK_REPORT_V3_BID_PRICE_WORD_INDEX: usize = 7;
const CHAINLINK_REPORT_V3_ASK_PRICE_WORD_INDEX: usize = 8;
pub(crate) const CHAINLINK_REPORT_MILLISECONDS_PER_SECOND: u64 = 1_000;
const CHAINLINK_REPORT_SIGN_BIT_MASK: u8 = 0x80;
const CHAINLINK_REPORT_BASE256_RADIX: f64 = 256.0;
const CHAINLINK_REPORT_DECIMAL_RADIX: f64 = 10.0;
const CHAINLINK_FEED_ID_PREFIX: &str = "0x";
const CHAINLINK_FEED_ID_HEX_LENGTH: usize = 64;

pub(crate) struct PriceToBeatReportBinding {
    pub(crate) feed_id: String,
    pub(crate) schema_version: u64,
    pub(crate) decimal_scale: u64,
}

pub(crate) struct DecodedPriceToBeatReport {
    pub(crate) feed_id: String,
    pub(crate) valid_from_timestamp_ms: u64,
    pub(crate) observations_timestamp_ms: u64,
    pub(crate) benchmark_price: f64,
    pub(crate) bid_price: f64,
    pub(crate) ask_price: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChainlinkDataStreamsReportApiResponse {
    pub(crate) report: ChainlinkDataStreamsReportSource,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChainlinkDataStreamsReportSource {
    #[serde(rename = "feedID")]
    feed_id: String,
    #[serde(rename = "validFromTimestamp")]
    valid_from_timestamp: u64,
    #[serde(rename = "observationsTimestamp")]
    observations_timestamp: u64,
    #[serde(rename = "fullReport")]
    full_report: String,
}

impl ChainlinkDataStreamsReportSource {
    pub(crate) fn feed_id(&self) -> &str {
        self.feed_id.as_str()
    }
}

pub(crate) fn decode_price_to_beat_report(
    report_bytes: &[u8],
    binding: &PriceToBeatReportBinding,
) -> Result<DecodedPriceToBeatReport, BoltV3OperatorArtifactError> {
    if binding.schema_version != CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3 {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let source: ChainlinkDataStreamsReportSource = serde_json::from_slice(report_bytes)
        .map_err(|_| price_to_beat_report_provenance_invalid())?;
    if !is_lowercase_chainlink_feed_id(&source.feed_id)
        || source.feed_id != binding.feed_id
        || source.valid_from_timestamp == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || source.observations_timestamp == ENTRY_DECISION_ZERO_TIMESTAMP_MS
        || source.valid_from_timestamp > source.observations_timestamp
        || source.full_report.trim() != source.full_report
        || source.full_report.is_empty()
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let full_report_hex = source
        .full_report
        .strip_prefix(CHAINLINK_FEED_ID_PREFIX)
        .unwrap_or(source.full_report.as_str());
    let full_report =
        hex::decode(full_report_hex).map_err(|_| price_to_beat_report_provenance_invalid())?;
    let report_blob = decode_chainlink_full_report_blob(&full_report)?;
    let decoded = decode_chainlink_v3_report_blob(report_blob, binding)?;
    if decoded.feed_id != source.feed_id
        || decoded.valid_from_timestamp_ms
            != source
                .valid_from_timestamp
                .checked_mul(CHAINLINK_REPORT_MILLISECONDS_PER_SECOND)
                .ok_or_else(price_to_beat_report_provenance_invalid)?
        || decoded.observations_timestamp_ms
            != source
                .observations_timestamp
                .checked_mul(CHAINLINK_REPORT_MILLISECONDS_PER_SECOND)
                .ok_or_else(price_to_beat_report_provenance_invalid)?
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    Ok(decoded)
}

fn decode_chainlink_full_report_blob(
    full_report: &[u8],
) -> Result<&[u8], BoltV3OperatorArtifactError> {
    if full_report.len() < CHAINLINK_REPORT_CALLBACK_MIN_BYTES {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let offset =
        read_chainlink_abi_usize_word(full_report, CHAINLINK_REPORT_BLOB_OFFSET_WORD_INDEX)?;
    if offset < CHAINLINK_REPORT_CALLBACK_MIN_BYTES
        || offset
            .checked_add(CHAINLINK_REPORT_ABI_WORD_BYTES)
            .is_none_or(|end| end > full_report.len())
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let length = read_chainlink_abi_usize_at(full_report, offset)?;
    let start = offset
        .checked_add(CHAINLINK_REPORT_ABI_WORD_BYTES)
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let end = start
        .checked_add(length)
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    if end > full_report.len() {
        return Err(price_to_beat_report_provenance_invalid());
    }
    // The blob is only read sequentially by `decode_chainlink_v3_report_blob`,
    // so borrow the slice in place rather than allocating a fresh `Vec` per
    // decode (one strike report per interval).
    Ok(&full_report[start..end])
}

fn decode_chainlink_v3_report_blob(
    report_blob: &[u8],
    binding: &PriceToBeatReportBinding,
) -> Result<DecodedPriceToBeatReport, BoltV3OperatorArtifactError> {
    if report_blob.len() < CHAINLINK_REPORT_V3_WORD_COUNT * CHAINLINK_REPORT_ABI_WORD_BYTES {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let mut feed_id = String::from(CHAINLINK_FEED_ID_PREFIX);
    feed_id.push_str(&hex::encode(read_chainlink_abi_word(
        report_blob,
        CHAINLINK_REPORT_V3_FEED_ID_WORD_INDEX,
    )?));
    let valid_from_timestamp_ms = u64::from(read_chainlink_abi_u32_word(
        report_blob,
        CHAINLINK_REPORT_V3_VALID_FROM_WORD_INDEX,
    )?)
    .checked_mul(CHAINLINK_REPORT_MILLISECONDS_PER_SECOND)
    .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let observations_timestamp_ms = u64::from(read_chainlink_abi_u32_word(
        report_blob,
        CHAINLINK_REPORT_V3_OBSERVATIONS_WORD_INDEX,
    )?)
    .checked_mul(CHAINLINK_REPORT_MILLISECONDS_PER_SECOND)
    .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let benchmark_price_raw =
        read_chainlink_abi_i192_word(report_blob, CHAINLINK_REPORT_V3_BENCHMARK_PRICE_WORD_INDEX)?;
    let benchmark_price =
        scale_chainlink_report_price(&benchmark_price_raw, binding.decimal_scale)?;
    let bid_price_raw =
        read_chainlink_abi_i192_word(report_blob, CHAINLINK_REPORT_V3_BID_PRICE_WORD_INDEX)?;
    let bid_price = scale_chainlink_report_price(&bid_price_raw, binding.decimal_scale)?;
    let ask_price_raw =
        read_chainlink_abi_i192_word(report_blob, CHAINLINK_REPORT_V3_ASK_PRICE_WORD_INDEX)?;
    let ask_price = scale_chainlink_report_price(&ask_price_raw, binding.decimal_scale)?;
    Ok(DecodedPriceToBeatReport {
        feed_id,
        valid_from_timestamp_ms,
        observations_timestamp_ms,
        benchmark_price,
        bid_price,
        ask_price,
    })
}

fn read_chainlink_abi_word(
    bytes: &[u8],
    word_index: usize,
) -> Result<&[u8], BoltV3OperatorArtifactError> {
    let start = word_index
        .checked_mul(CHAINLINK_REPORT_ABI_WORD_BYTES)
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    let end = start
        .checked_add(CHAINLINK_REPORT_ABI_WORD_BYTES)
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    bytes
        .get(start..end)
        .ok_or_else(price_to_beat_report_provenance_invalid)
}

fn read_chainlink_abi_word_at(
    bytes: &[u8],
    start: usize,
) -> Result<&[u8], BoltV3OperatorArtifactError> {
    let end = start
        .checked_add(CHAINLINK_REPORT_ABI_WORD_BYTES)
        .ok_or_else(price_to_beat_report_provenance_invalid)?;
    bytes
        .get(start..end)
        .ok_or_else(price_to_beat_report_provenance_invalid)
}

fn read_chainlink_abi_usize_word(
    bytes: &[u8],
    word_index: usize,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let word = read_chainlink_abi_word(bytes, word_index)?;
    read_chainlink_abi_usize_from_word(word)
}

fn read_chainlink_abi_usize_at(
    bytes: &[u8],
    start: usize,
) -> Result<usize, BoltV3OperatorArtifactError> {
    let word = read_chainlink_abi_word_at(bytes, start)?;
    read_chainlink_abi_usize_from_word(word)
}

fn read_chainlink_abi_usize_from_word(word: &[u8]) -> Result<usize, BoltV3OperatorArtifactError> {
    if word.len() != CHAINLINK_REPORT_ABI_WORD_BYTES
        || word[..CHAINLINK_REPORT_ABI_U64_PREFIX_BYTES]
            .iter()
            .any(|byte| *byte != u8::MIN)
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let mut value = [u8::MIN; CHAINLINK_REPORT_ABI_U64_VALUE_BYTES];
    value.copy_from_slice(
        &word[CHAINLINK_REPORT_ABI_U64_PREFIX_BYTES..CHAINLINK_REPORT_ABI_WORD_BYTES],
    );
    usize::try_from(u64::from_be_bytes(value))
        .map_err(|_| price_to_beat_report_provenance_invalid())
}

fn read_chainlink_abi_u32_word(
    bytes: &[u8],
    word_index: usize,
) -> Result<u32, BoltV3OperatorArtifactError> {
    let word = read_chainlink_abi_word(bytes, word_index)?;
    if word[..CHAINLINK_REPORT_ABI_U32_PREFIX_BYTES]
        .iter()
        .any(|byte| *byte != u8::MIN)
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let mut value = [u8::MIN; CHAINLINK_REPORT_ABI_U32_VALUE_BYTES];
    value.copy_from_slice(
        &word[CHAINLINK_REPORT_ABI_U32_PREFIX_BYTES..CHAINLINK_REPORT_ABI_WORD_BYTES],
    );
    Ok(u32::from_be_bytes(value))
}

fn read_chainlink_abi_i192_word(
    bytes: &[u8],
    word_index: usize,
) -> Result<[u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES], BoltV3OperatorArtifactError> {
    let word = read_chainlink_abi_word(bytes, word_index)?;
    let negative =
        (word[CHAINLINK_REPORT_ABI_I192_PREFIX_BYTES] & CHAINLINK_REPORT_SIGN_BIT_MASK) != u8::MIN;
    let expected = if negative { u8::MAX } else { u8::MIN };
    if word[..CHAINLINK_REPORT_ABI_I192_PREFIX_BYTES]
        .iter()
        .any(|byte| *byte != expected)
    {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let mut value = [u8::MIN; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES];
    value.copy_from_slice(
        &word[CHAINLINK_REPORT_ABI_I192_PREFIX_BYTES..CHAINLINK_REPORT_ABI_WORD_BYTES],
    );
    Ok(value)
}

fn scale_chainlink_report_price(
    value: &[u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES],
    decimal_scale: u64,
) -> Result<f64, BoltV3OperatorArtifactError> {
    let scale =
        i32::try_from(decimal_scale).map_err(|_| price_to_beat_report_provenance_invalid())?;
    let scale_factor = CHAINLINK_REPORT_DECIMAL_RADIX.powi(-scale);
    if !scale_factor.is_finite() || scale_factor <= f64::from(u8::MIN) {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let magnitude = chainlink_i192_magnitude_to_f64(value);
    if !magnitude.is_finite() {
        return Err(price_to_beat_report_provenance_invalid());
    }
    let price = if chainlink_i192_word_is_negative(value) {
        -magnitude * scale_factor
    } else {
        magnitude * scale_factor
    };
    if !price.is_finite() {
        return Err(price_to_beat_report_provenance_invalid());
    }
    Ok(price)
}

fn chainlink_i192_word_is_negative(value: &[u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES]) -> bool {
    value
        .first()
        .is_some_and(|byte| (*byte & CHAINLINK_REPORT_SIGN_BIT_MASK) != u8::MIN)
}

fn chainlink_i192_magnitude_to_f64(value: &[u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES]) -> f64 {
    let magnitude = if chainlink_i192_word_is_negative(value) {
        chainlink_i192_twos_complement_abs(value)
    } else {
        *value
    };
    magnitude.iter().fold(f64::from(u8::MIN), |acc, byte| {
        acc.mul_add(CHAINLINK_REPORT_BASE256_RADIX, f64::from(*byte))
    })
}

fn chainlink_i192_twos_complement_abs(
    value: &[u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES],
) -> [u8; CHAINLINK_REPORT_ABI_I192_VALUE_BYTES] {
    let mut magnitude = *value;
    for byte in &mut magnitude {
        *byte = !*byte;
    }
    let mut carry = u8::from(true);
    for byte in magnitude.iter_mut().rev() {
        let (next, overflow) = byte.overflowing_add(carry);
        *byte = next;
        carry = u8::from(overflow);
        if carry == u8::MIN {
            break;
        }
    }
    magnitude
}

pub(crate) fn is_lowercase_chainlink_feed_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix(CHAINLINK_FEED_ID_PREFIX) else {
        return false;
    };
    hex.len() == CHAINLINK_FEED_ID_HEX_LENGTH
        && hex
            .chars()
            .all(|char| matches!(char, '0'..='9' | 'a'..='f'))
}

#[cfg(test)]
mod tests {
    //! V3 `fullReport` decode and feed-id shape unit tests.
    //!
    //! The report-blob fixture builder mirrors the ABI layout used by the
    //! offline materializer tests in `tests/bolt_v3_operator_artifacts.rs`
    //! (same word indices, same `validFrom`/`observations`/benchmark slots, same
    //! `0x`-prefixed full-report hex), so the expected decoded values are the
    //! same across both call sites.

    use rust_decimal::{Decimal, prelude::ToPrimitive};

    use super::*;

    const TEST_FEED_ID: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const TEST_DECIMAL_SCALE: u64 = 18;
    const TEST_BENCHMARK_PRICE: f64 = 3300.5;
    const TEST_VALID_FROM_SECONDS: u32 = 600;
    const TEST_OBSERVATIONS_SECONDS: u32 = 601;

    fn binding(feed_id: &str, schema_version: u64, decimal_scale: u64) -> PriceToBeatReportBinding {
        PriceToBeatReportBinding {
            feed_id: feed_id.to_string(),
            schema_version,
            decimal_scale,
        }
    }

    fn abi_zero_word() -> [u8; 32] {
        [0_u8; 32]
    }

    fn abi_u32_word(value: u32) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[28..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn abi_usize_word(value: usize) -> [u8; 32] {
        let mut word = [0_u8; 32];
        word[24..32].copy_from_slice(&(value as u64).to_be_bytes());
        word
    }

    fn abi_i192_word(value: i128) -> [u8; 32] {
        let mut word = if value < 0 { [0xff_u8; 32] } else { [0_u8; 32] };
        word[16..32].copy_from_slice(&value.to_be_bytes());
        word
    }

    fn feed_id_bytes(feed_id: &str) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        let decoded = hex::decode(feed_id.strip_prefix("0x").expect("feed id should have 0x"))
            .expect("feed id should decode");
        bytes.copy_from_slice(&decoded);
        bytes
    }

    fn scaled_price(benchmark_price: f64, decimal_scale: u64) -> i128 {
        let scale = 10_i128
            .checked_pow(u32::try_from(decimal_scale).expect("scale should fit u32"))
            .expect("scale should fit i128");
        let price = Decimal::from_str_exact(&benchmark_price.to_string())
            .expect("benchmark price should be decimal");
        (price * Decimal::from(scale))
            .round()
            .to_i128()
            .expect("scaled price should fit i128")
    }

    /// Builds a full ABI-encoded `fullReport` callback payload (4-word header +
    /// length-prefixed V3 report blob) for the given feed/timestamps/price.
    fn full_report_payload(
        feed_id: &str,
        valid_from_seconds: u32,
        observations_seconds: u32,
        benchmark_word: [u8; 32],
    ) -> Vec<u8> {
        let mut blob = Vec::new();
        blob.extend_from_slice(&feed_id_bytes(feed_id));
        blob.extend_from_slice(&abi_u32_word(valid_from_seconds));
        blob.extend_from_slice(&abi_u32_word(observations_seconds));
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_zero_word());
        blob.extend_from_slice(&abi_u32_word(observations_seconds + 60));
        blob.extend_from_slice(&benchmark_word);
        blob.extend_from_slice(&benchmark_word);
        blob.extend_from_slice(&benchmark_word);

        let mut payload = Vec::new();
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_zero_word());
        payload.extend_from_slice(&abi_usize_word(128));
        payload.extend_from_slice(&abi_usize_word(blob.len()));
        payload.extend_from_slice(&blob);
        payload
    }

    /// Serializes a Chainlink Data Streams report-source JSON envelope wrapping
    /// the given full-report payload.
    fn report_source_json(
        feed_id: &str,
        valid_from_seconds: u32,
        observations_seconds: u32,
        benchmark_word: [u8; 32],
        include_hex_prefix: bool,
    ) -> Vec<u8> {
        let full_report = full_report_payload(
            feed_id,
            valid_from_seconds,
            observations_seconds,
            benchmark_word,
        );
        let full_report_hex = if include_hex_prefix {
            format!("0x{}", hex::encode(full_report))
        } else {
            hex::encode(full_report)
        };
        serde_json::to_vec_pretty(&serde_json::json!({
            "feedID": feed_id,
            "validFromTimestamp": valid_from_seconds,
            "observationsTimestamp": observations_seconds,
            "fullReport": full_report_hex,
        }))
        .expect("report source JSON should serialize")
    }

    #[test]
    fn decode_v3_report_recovers_feed_timestamps_and_benchmark_price() {
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            TEST_VALID_FROM_SECONDS,
            TEST_OBSERVATIONS_SECONDS,
            abi_i192_word(scaled_price(TEST_BENCHMARK_PRICE, TEST_DECIMAL_SCALE)),
            true,
        );
        let decoded = decode_price_to_beat_report(
            &report_bytes,
            &binding(
                TEST_FEED_ID,
                CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3,
                TEST_DECIMAL_SCALE,
            ),
        )
        .expect("well-formed V3 report should decode");

        assert_eq!(decoded.feed_id, TEST_FEED_ID);
        assert_eq!(
            decoded.valid_from_timestamp_ms,
            u64::from(TEST_VALID_FROM_SECONDS) * CHAINLINK_REPORT_MILLISECONDS_PER_SECOND
        );
        assert_eq!(
            decoded.observations_timestamp_ms,
            u64::from(TEST_OBSERVATIONS_SECONDS) * CHAINLINK_REPORT_MILLISECONDS_PER_SECOND
        );
        assert!(
            (decoded.benchmark_price - TEST_BENCHMARK_PRICE).abs() < 1e-6,
            "benchmark price should round-trip, got {}",
            decoded.benchmark_price
        );
    }

    #[test]
    fn decode_v3_report_handles_full_int192_width_without_i128_bound() {
        // 2^128 (set bit 128) scaled by 1e38 decodes to ~3.4 — exercises the
        // i192 path beyond the i128 range, matching the offline materializer's
        // int192 fixture expectation.
        let mut benchmark_word = [0_u8; 32];
        benchmark_word[15] = 1;
        let report_bytes = report_source_json(TEST_FEED_ID, 600, 601, benchmark_word, true);
        let decoded = decode_price_to_beat_report(
            &report_bytes,
            &binding(TEST_FEED_ID, CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3, 38),
        )
        .expect("int192-width benchmark should decode");
        assert!(
            decoded.benchmark_price.is_finite()
                && decoded.benchmark_price > 3.4
                && decoded.benchmark_price < 3.5,
            "2^128 / 1e38 should be ~3.4, got {}",
            decoded.benchmark_price
        );
    }

    #[test]
    fn decode_v3_report_rejects_non_v3_schema_version() {
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            TEST_VALID_FROM_SECONDS,
            TEST_OBSERVATIONS_SECONDS,
            abi_i192_word(scaled_price(TEST_BENCHMARK_PRICE, TEST_DECIMAL_SCALE)),
            true,
        );
        assert!(
            decode_price_to_beat_report(
                &report_bytes,
                &binding(
                    TEST_FEED_ID,
                    CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3 + 1,
                    TEST_DECIMAL_SCALE
                ),
            )
            .is_err(),
            "a non-V3 schema version must fail closed"
        );
    }

    #[test]
    fn decode_v3_report_rejects_feed_id_mismatch_against_binding() {
        let other_feed_id = "0x0009d39e2dd17e7c1c8d2e0d6e8f3b3a1c2d4e5f60718293a4b5c6d7e8f90a1b";
        let report_bytes = report_source_json(
            TEST_FEED_ID,
            TEST_VALID_FROM_SECONDS,
            TEST_OBSERVATIONS_SECONDS,
            abi_i192_word(scaled_price(TEST_BENCHMARK_PRICE, TEST_DECIMAL_SCALE)),
            true,
        );
        assert!(
            decode_price_to_beat_report(
                &report_bytes,
                &binding(
                    other_feed_id,
                    CHAINLINK_PRICE_REPORT_SCHEMA_VERSION_V3,
                    TEST_DECIMAL_SCALE
                ),
            )
            .is_err(),
            "a report whose feed id differs from the binding must fail closed"
        );
    }

    #[test]
    fn feed_id_shape_accepts_only_lowercase_0x_64_hex() {
        assert!(is_lowercase_chainlink_feed_id(TEST_FEED_ID));
        // Uppercase hex digit.
        assert!(!is_lowercase_chainlink_feed_id(
            "0x000362205E10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9"
        ));
        // Missing 0x prefix.
        assert!(!is_lowercase_chainlink_feed_id(
            "000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9"
        ));
        // Wrong length (63 hex chars).
        assert!(!is_lowercase_chainlink_feed_id(
            "0x00362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9"
        ));
        // Non-hex character.
        assert!(!is_lowercase_chainlink_feed_id(
            "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3aeg"
        ));
    }
}
