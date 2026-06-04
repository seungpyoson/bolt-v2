//! OKX full-L2 order-book converter (venue slice of spec 023 `1-backtesting-engine`).
//!
//! OKX is a FULL L2 (market-by-price) venue. Its order-book archive object is a
//! gzip of a POSIX `ustar` tar whose single `*.data` member is JSONL, one book
//! message per line:
//!
//! ```text
//! {"instId","action":"snapshot"|"update","ts","asks":[["px","sz","cnt"],...],"bids":[...]}
//! ```
//!
//! Mapping to NautilusTrader [`OrderBookDelta`]s follows the venue's own L2
//! semantics, identical to NautilusTrader's Tardis L2 loader convention
//! (`crates/adapters/tardis/src/csv/{mod,load}.rs` @ rev `6e059dc`):
//!
//! * `action == "snapshot"` opens a new book image: emit one
//!   [`BookAction::Clear`] at the message timestamp, then one
//!   [`BookAction::Add`] per level (a `sz == "0"` level inside a snapshot maps to
//!   [`BookAction::Delete`], matching `parse_book_action`).
//! * `action == "update"` is an incremental change: each level maps to
//!   [`BookAction::Delete`] when `sz == "0"`, otherwise [`BookAction::Update`].
//! * `bids` map to [`OrderSide::Buy`]; `asks` map to [`OrderSide::Sell`].
//! * `order_id` is `0` — not applicable to L2 (price-level keyed) data.
//! * `ts` is exchange milliseconds; converted to UnixNanos.
//!
//! All price/size strings are normalized to a single venue-wide precision
//! (the maximum decimal places observed across every level), so every emitted
//! delta shares the price/size precision NautilusTrader records in the catalog
//! parquet metadata. No price, size, instrument id, or venue literal is baked
//! into this module — those arrive through [`OkxBookSpec`] and the parsed data.
//!
//! # Other OKX market-data families
//!
//! Beyond the full-L2 order book, this module also projects the two OKX
//! market-data families that carry no order book, sharing the same NT-first
//! contract (parse the real `.zip` object, project into an NT-native type, write
//! and read back via [`ParquetDataCatalog`]):
//!
//! * `family=trades` -> NautilusTrader [`TradeTick`]. The staged object is a
//!   ZIP of a single header CSV
//!   (`instrument_name,trade_id,side,price,size,created_time`). `side` is the
//!   taker/aggressor side directly (`buy`/`sell`), and `created_time` is integer
//!   milliseconds. See [`project_okx_trades_archive_to_catalog`].
//! * `family=candlesticks` -> NautilusTrader [`Bar`]. The staged object is a ZIP
//!   of a single header CSV
//!   (`instrument_name,open,high,low,close,vol,vol_ccy,vol_quote,open_time,confirm`).
//!   `open_time` is the bar-open timestamp in integer milliseconds, `vol` is the
//!   contract volume, and the bar step/unit arrive in [`OkxBarSpec`]. See
//!   [`project_okx_candlesticks_archive_to_catalog`].
//!
//! `family=funding_rates` is deliberately not converted here: a funding rate is
//! not a NautilusTrader market-data catalog type. It is left for direct-parquet
//! research.

use std::{
    fs,
    io::Read,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::{Crc, read::DeflateDecoder, read::GzDecoder};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{Bar, BarSpecification, BarType, BookOrder, OrderBookDelta, TradeTick},
    enums::{
        AggregationSource, AggressorSide, BarAggregation, BookAction, OrderSide, PriceType,
        RecordFlag,
    },
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// NautilusTrader data type written for the OKX `trades` family.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the OKX `candlesticks` family.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// NautilusTrader venue code for OKX, appended to a venue-native instrument id
/// to form the catalog instrument id (`<venue_inst_id>.OKX`). The data-derived
/// bulk path needs this because OKX stages no instrument universe to carry it.
pub const OKX_VENUE: &str = "OKX";

/// Exchange timestamps in the OKX `trades`/`candlesticks` CSVs are integer
/// milliseconds.
const TRADES_NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// OKX book messages carry no per-order identity (full L2 / market-by-price), so
/// every emitted [`BookOrder`] uses the NautilusTrader L2 sentinel order id `0`.
const L2_ORDER_ID: u64 = 0;

/// Exchange timestamps in the OKX archive are integer milliseconds.
const NANOS_PER_MILLISECOND: u64 = 1_000_000;

/// Size of a POSIX `ustar` header block.
const TAR_BLOCK: usize = 512;

/// Instrument/venue binding needed to build the NautilusTrader [`InstrumentId`].
///
/// Built from accepted run-spec metadata; never hardcoded in the converter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkxBookSpec {
    /// NautilusTrader instrument id for the book stream, for example
    /// `BNB-USD_UM_XPERP-310523.OKX`.
    pub nt_instrument_id: String,
    /// Venue-native instrument id as it appears in the `instId` field, used to
    /// fence out foreign rows, for example `BNB-USD_UM_XPERP-310523`.
    pub venue_inst_id: String,
}

/// One price level: `[price, size, order_count]` as decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OkxBookLevel(pub String, pub String, pub String);

/// One OKX order-book message line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OkxBookMessage {
    #[serde(rename = "instId")]
    pub inst_id: String,
    pub action: OkxBookAction,
    /// Exchange timestamp, integer milliseconds, as a string.
    pub ts: String,
    #[serde(default)]
    pub asks: Vec<OkxBookLevel>,
    #[serde(default)]
    pub bids: Vec<OkxBookLevel>,
}

/// OKX book message action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OkxBookAction {
    Snapshot,
    Update,
}

/// Result of projecting OKX book messages into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkxBookProjection {
    pub catalog_root: PathBuf,
    pub nt_instrument_id: String,
    pub data_type: String,
    pub delta_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
}

/// Gunzip an OKX order-book archive and return the JSONL bytes of its single
/// `*.data` tar member.
///
/// The archive is a gzip of a POSIX `ustar` tar carrying exactly one regular
/// file. The tar is parsed directly from its 512-byte header blocks (octal size
/// field) — no external tar dependency — because the layout is fixed and the
/// crate must stay self-contained.
///
/// # Errors
///
/// Returns an error if the gzip cannot be decoded, no regular-file `.data`
/// member is found, or a header is malformed.
pub fn extract_jsonl_from_archive(gz_bytes: &[u8]) -> Result<String> {
    let mut decoder = GzDecoder::new(gz_bytes);
    let mut tar = Vec::new();
    decoder
        .read_to_end(&mut tar)
        .context("gunzip OKX order-book archive")?;

    let mut offset = 0usize;
    while offset + TAR_BLOCK <= tar.len() {
        let header = &tar[offset..offset + TAR_BLOCK];
        // An all-zero header marks end-of-archive padding; stop the member scan.
        if header.iter().all(|&b| b == 0) {
            break;
        }

        let size = parse_tar_octal_size(header)?;
        let typeflag = header[156];
        let data_start = offset + TAR_BLOCK;
        let data_end = data_start
            .checked_add(size)
            .context("tar member size overflow")?;
        ensure!(
            data_end <= tar.len(),
            "tar member extends past archive end (need {data_end}, have {})",
            tar.len()
        );

        // typeflag '0' or NUL = regular file (the JSONL `.data` member).
        if (typeflag == b'0' || typeflag == 0) && tar_member_name(header).ends_with(".data") {
            return String::from_utf8(tar[data_start..data_end].to_vec())
                .context("OKX `.data` member is not valid UTF-8");
        }

        // Advance past this member's data, rounded up to the next block.
        let padded = size.div_ceil(TAR_BLOCK) * TAR_BLOCK;
        offset = data_start
            .checked_add(padded)
            .context("tar offset overflow")?;
    }

    bail!("no `.data` member found in OKX order-book archive")
}

/// Parse the octal `size` field (bytes 124..136) of a POSIX tar header.
fn parse_tar_octal_size(header: &[u8]) -> Result<usize> {
    let trimmed = header[124..136]
        .iter()
        .take_while(|&&b| b != 0 && b != b' ')
        .copied()
        .collect::<Vec<u8>>();
    let text = std::str::from_utf8(&trimmed).context("tar size field not ASCII")?;
    let text = text.trim();
    ensure!(!text.is_empty(), "tar size field empty");
    usize::from_str_radix(text, 8).context("tar size field not octal")
}

/// Parse the `name` field (bytes 0..100) of a POSIX tar header.
fn tar_member_name(header: &[u8]) -> String {
    let raw = &header[0..100];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// Parse OKX book messages from JSONL text, keeping only messages whose `instId`
/// matches `venue_inst_id`.
///
/// # Errors
///
/// Returns an error if any non-blank line is not a valid OKX book message.
pub fn parse_okx_book_messages(jsonl: &str, venue_inst_id: &str) -> Result<Vec<OkxBookMessage>> {
    let mut messages = Vec::new();
    for (index, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: OkxBookMessage = serde_json::from_str(line)
            .with_context(|| format!("parse OKX book message on line {}", index + 1))?;
        if message.inst_id == venue_inst_id {
            messages.push(message);
        }
    }
    Ok(messages)
}

/// NautilusTrader standard `FIXED_PRECISION`. Source decimals are rounded to this
/// many places before precision is derived or a `Price`/`Quantity` is built: OKX
/// renders some values as f64 round-trip artifacts (`"0.09655999999999999"` for a
/// true `0.09656`, `"0.09382000000000000"` for `0.09382`) whose spurious 15-17th
/// place digits would otherwise blow past the cap. Rounding recovers the intended
/// tick value and bounds precision to what the catalog can store.
const NT_FIXED_PRECISION: u32 = 9;

/// Significant decimal places of a decimal string after rounding to
/// [`NT_FIXED_PRECISION`] and stripping trailing zeros (`"643.3"` -> 1,
/// `"5995"` -> 0, `"0.09655999999999999"` -> 5).
fn decimal_places(value: &str) -> Result<u8> {
    let decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.round_dp(NT_FIXED_PRECISION).normalize().scale())
        .context("decimal scale exceeds u8")
}

/// Scan all levels of all messages to determine the uniform price/size precision
/// (the maximum decimal places observed for each), so every emitted delta shares
/// the precision NautilusTrader records in the catalog parquet metadata.
fn resolve_precisions(messages: &[OkxBookMessage]) -> Result<(u8, u8)> {
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for message in messages {
        for level in message.asks.iter().chain(message.bids.iter()) {
            price_precision = price_precision.max(decimal_places(&level.0)?);
            size_precision = size_precision.max(decimal_places(&level.1)?);
        }
    }
    Ok((price_precision, size_precision))
}

/// Rescale a decimal string to exactly `precision` decimal places so the parsed
/// [`Price`]/[`Quantity`] carries the uniform venue precision.
fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value)
        .with_context(|| format!("decimal {value:?}"))?
        .round_dp(NT_FIXED_PRECISION);
    ensure!(
        decimal.normalize().scale() <= u32::from(precision),
        "value {value:?} has more precision than venue allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

fn price_at(value: &str, precision: u8) -> Result<Price> {
    let text = rescaled(value, precision)?;
    Price::from_str(&text).map_err(|error| anyhow::anyhow!("invalid price {text:?}: {error}"))
}

fn quantity_at(value: &str, precision: u8) -> Result<Quantity> {
    let text = rescaled(value, precision)?;
    Quantity::from_str(&text).map_err(|error| anyhow::anyhow!("invalid size {text:?}: {error}"))
}

/// Build one [`OrderBookDelta`] for a single price level.
#[allow(clippy::too_many_arguments)]
fn level_to_delta(
    instrument_id: InstrumentId,
    side: OrderSide,
    level: &OkxBookLevel,
    is_snapshot: bool,
    price_precision: u8,
    size_precision: u8,
    sequence: u64,
    ts: UnixNanos,
) -> Result<OrderBookDelta> {
    let price = price_at(&level.0, price_precision)?;
    let size = quantity_at(&level.1, size_precision)?;
    // Identical to NautilusTrader's `parse_book_action`: a zero-size level is a
    // delete regardless of message kind; otherwise a snapshot level is an Add and
    // an update level is an Update.
    let action = if size.is_zero() {
        BookAction::Delete
    } else if is_snapshot {
        BookAction::Add
    } else {
        BookAction::Update
    };
    let flags = if is_snapshot {
        RecordFlag::F_SNAPSHOT as u8
    } else {
        0
    };
    let order = BookOrder::new(side, price, size, L2_ORDER_ID);
    Ok(OrderBookDelta::new(
        instrument_id,
        action,
        order,
        flags,
        sequence,
        ts,
        ts,
    ))
}

/// Convert parsed OKX book messages into NautilusTrader [`OrderBookDelta`]s.
///
/// A `snapshot` message emits a leading [`BookAction::Clear`] then one delta per
/// level; an `update` message emits one delta per level. Deltas are produced in
/// message order, which preserves the archive's non-decreasing timestamps, so
/// the resulting stream satisfies NautilusTrader's ascending-timestamp write
/// contract.
///
/// # Errors
///
/// Returns an error if a price/size string cannot be represented at the resolved
/// venue precision, if a foreign instrument leaks in, or if the messages are not
/// timestamp-ordered.
pub fn okx_book_messages_to_deltas(
    messages: &[OkxBookMessage],
    spec: &OkxBookSpec,
) -> Result<Vec<OrderBookDelta>> {
    let instrument_id = InstrumentId::from_str(&spec.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", spec.nt_instrument_id))?;
    let (price_precision, size_precision) = resolve_precisions(messages)?;

    let mut deltas = Vec::new();
    let mut sequence = 0u64;
    let mut last_ts = 0u64;

    for message in messages {
        ensure!(
            message.inst_id == spec.venue_inst_id,
            "message instId {:?} does not match spec venue_inst_id {:?}",
            message.inst_id,
            spec.venue_inst_id
        );
        let ts_ms = message
            .ts
            .parse::<u64>()
            .with_context(|| format!("non-integer ts {:?}", message.ts))?;
        let ts_nanos = ts_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .context("ts milliseconds overflow when scaling to nanos")?;
        ensure!(
            ts_nanos >= last_ts,
            "OKX messages out of order: ts {ts_nanos} < previous {last_ts}"
        );
        last_ts = ts_nanos;
        let ts = UnixNanos::from(ts_nanos);
        let is_snapshot = matches!(message.action, OkxBookAction::Snapshot);

        if is_snapshot {
            deltas.push(OrderBookDelta::clear(instrument_id, sequence, ts, ts));
            sequence += 1;
        }

        for (side, levels) in [
            (OrderSide::Buy, &message.bids),
            (OrderSide::Sell, &message.asks),
        ] {
            for level in levels {
                deltas.push(level_to_delta(
                    instrument_id,
                    side,
                    level,
                    is_snapshot,
                    price_precision,
                    size_precision,
                    sequence,
                    ts,
                )?);
                sequence += 1;
            }
        }
    }

    Ok(deltas)
}

/// Project an OKX order-book archive into a NautilusTrader `ParquetDataCatalog`.
///
/// Extracts the JSONL member, parses and fences the messages to the spec's
/// instrument, builds [`OrderBookDelta`]s, and writes them with NautilusTrader's
/// own `write_to_parquet`. Returns a deterministic catalog hash.
///
/// # Errors
///
/// Returns an error if extraction, parsing, delta construction, or the catalog
/// write fails, or if `catalog_root` is a non-empty directory.
pub fn project_okx_book_archive_to_catalog(
    gz_bytes: &[u8],
    spec: &OkxBookSpec,
    catalog_root: &Path,
) -> Result<OkxBookProjection> {
    let jsonl = extract_jsonl_from_archive(gz_bytes)?;
    let messages = parse_okx_book_messages(&jsonl, &spec.venue_inst_id)?;
    ensure!(
        !messages.is_empty(),
        "no OKX book messages matched venue_inst_id {:?}",
        spec.venue_inst_id
    );
    let (price_precision, size_precision) = resolve_precisions(&messages)?;
    let deltas = okx_book_messages_to_deltas(&messages, spec)?;
    let delta_count = deltas.len();

    // Fail closed on a dirty catalog root: NautilusTrader's `write_to_parquet`
    // skips writing when a file for the same instrument/interval already exists,
    // so projecting into a non-empty root could silently read back stale data.
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(deltas, None, None, None)
        .context("write order book deltas to catalog")?;

    Ok(OkxBookProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_instrument_id: spec.nt_instrument_id.clone(),
        data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
        delta_count,
        price_precision,
        size_precision,
        catalog_hash: catalog_hash(catalog_root)?,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `OrderBookDelta` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_order_book_deltas(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<OrderBookDelta>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<OrderBookDelta>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query order book deltas from catalog")
}

/// Deterministic SHA-256 hex over every file under `root`, ordered by relative
/// path, mixing in each relative path so renames change the hash.
fn catalog_hash(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for relative in files {
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = fs::read(root.join(&relative))
            .with_context(|| format!("read catalog file {}", relative.display()))?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

// ===========================================================================
// ZIP single-member extraction (shared by trades + candlesticks)
// ===========================================================================
//
// OKX `trades` and `candlesticks` staged objects are ZIP archives carrying
// exactly one deflate-compressed CSV member. As with the order-book tar parser
// above, the ZIP local file header is parsed directly (no external `zip`
// dependency) and the single member is inflated with `flate2`'s raw deflate
// decoder, so the crate stays self-contained.

/// ZIP local file header signature (`PK\x03\x04`).
const ZIP_LOCAL_HEADER_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// ZIP central directory header signature (`PK\x01\x02`).
const ZIP_CENTRAL_HEADER_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
/// Fixed length of a ZIP local file header before the variable-length name.
const ZIP_LOCAL_HEADER_LEN: usize = 30;
/// Compression method code for raw DEFLATE.
const ZIP_METHOD_DEFLATE: u16 = 8;
/// Compression method code for STORED (no compression).
const ZIP_METHOD_STORED: u16 = 0;
/// General-purpose flag bit 3: sizes/CRC are zero in the local header and a
/// data descriptor follows the compressed data instead.
const ZIP_FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
/// ZIP64 extended-information extra-field header id.
const ZIP64_EXTRA_ID: u16 = 0x0001;
/// Sentinel a 32-bit ZIP size field carries when the real 64-bit value lives in
/// the ZIP64 extended-information extra field (member exceeds the 4 GiB u32 cap).
const ZIP32_SIZE_SENTINEL: u32 = 0xFFFF_FFFF;

fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Locate the single member's compressed size by scanning the ZIP central
/// directory, used when the local header advertised streaming sizes
/// (general-purpose flag bit 3 set).
///
/// Returns `(compressed_size, uncompressed_size, crc32)`.
fn central_directory_member(zip: &[u8]) -> Result<(usize, usize, u32)> {
    // Scan forward for the first central directory header. The archives here
    // carry exactly one member, so the first central record describes it.
    let mut offset = 0usize;
    while offset + 46 <= zip.len() {
        if zip[offset..offset + 4] == ZIP_CENTRAL_HEADER_SIG {
            let crc = read_u32_le(zip, offset + 16);
            let csize = read_u32_le(zip, offset + 20) as usize;
            let usize_field = read_u32_le(zip, offset + 24) as usize;
            return Ok((csize, usize_field, crc));
        }
        offset += 1;
    }
    bail!("ZIP central directory header not found while resolving streamed member sizes")
}

/// Resolve the real 64-bit `(uncompressed, compressed)` sizes from a local
/// header's ZIP64 extended-information extra field. Per APPNOTE the block
/// (id `0x0001`) stores the original (uncompressed) size first, then the
/// compressed size, each present only when the corresponding 32-bit field held
/// the `0xFFFFFFFF` sentinel, in that fixed order. Returns `None` for a size
/// whose 32-bit field was not a sentinel.
fn zip64_sizes(
    extra: &[u8],
    uncompressed_is_sentinel: bool,
    compressed_is_sentinel: bool,
) -> Result<(Option<usize>, Option<usize>)> {
    let mut offset = 0usize;
    while offset + 4 <= extra.len() {
        let id = read_u16_le(extra, offset);
        let block_len = read_u16_le(extra, offset + 2) as usize;
        let body = offset + 4;
        let body_end = body
            .checked_add(block_len)
            .context("ZIP64 extra block length overflow")?;
        ensure!(
            body_end <= extra.len(),
            "ZIP64 extra block extends past the extra field"
        );
        if id == ZIP64_EXTRA_ID {
            let mut cursor = body;
            let mut uncompressed = None;
            if uncompressed_is_sentinel {
                ensure!(
                    cursor + 8 <= body_end,
                    "ZIP64 extra block too short for the uncompressed size"
                );
                let value = u64::from_le_bytes(
                    extra[cursor..cursor + 8]
                        .try_into()
                        .expect("8-byte little-endian slice"),
                );
                uncompressed =
                    Some(usize::try_from(value).context("ZIP64 uncompressed size exceeds usize")?);
                cursor += 8;
            }
            let mut compressed = None;
            if compressed_is_sentinel {
                ensure!(
                    cursor + 8 <= body_end,
                    "ZIP64 extra block too short for the compressed size"
                );
                let value = u64::from_le_bytes(
                    extra[cursor..cursor + 8]
                        .try_into()
                        .expect("8-byte little-endian slice"),
                );
                compressed =
                    Some(usize::try_from(value).context("ZIP64 compressed size exceeds usize")?);
            }
            return Ok((uncompressed, compressed));
        }
        offset = body_end;
    }
    bail!("ZIP64 extra field (id 0x0001) not found despite a 0xFFFFFFFF size sentinel")
}

/// A streaming reader over the single member of an OKX/Binance ZIP archive.
///
/// Inflates DEFLATE (or passes through STORED) on the fly while accumulating the
/// CRC-32 and inflated byte count, so a multi-GiB member is consumed in bounded
/// chunks (the Binance aggTrades / markPriceKlines bulk paths stream through
/// this) and a corrupt or truncated member fails loud at end-of-stream — without
/// ever holding the whole inflated body in memory. [`extract_csv_from_zip`]
/// reads it whole for the small-object families.
///
/// [`verify`](ZipMemberReader::verify) MUST be called once the stream is fully
/// drained; it checks the inflated length and CRC-32 against the archive's
/// declared values.
pub struct ZipMemberReader<'a> {
    source: ZipMemberSource<'a>,
    hasher: Crc,
    inflated_len: u64,
    declared_uncompressed_len: u64,
    declared_crc: u32,
}

enum ZipMemberSource<'a> {
    Deflate(DeflateDecoder<&'a [u8]>),
    Stored(&'a [u8]),
}

impl ZipMemberReader<'_> {
    /// The member's declared uncompressed length, for sizing a whole-buffer read
    /// (`Vec::with_capacity`).
    pub fn declared_len(&self) -> usize {
        usize::try_from(self.declared_uncompressed_len).unwrap_or(usize::MAX)
    }

    /// Verify the fully drained member against its declared length and CRC-32.
    ///
    /// # Errors
    ///
    /// Returns an error if the inflated byte count or CRC-32 does not match the
    /// archive's declared values (a truncated or corrupt member).
    pub fn verify(&self) -> Result<()> {
        ensure!(
            self.inflated_len == self.declared_uncompressed_len,
            "ZIP member inflated to {} bytes, header declared {}",
            self.inflated_len,
            self.declared_uncompressed_len
        );
        let computed = self.hasher.sum();
        ensure!(
            computed == self.declared_crc,
            "ZIP member CRC-32 mismatch (computed {computed:#010x}, declared {:#010x})",
            self.declared_crc
        );
        Ok(())
    }
}

impl Read for ZipMemberReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = match &mut self.source {
            ZipMemberSource::Deflate(decoder) => decoder.read(buf)?,
            ZipMemberSource::Stored(rest) => rest.read(buf)?,
        };
        self.hasher.update(&buf[..read]);
        self.inflated_len += read as u64;
        Ok(read)
    }
}

/// Open a streaming [`ZipMemberReader`] over the single member of a ZIP archive.
///
/// The archive is a standard ZIP carrying exactly one regular file compressed
/// with DEFLATE (or STORED). The local file header is parsed directly; when the
/// header advertises streamed sizes (general-purpose flag bit 3), the member's
/// sizes and CRC are recovered from the central directory; 64-bit sizes are read
/// from the ZIP64 extra field. The returned reader is positioned at the member's
/// compressed data and verifies CRC-32 + length on [`ZipMemberReader::verify`].
///
/// # Errors
///
/// Returns an error if the signature is wrong, the member extends past the
/// archive, or the compression method is unsupported.
pub fn zip_member_reader(zip_bytes: &[u8]) -> Result<ZipMemberReader<'_>> {
    ensure!(
        zip_bytes.len() >= ZIP_LOCAL_HEADER_LEN,
        "ZIP archive is shorter than a local file header"
    );
    ensure!(
        zip_bytes[0..4] == ZIP_LOCAL_HEADER_SIG,
        "missing ZIP local file header signature"
    );

    let flags = read_u16_le(zip_bytes, 6);
    let method = read_u16_le(zip_bytes, 8);
    let mut crc = read_u32_le(zip_bytes, 14);
    let mut compressed_size = read_u32_le(zip_bytes, 18) as usize;
    let mut uncompressed_size = read_u32_le(zip_bytes, 22) as usize;
    let name_len = read_u16_le(zip_bytes, 26) as usize;
    let extra_len = read_u16_le(zip_bytes, 28) as usize;

    // Streamed entry: the real size/CRC live in the central directory.
    if flags & ZIP_FLAG_DATA_DESCRIPTOR != 0 {
        let (csize, usize_field, central_crc) = central_directory_member(zip_bytes)?;
        compressed_size = csize;
        uncompressed_size = usize_field;
        crc = central_crc;
    } else if compressed_size == ZIP32_SIZE_SENTINEL as usize
        || uncompressed_size == ZIP32_SIZE_SENTINEL as usize
    {
        // ZIP64 member: the 32-bit size fields hold the 0xFFFFFFFF sentinel and
        // the real 64-bit sizes live in the local header's ZIP64 extra field
        // (data.binance.vision monthly archives exceed 4 GiB uncompressed).
        let extra_start = ZIP_LOCAL_HEADER_LEN
            .checked_add(name_len)
            .context("ZIP local header name length overflow")?;
        let extra_end = extra_start
            .checked_add(extra_len)
            .context("ZIP local header extra length overflow")?;
        ensure!(
            extra_end <= zip_bytes.len(),
            "ZIP local header extra field extends past archive end"
        );
        let (uncompressed64, compressed64) = zip64_sizes(
            &zip_bytes[extra_start..extra_end],
            uncompressed_size == ZIP32_SIZE_SENTINEL as usize,
            compressed_size == ZIP32_SIZE_SENTINEL as usize,
        )?;
        if let Some(value) = uncompressed64 {
            uncompressed_size = value;
        }
        if let Some(value) = compressed64 {
            compressed_size = value;
        }
    }

    let data_start = ZIP_LOCAL_HEADER_LEN
        .checked_add(name_len)
        .and_then(|value| value.checked_add(extra_len))
        .context("ZIP local header length overflow")?;
    let data_end = data_start
        .checked_add(compressed_size)
        .context("ZIP member size overflow")?;
    ensure!(
        data_end <= zip_bytes.len(),
        "ZIP member extends past archive end (need {data_end}, have {})",
        zip_bytes.len()
    );
    let compressed = &zip_bytes[data_start..data_end];

    let source = match method {
        ZIP_METHOD_DEFLATE => ZipMemberSource::Deflate(DeflateDecoder::new(compressed)),
        ZIP_METHOD_STORED => ZipMemberSource::Stored(compressed),
        other => bail!("unsupported ZIP compression method {other}"),
    };

    Ok(ZipMemberReader {
        source,
        hasher: Crc::new(),
        inflated_len: 0,
        declared_uncompressed_len: uncompressed_size as u64,
        declared_crc: crc,
    })
}

/// Extract the single CSV member of an OKX `trades`/`candlesticks` ZIP archive
/// and return its decompressed UTF-8 text.
///
/// Reads the whole member through [`zip_member_reader`], verifying CRC-32 and
/// length — the small-object path. Large members (Binance) stream through the
/// reader directly instead of materialising the whole text.
///
/// # Errors
///
/// Returns an error if the archive is malformed, the member extends past the
/// archive, inflation fails, the CRC or length mismatches, or the bytes are not
/// valid UTF-8.
pub fn extract_csv_from_zip(zip_bytes: &[u8]) -> Result<String> {
    let mut reader = zip_member_reader(zip_bytes)?;
    let mut inflated = Vec::with_capacity(reader.declared_len());
    reader
        .read_to_end(&mut inflated)
        .context("inflate ZIP member")?;
    reader.verify()?;
    String::from_utf8(inflated).context("ZIP CSV member is not valid UTF-8")
}

// ===========================================================================
// Shared spec + precision helpers for the no-book families
// ===========================================================================

/// Instrument/venue binding for the OKX `trades` and `candlesticks` families.
///
/// Built from accepted run-spec metadata; never hardcoded in the converter.
/// Price/size precision is supplied as decimal-string increments (config-driven)
/// rather than inferred, so the catalog precision is deterministic across
/// objects of the same instrument regardless of which rows a given object
/// happens to contain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkxInstrumentSpec {
    /// NautilusTrader instrument id, for example `DOGE-USD_UM_XPERP-310404.OKX`.
    pub nt_instrument_id: String,
    /// Venue-native instrument id as it appears in the `instrument_name` column,
    /// used to fence out foreign rows, for example `DOGE-USD_UM_XPERP-310404`.
    pub venue_inst_id: String,
    /// Price tick size as a decimal string, for example `0.00001`. Trailing
    /// zeros are significant; the decimal places set the catalog price precision.
    pub price_increment: String,
    /// Size step as a decimal string, for example `1` or `0.1`. The decimal
    /// places set the catalog size precision.
    pub size_increment: String,
}

impl OkxInstrumentSpec {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("nt_instrument_id", &self.nt_instrument_id),
            ("venue_inst_id", &self.venue_inst_id),
            ("price_increment", &self.price_increment),
            ("size_increment", &self.size_increment),
        ] {
            ensure!(!value.trim().is_empty(), "empty OKX spec field: {name}");
        }
        // The increments must parse as decimals so the derived precision is real.
        Decimal::from_str(&self.price_increment)
            .with_context(|| format!("invalid price_increment {:?}", self.price_increment))?;
        Decimal::from_str(&self.size_increment)
            .with_context(|| format!("invalid size_increment {:?}", self.size_increment))?;
        Ok(())
    }

    fn instrument_id(&self) -> Result<InstrumentId> {
        InstrumentId::from_str(&self.nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {:?}", self.nt_instrument_id))
    }

    fn price_precision(&self) -> u8 {
        increment_decimal_places(&self.price_increment)
    }

    fn size_precision(&self) -> u8 {
        increment_decimal_places(&self.size_increment)
    }
}

/// Decimal places implied by a decimal-string increment (`0.1` -> 1,
/// `0.00001` -> 5, `1` -> 0). Trailing zeros are significant.
#[must_use]
fn increment_decimal_places(increment: &str) -> u8 {
    match increment.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Rescale a decimal string to exactly `precision` places, refusing to silently
/// drop precision the instrument cannot represent.
///
/// OKX `trades`/`candlesticks` CSVs render values at the source's own scale, so
/// an integer-contract size arrives as `"1.0"` and a non-flat candle volume as
/// `"764.0"`. Tolerate that trailing-zero padding by checking only the
/// SIGNIFICANT scale (trailing zeros stripped via [`Decimal::normalize`]):
/// `"1.0"` is admissible at precision 0 (the dropped digit is zero, lossless),
/// while a genuine `"1.05"` is refused at precision 0. This mirrors the shared
/// `catalog_projection::rescaled` convention.
fn rescaled_to(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value)
        .with_context(|| format!("decimal {value:?}"))?
        .round_dp(NT_FIXED_PRECISION);
    ensure!(
        decimal.normalize().scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

fn price_field(value: &str, precision: u8) -> Result<Price> {
    let text = rescaled_to(value, precision)?;
    Price::from_str(&text).map_err(|error| anyhow::anyhow!("invalid price {text:?}: {error}"))
}

fn quantity_field(value: &str, precision: u8) -> Result<Quantity> {
    let text = rescaled_to(value, precision)?;
    Quantity::from_str(&text).map_err(|error| anyhow::anyhow!("invalid size {text:?}: {error}"))
}

/// Convert integer-millisecond exchange time to Unix nanoseconds.
fn millis_to_nanos(raw: &str, label: &str) -> Result<i64> {
    let millis: i64 = raw
        .trim()
        .parse()
        .with_context(|| format!("non-integer {label} {raw:?}"))?;
    millis
        .checked_mul(TRADES_NANOS_PER_MILLISECOND)
        .with_context(|| format!("{label} milliseconds overflow scaling to nanos"))
}

// ===========================================================================
// trades -> NautilusTrader TradeTick
// ===========================================================================

/// Header of an OKX `trades` CSV, in column order.
pub const OKX_TRADES_HEADER: [&str; 6] = [
    "instrument_name",
    "trade_id",
    "side",
    "price",
    "size",
    "created_time",
];

/// Aggressor (taker) side of an OKX trade print.
///
/// The OKX `trades` object records the taker side directly in the `side`
/// column (`buy`/`sell`), so the aggressor is read from the source rather than
/// inferred from a maker flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OkxTradeSide {
    Buy,
    Sell,
}

impl OkxTradeSide {
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "buy" => Ok(Self::Buy),
            "sell" => Ok(Self::Sell),
            other => bail!("unknown OKX trade side token: {other:?}"),
        }
    }

    const fn to_nt(self) -> AggressorSide {
        match self {
            Self::Buy => AggressorSide::Buyer,
            Self::Sell => AggressorSide::Seller,
        }
    }
}

/// One parsed OKX trade print.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkxTradeRow {
    /// Exchange event time in Unix nanoseconds.
    pub event_time: i64,
    /// Native trade id.
    pub trade_id: String,
    pub aggressor_side: OkxTradeSide,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
}

/// Parse an OKX `trades` CSV into validated trade rows, keeping only rows whose
/// `instrument_name` matches `venue_inst_id` and sorting by event time so the
/// projection satisfies NautilusTrader's non-decreasing-timestamp write
/// contract on real (already time-ordered) data.
///
/// # Errors
///
/// Returns an error if the header does not match [`OKX_TRADES_HEADER`], a row is
/// malformed, a field fails to parse, or a price/size is non-positive.
pub fn parse_okx_trades(csv_text: &str, venue_inst_id: &str) -> Result<Vec<OkxTradeRow>> {
    let mut lines = csv_text.lines();
    let header = lines
        .next()
        .context("empty OKX trades csv: missing header")?;
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        columns == OKX_TRADES_HEADER,
        "OKX trades header {columns:?} does not match expected {OKX_TRADES_HEADER:?}"
    );

    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == OKX_TRADES_HEADER.len(),
            "OKX trades row {index} has {} fields, expected {}",
            fields.len(),
            OKX_TRADES_HEADER.len()
        );

        let instrument = fields[0].trim();
        if instrument != venue_inst_id {
            continue;
        }
        let trade_id = fields[1].trim();
        let aggressor_side = OkxTradeSide::parse(fields[2])
            .with_context(|| format!("OKX trades row {index}: invalid side"))?;
        let price_raw = fields[3].trim();
        let size_raw = fields[4].trim();
        let event_time = millis_to_nanos(fields[5], "created_time")
            .with_context(|| format!("OKX trades row {index}"))?;

        ensure!(
            !trade_id.is_empty(),
            "OKX trades row {index}: empty trade id"
        );
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("OKX trades row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("OKX trades row {index}: invalid size {size_raw:?}"))?;
        ensure!(
            price > Decimal::ZERO,
            "OKX trades row {index}: non-positive price"
        );
        ensure!(
            size > Decimal::ZERO,
            "OKX trades row {index}: non-positive size"
        );

        rows.push(OkxTradeRow {
            event_time,
            trade_id: trade_id.to_string(),
            aggressor_side,
            price: price_raw.to_string(),
            size: size_raw.to_string(),
        });
    }

    // OKX archives are already time-ordered, but real data can carry equal
    // timestamps; sort (stable) so the write contract holds without erroring on
    // ties or rare out-of-order rows.
    rows.sort_by_key(|row| row.event_time);
    Ok(rows)
}

/// Convert parsed OKX trade rows into NautilusTrader [`TradeTick`]s at the
/// instrument's configured price/size precision.
///
/// # Errors
///
/// Returns an error if a price/size cannot be represented at the configured
/// precision, a trade id is invalid, or an event time is negative.
pub fn okx_trades_to_trade_ticks(
    rows: &[OkxTradeRow],
    spec: &OkxInstrumentSpec,
) -> Result<Vec<TradeTick>> {
    spec.validate()?;
    let instrument_id = spec.instrument_id()?;
    let price_precision = spec.price_precision();
    let size_precision = spec.size_precision();
    rows.iter()
        .map(|row| {
            let price = price_field(&row.price, price_precision)?;
            let size = quantity_field(&row.size, size_precision)?;
            let trade_id = TradeId::new_checked(&row.trade_id)
                .map_err(|error| anyhow::anyhow!("invalid trade id {:?}: {error}", row.trade_id))?;
            let ts = UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
            Ok(TradeTick::new(
                instrument_id,
                price,
                size,
                row.aggressor_side.to_nt(),
                trade_id,
                ts,
                ts,
            ))
        })
        .collect()
}

/// Result of projecting an OKX no-book family into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkxCatalogProjection {
    pub catalog_root: PathBuf,
    /// The NautilusTrader catalog identifier the data was written under
    /// (instrument id for trades, bar-type string for candlesticks).
    pub nt_identifier: String,
    pub data_type: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
    /// Deterministic SHA-256 hex over the catalog's written data files.
    pub catalog_hash: String,
}

/// Project an OKX `trades` ZIP archive into a NautilusTrader `ParquetDataCatalog`
/// as `TradeTick` data.
///
/// Extracts the CSV member, parses and fences the rows to the spec's instrument,
/// builds [`TradeTick`]s, and writes them with NautilusTrader's own
/// `write_to_parquet`. Returns a deterministic catalog hash.
///
/// # Errors
///
/// Returns an error if extraction, parsing, tick construction, or the catalog
/// write fails, or if `catalog_root` is a non-empty directory.
pub fn project_okx_trades_archive_to_catalog(
    zip_bytes: &[u8],
    spec: &OkxInstrumentSpec,
    catalog_root: &Path,
) -> Result<OkxCatalogProjection> {
    spec.validate()?;
    let csv = extract_csv_from_zip(zip_bytes)?;
    let rows = parse_okx_trades(&csv, &spec.venue_inst_id)?;
    ensure!(
        !rows.is_empty(),
        "no OKX trade rows matched venue_inst_id {:?}",
        spec.venue_inst_id
    );
    let ticks = okx_trades_to_trade_ticks(&rows, spec)?;
    let record_count = ticks.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write OKX trade ticks to catalog")?;

    Ok(OkxCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: spec.nt_instrument_id.clone(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        record_count,
        price_precision: spec.price_precision(),
        size_precision: spec.size_precision(),
        catalog_hash: catalog_hash(catalog_root)?,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected
/// `TradeTick` data back from `catalog_root`.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_trade_ticks(
    catalog_root: &Path,
    nt_instrument_id: &str,
) -> Result<Vec<TradeTick>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<TradeTick>(
            Some(vec![nt_instrument_id.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query OKX trade ticks from catalog")
}

// ===========================================================================
// trades bulk-append path (data-derived precision, no clean-root guard)
// ===========================================================================

/// The decimal-string increment whose fractional length is exactly `precision`
/// (`0 -> "1"`, `1 -> "0.1"`, `5 -> "0.00001"`) — the inverse of
/// [`increment_decimal_places`]. Lets a data-derived precision be expressed as
/// the [`OkxInstrumentSpec`] increment the converter consumes.
#[must_use]
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}

/// Distinct venue-native instrument ids appearing in an OKX `trades` CSV, in
/// first-seen order.
///
/// A staged object is per-selector but can carry more than one dated contract
/// (`...-310404`, `...-310523`) across a roll, so the bulk converter writes one
/// catalog stream per distinct instrument rather than assuming a single one.
///
/// # Errors
///
/// Returns an error if the header does not match [`OKX_TRADES_HEADER`].
pub fn okx_trade_instruments(csv_text: &str) -> Result<Vec<String>> {
    let mut lines = csv_text.lines();
    let header = lines
        .next()
        .context("empty OKX trades csv: missing header")?;
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        columns == OKX_TRADES_HEADER,
        "OKX trades header {columns:?} does not match expected {OKX_TRADES_HEADER:?}"
    );

    let mut seen: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let inst = line.split(',').next().unwrap_or("").trim();
        if !inst.is_empty() && !seen.iter().any(|s| s == inst) {
            seen.push(inst.to_string());
        }
    }
    Ok(seen)
}

/// Build an [`OkxInstrumentSpec`] whose price/size precision is derived from the
/// rows themselves — the maximum decimal places the exchange rendered for this
/// instrument in this object.
///
/// OKX renders every row of a column at the instrument's native tick/lot scale
/// (a price at a `0.1` tick prints `660.0`, not `660`), so the maximum observed
/// scale is stable across objects of the same instrument and is the precision
/// NautilusTrader pins per catalog file — no external instrument universe is
/// needed, and OKX stages none.
///
/// # Errors
///
/// Returns an error if `rows` is empty or a price/size string is not decimal.
pub fn okx_trades_spec_from_rows(
    rows: &[OkxTradeRow],
    venue_inst_id: &str,
) -> Result<OkxInstrumentSpec> {
    ensure!(
        !rows.is_empty(),
        "cannot derive OKX precision from zero rows"
    );
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in rows {
        price_precision = price_precision.max(decimal_places(&row.price)?);
        size_precision = size_precision.max(decimal_places(&row.size)?);
    }
    Ok(OkxInstrumentSpec {
        nt_instrument_id: format!("{venue_inst_id}.{OKX_VENUE}"),
        venue_inst_id: venue_inst_id.to_string(),
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
    })
}

/// One instrument's write summary produced by [`append_okx_trades_archive`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkxAppendSummary {
    pub nt_instrument_id: String,
    pub record_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

/// Append every instrument's trades from one OKX `trades` ZIP object into an
/// already-open [`ParquetDataCatalog`] — the bulk-conversion path.
///
/// Unlike [`project_okx_trades_archive_to_catalog`] (the hermetic single-object
/// proof harness, which refuses a dirty root), this appends into a shared,
/// possibly-S3 catalog: it relies on NautilusTrader's own per-instrument,
/// per-time-range file naming and skip-on-existing so many objects flow into one
/// catalog. Precision is derived from each instrument's own rows
/// ([`okx_trades_spec_from_rows`]). Returns one summary per distinct instrument
/// written.
///
/// # Errors
///
/// Returns an error if extraction, parsing, tick construction, or the catalog
/// write fails, or if the object yields no instruments.
pub fn append_okx_trades_archive(
    zip_bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<OkxAppendSummary>> {
    let csv = extract_csv_from_zip(zip_bytes)?;
    let instruments = okx_trade_instruments(&csv)?;
    let mut summaries = Vec::new();
    for venue_inst_id in instruments {
        let rows = parse_okx_trades(&csv, &venue_inst_id)?;
        if rows.is_empty() {
            continue;
        }
        let spec = okx_trades_spec_from_rows(&rows, &venue_inst_id)?;
        let ticks = okx_trades_to_trade_ticks(&rows, &spec)?;
        let summary = OkxAppendSummary {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            record_count: ticks.len(),
            price_precision: spec.price_precision(),
            size_precision: spec.size_precision(),
        };
        catalog
            .write_to_parquet(ticks, None, None, None)
            .with_context(|| format!("append OKX trade ticks for {venue_inst_id}"))?;
        summaries.push(summary);
    }
    ensure!(
        !summaries.is_empty(),
        "OKX trades object yielded no instruments"
    );
    Ok(summaries)
}

// ===========================================================================
// candlesticks -> NautilusTrader Bar
// ===========================================================================

/// Header of an OKX `candlesticks` CSV, in column order.
pub const OKX_CANDLES_HEADER: [&str; 10] = [
    "instrument_name",
    "open",
    "high",
    "low",
    "close",
    "vol",
    "vol_ccy",
    "vol_quote",
    "open_time",
    "confirm",
];

/// Bar specification for OKX candlesticks supplied by the caller from the
/// staged interval (for example a 1-minute candle -> step 1,
/// [`BarAggregation::Minute`]). Candles are aggregated by the exchange, so they
/// replay as `EXTERNAL`-sourced, `LAST`-price bars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkxBarSpec {
    pub step: usize,
    pub aggregation: BarAggregation,
}

impl OkxBarSpec {
    fn to_bar_type(self, instrument_id: InstrumentId) -> Result<BarType> {
        let step = NonZeroUsize::new(self.step).context("OKX bar step must be positive")?;
        let spec = BarSpecification::new(step.get(), self.aggregation, PriceType::Last);
        Ok(BarType::new(
            instrument_id,
            spec,
            AggregationSource::External,
        ))
    }
}

/// One parsed OKX candle (1 row of the candlesticks CSV).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkxCandleRow {
    /// Bar open (event) timestamp in Unix nanoseconds.
    pub open_time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    /// Exact source contract-volume string (`vol` column).
    pub volume: String,
}

/// Parse an OKX `candlesticks` CSV into validated candle rows, keeping only rows
/// whose `instrument_name` matches `venue_inst_id` and sorting by open time so
/// the projection satisfies NautilusTrader's non-decreasing-timestamp contract.
///
/// Only confirmed candles (`confirm == 1`) are kept; an in-progress candle
/// (`confirm == 0`) is not a settled bar and is dropped.
///
/// # Errors
///
/// Returns an error if the header does not match [`OKX_CANDLES_HEADER`], a row
/// is malformed, a field fails to parse, the OHLC invariant is violated, or
/// volume is negative.
pub fn parse_okx_candlesticks(csv_text: &str, venue_inst_id: &str) -> Result<Vec<OkxCandleRow>> {
    let mut lines = csv_text.lines();
    let header = lines
        .next()
        .context("empty OKX candlesticks csv: missing header")?;
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        columns == OKX_CANDLES_HEADER,
        "OKX candlesticks header {columns:?} does not match expected {OKX_CANDLES_HEADER:?}"
    );

    let mut rows = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').collect();
        ensure!(
            fields.len() == OKX_CANDLES_HEADER.len(),
            "OKX candlesticks row {index} has {} fields, expected {}",
            fields.len(),
            OKX_CANDLES_HEADER.len()
        );

        let instrument = fields[0].trim();
        if instrument != venue_inst_id {
            continue;
        }
        // confirm == 0 marks an in-progress candle; only settled bars project.
        let confirm = fields[9].trim();
        if confirm == "0" {
            continue;
        }
        ensure!(
            confirm == "1",
            "OKX candlesticks row {index}: unexpected confirm flag {confirm:?}"
        );

        let open_raw = fields[1].trim();
        let high_raw = fields[2].trim();
        let low_raw = fields[3].trim();
        let close_raw = fields[4].trim();
        let volume_raw = fields[5].trim();
        let open_time = millis_to_nanos(fields[8], "open_time")
            .with_context(|| format!("OKX candlesticks row {index}"))?;

        let open: Decimal = open_raw
            .parse()
            .with_context(|| format!("OKX candlesticks row {index}: invalid open {open_raw:?}"))?;
        let high: Decimal = high_raw
            .parse()
            .with_context(|| format!("OKX candlesticks row {index}: invalid high {high_raw:?}"))?;
        let low: Decimal = low_raw
            .parse()
            .with_context(|| format!("OKX candlesticks row {index}: invalid low {low_raw:?}"))?;
        let close: Decimal = close_raw.parse().with_context(|| {
            format!("OKX candlesticks row {index}: invalid close {close_raw:?}")
        })?;
        let volume: Decimal = volume_raw
            .parse()
            .with_context(|| format!("OKX candlesticks row {index}: invalid vol {volume_raw:?}"))?;

        ensure!(
            open > Decimal::ZERO,
            "OKX candlesticks row {index}: non-positive open"
        );
        ensure!(
            low > Decimal::ZERO,
            "OKX candlesticks row {index}: non-positive low"
        );
        ensure!(
            volume >= Decimal::ZERO,
            "OKX candlesticks row {index}: negative volume"
        );
        // NautilusTrader's `Bar::new_checked` re-asserts this on the rounded
        // prices; checking the source decimals fails loud before any rounding.
        ensure!(
            high >= open && high >= low && high >= close,
            "OKX candlesticks row {index}: high {high} is not the maximum (o={open} l={low} c={close})"
        );
        ensure!(
            low <= open && low <= close,
            "OKX candlesticks row {index}: low {low} is not the minimum (o={open} c={close})"
        );

        rows.push(OkxCandleRow {
            open_time,
            open: open_raw.to_string(),
            high: high_raw.to_string(),
            low: low_raw.to_string(),
            close: close_raw.to_string(),
            volume: volume_raw.to_string(),
        });
    }

    rows.sort_by_key(|row| row.open_time);
    Ok(rows)
}

/// Convert parsed OKX candle rows into NautilusTrader [`Bar`]s under the
/// configured bar type at the instrument's price/size precision.
///
/// # Errors
///
/// Returns an error if an OHLCV value cannot be represented at the configured
/// precision or fails NautilusTrader's `Bar::new_checked` OHLC checks.
pub fn okx_candlesticks_to_bars(
    rows: &[OkxCandleRow],
    spec: &OkxInstrumentSpec,
    bar_spec: OkxBarSpec,
) -> Result<Vec<Bar>> {
    spec.validate()?;
    let instrument_id = spec.instrument_id()?;
    let bar_type = bar_spec.to_bar_type(instrument_id)?;
    let price_precision = spec.price_precision();
    let size_precision = spec.size_precision();
    rows.iter()
        .map(|row| {
            let open = price_field(&row.open, price_precision)?;
            let high = price_field(&row.high, price_precision)?;
            let low = price_field(&row.low, price_precision)?;
            let close = price_field(&row.close, price_precision)?;
            let volume = quantity_field(&row.volume, size_precision)?;
            let ts = UnixNanos::from(u64::try_from(row.open_time).context("negative open_time")?);
            Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts)
                .context("build OKX bar")
        })
        .collect()
}

/// The NautilusTrader bar-type string for an OKX candle stream, used as the
/// catalog identifier the bars are written under.
///
/// # Errors
///
/// Returns an error if the instrument id is invalid or the bar step is zero.
pub fn okx_bar_type_string(spec: &OkxInstrumentSpec, bar_spec: OkxBarSpec) -> Result<String> {
    let instrument_id = spec.instrument_id()?;
    Ok(bar_spec.to_bar_type(instrument_id)?.to_string())
}

/// Project an OKX `candlesticks` ZIP archive into a NautilusTrader
/// `ParquetDataCatalog` as `Bar` data.
///
/// Extracts the CSV member, parses and fences the rows to the spec's instrument,
/// builds [`Bar`]s under the caller-supplied bar spec, and writes them with
/// NautilusTrader's own `write_to_parquet`. Returns a deterministic catalog hash.
///
/// # Errors
///
/// Returns an error if extraction, parsing, bar construction, or the catalog
/// write fails, or if `catalog_root` is a non-empty directory.
pub fn project_okx_candlesticks_archive_to_catalog(
    zip_bytes: &[u8],
    spec: &OkxInstrumentSpec,
    bar_spec: OkxBarSpec,
    catalog_root: &Path,
) -> Result<OkxCatalogProjection> {
    spec.validate()?;
    let csv = extract_csv_from_zip(zip_bytes)?;
    let rows = parse_okx_candlesticks(&csv, &spec.venue_inst_id)?;
    ensure!(
        !rows.is_empty(),
        "no OKX candle rows matched venue_inst_id {:?}",
        spec.venue_inst_id
    );
    let bars = okx_candlesticks_to_bars(&rows, spec, bar_spec)?;
    let record_count = bars.len();

    assert_clean_catalog_root(catalog_root)?;
    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write OKX bars to catalog")?;

    Ok(OkxCatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: okx_bar_type_string(spec, bar_spec)?,
        data_type: NT_DATA_TYPE_BAR.to_string(),
        record_count,
        price_precision: spec.price_precision(),
        size_precision: spec.size_precision(),
        catalog_hash: catalog_hash(catalog_root)?,
    })
}

/// Prove the resolved NautilusTrader dependency can read the projected `Bar`
/// data back from `catalog_root` by its bar-type identifier.
///
/// # Errors
///
/// Returns an error if the catalog query fails.
pub fn read_back_bars(catalog_root: &Path, bar_type: &str) -> Result<Vec<Bar>> {
    let mut catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .query_typed_data::<Bar>(
            Some(vec![bar_type.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query OKX bars from catalog")
}

// ===========================================================================
// candlesticks bulk-append path (data-derived precision + bar interval,
// no clean-root guard)
// ===========================================================================

/// Fixed-duration NautilusTrader bar units, longest first, paired with their
/// millisecond length. Used to express an observed candle interval as the
/// largest unit that divides it evenly. Calendar-variable units
/// ([`BarAggregation::Month`]/[`BarAggregation::Year`]) are deliberately
/// excluded: their millisecond length is not constant, so a uniform ms gap
/// cannot honestly prove one — such an interval fails loud instead.
const OKX_BAR_UNITS_MS: [(BarAggregation, u64); 5] = [
    (BarAggregation::Week, 7 * 24 * 60 * 60 * 1_000),
    (BarAggregation::Day, 24 * 60 * 60 * 1_000),
    (BarAggregation::Hour, 60 * 60 * 1_000),
    (BarAggregation::Minute, 60 * 1_000),
    (BarAggregation::Second, 1_000),
];

/// Distinct venue-native instrument ids appearing in an OKX `candlesticks` CSV,
/// in first-seen order.
///
/// Like [`okx_trade_instruments`], a staged candlesticks object is per-selector
/// but can carry more than one dated contract across a roll, so the bulk
/// converter writes one catalog bar stream per distinct instrument.
///
/// # Errors
///
/// Returns an error if the header does not match [`OKX_CANDLES_HEADER`].
pub fn okx_candle_instruments(csv_text: &str) -> Result<Vec<String>> {
    let mut lines = csv_text.lines();
    let header = lines
        .next()
        .context("empty OKX candlesticks csv: missing header")?;
    let columns: Vec<&str> = header.split(',').map(str::trim).collect();
    ensure!(
        columns == OKX_CANDLES_HEADER,
        "OKX candlesticks header {columns:?} does not match expected {OKX_CANDLES_HEADER:?}"
    );

    let mut seen: Vec<String> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let inst = line.split(',').next().unwrap_or("").trim();
        if !inst.is_empty() && !seen.iter().any(|s| s == inst) {
            seen.push(inst.to_string());
        }
    }
    Ok(seen)
}

/// Build an [`OkxInstrumentSpec`] for a candle stream whose price/size precision
/// is derived from the rows themselves — the maximum decimal places the exchange
/// rendered across the OHLC prices and the contract volume.
///
/// As with [`okx_trades_spec_from_rows`], OKX renders every candle field at the
/// instrument's native tick/lot scale, so the maximum observed scale is stable
/// across objects of the same instrument and is the precision NautilusTrader
/// pins per catalog file. No external instrument universe is needed (OKX stages
/// none).
///
/// # Errors
///
/// Returns an error if `rows` is empty or an OHLCV string is not decimal.
pub fn okx_candles_spec_from_rows(
    rows: &[OkxCandleRow],
    venue_inst_id: &str,
) -> Result<OkxInstrumentSpec> {
    ensure!(
        !rows.is_empty(),
        "cannot derive OKX candle precision from zero rows"
    );
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in rows {
        for price in [&row.open, &row.high, &row.low, &row.close] {
            price_precision = price_precision.max(decimal_places(price)?);
        }
        size_precision = size_precision.max(decimal_places(&row.volume)?);
    }
    Ok(OkxInstrumentSpec {
        nt_instrument_id: format!("{venue_inst_id}.{OKX_VENUE}"),
        venue_inst_id: venue_inst_id.to_string(),
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
    })
}

/// Express a fixed millisecond interval as the NautilusTrader `(step, unit)` pair
/// using the largest fixed-duration unit that divides it evenly (60_000 ms ->
/// step 1 [`BarAggregation::Minute`]; 300_000 ms -> step 5 minute; 3_600_000 ms
/// -> step 1 [`BarAggregation::Hour`]).
///
/// # Errors
///
/// Returns an error if `interval_ms` is zero or is not an exact multiple of any
/// fixed-duration unit down to one second (sub-second or calendar-variable
/// candle intervals are not honestly representable from a uniform ms gap).
fn bar_spec_from_interval_ms(interval_ms: u64) -> Result<OkxBarSpec> {
    ensure!(interval_ms > 0, "candle interval is zero");
    for (aggregation, unit_ms) in OKX_BAR_UNITS_MS {
        if interval_ms.is_multiple_of(unit_ms) {
            let step = usize::try_from(interval_ms / unit_ms).context("candle step overflow")?;
            return Ok(OkxBarSpec { step, aggregation });
        }
    }
    bail!(
        "OKX candle interval {interval_ms} ms is not a whole number of seconds; \
         cannot derive a bar unit"
    )
}

/// Derive the [`OkxBarSpec`] (step + time unit) from a set of candle `open_time`
/// values (Unix nanoseconds).
///
/// OKX stages no interval in the object key or filename, so the bar period is
/// recovered from the data: the interval is the smallest positive gap between
/// consecutive distinct bar-open timestamps, and every gap must be an exact
/// positive multiple of it (a larger gap is a missing bar, never a different
/// period). Fewer than two distinct opens cannot prove a period and fails loud.
///
/// The caller controls the SCOPE of `open_times`. The bulk candlesticks path
/// passes the union of every instrument's opens in one object, because the
/// period is a per-object property of the vendor file's source granularity (an
/// illiquid strike that traded in a single minute cannot prove a period on its
/// own but inherits the object's). The single-stream [`okx_bar_spec_from_rows`]
/// passes one instrument's opens. The input need not be sorted.
///
/// # Errors
///
/// Returns an error if fewer than two distinct bar-open times are present, a gap
/// is not a multiple of the base interval, or the interval is not representable
/// as a fixed-duration NautilusTrader bar unit.
pub fn okx_bar_spec_from_open_times(open_times: &[i64]) -> Result<OkxBarSpec> {
    let mut times: Vec<i64> = open_times.to_vec();
    times.sort_unstable();
    times.dedup();
    ensure!(
        times.len() >= 2,
        "cannot derive OKX candle interval from fewer than two distinct bar-open times"
    );

    let mut gaps: Vec<u64> = Vec::with_capacity(times.len() - 1);
    for window in times.windows(2) {
        let delta = window[1]
            .checked_sub(window[0])
            .context("bar-open time underflow")?;
        let delta = u64::try_from(delta).context("negative bar-open gap")?;
        ensure!(delta > 0, "duplicate bar-open time survived dedup");
        gaps.push(delta);
    }

    let base = *gaps.iter().min().expect("at least one gap");
    // The base nanosecond interval scaled to milliseconds for unit selection.
    let base_ms = base
        .checked_div(NANOS_PER_MILLISECOND)
        .filter(|_| base.is_multiple_of(NANOS_PER_MILLISECOND))
        .context("candle interval is not a whole number of milliseconds")?;
    for gap in &gaps {
        ensure!(
            gap.is_multiple_of(base),
            "OKX candle gaps are not multiples of the base interval \
             ({gap} ns is not a multiple of {base} ns)"
        );
    }
    bar_spec_from_interval_ms(base_ms)
}

/// Derive the [`OkxBarSpec`] from one instrument's candle rows' own `open_time`
/// spacing — the single-stream path. Delegates to
/// [`okx_bar_spec_from_open_times`] so the period-derivation rule lives in one
/// place.
///
/// # Errors
///
/// See [`okx_bar_spec_from_open_times`].
pub fn okx_bar_spec_from_rows(rows: &[OkxCandleRow]) -> Result<OkxBarSpec> {
    let open_times: Vec<i64> = rows.iter().map(|row| row.open_time).collect();
    okx_bar_spec_from_open_times(&open_times)
}

/// Append every instrument's candles from one OKX `candlesticks` ZIP object into
/// an already-open [`ParquetDataCatalog`] — the bulk-conversion path.
///
/// Mirrors [`append_okx_trades_archive`]: extracts the object's CSV from the ZIP
/// envelope and delegates the per-instrument projection to
/// [`append_okx_candlesticks_csv`]. Refuses no dirty root — many objects flow
/// into one shared catalog. Returns one summary per distinct instrument written.
///
/// # Errors
///
/// Returns an error if extraction, parsing, interval/precision derivation, bar
/// construction, or the catalog write fails, or if the object yields no
/// instruments.
pub fn append_okx_candlesticks_archive(
    zip_bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<OkxAppendSummary>> {
    let csv = extract_csv_from_zip(zip_bytes)?;
    append_okx_candlesticks_csv(&csv, catalog)
}

/// Append every instrument's candles from one already-extracted OKX
/// `candlesticks` CSV into an open [`ParquetDataCatalog`] — the projection half
/// of [`append_okx_candlesticks_archive`], split out so the per-instrument
/// projection is exercised directly from CSV text (without the ZIP envelope) in
/// tests.
///
/// Enumerates the object's distinct instruments, derives each one's price/size
/// precision from its own rows ([`okx_candles_spec_from_rows`]), and derives the
/// bar period ONCE for the whole object from the union of every instrument's
/// `open_time`s ([`okx_bar_spec_from_open_times`]) — the period is a per-object
/// property of the vendor file's source granularity, so an illiquid strike that
/// traded in a single minute inherits the object's proven period instead of
/// aborting the object. Builds [`Bar`]s and appends them with NautilusTrader's
/// own per-bar-type, per-time-range file naming. Returns one summary per
/// distinct instrument written.
///
/// # Errors
///
/// Returns an error if parsing, interval/precision derivation, bar
/// construction, or the catalog write fails, or if the object yields no
/// instruments.
pub fn append_okx_candlesticks_csv(
    csv: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<OkxAppendSummary>> {
    let instruments = okx_candle_instruments(csv)?;
    // Parse every instrument's rows once, accumulating the union of all
    // `open_time`s. Precision is per-instrument (each contract renders at its
    // own native tick/lot scale); the bar period is per-object and is derived
    // from the union below so a single-bar strike does not abort the object.
    let mut parsed: Vec<(Vec<OkxCandleRow>, OkxInstrumentSpec)> = Vec::new();
    let mut object_open_times: Vec<i64> = Vec::new();
    for venue_inst_id in instruments {
        let rows = parse_okx_candlesticks(csv, &venue_inst_id)?;
        if rows.is_empty() {
            continue;
        }
        let spec = okx_candles_spec_from_rows(&rows, &venue_inst_id)?;
        object_open_times.extend(rows.iter().map(|row| row.open_time));
        parsed.push((rows, spec));
    }
    ensure!(
        !parsed.is_empty(),
        "OKX candlesticks object yielded no instruments"
    );
    let bar_spec = okx_bar_spec_from_open_times(&object_open_times)?;

    let mut summaries = Vec::with_capacity(parsed.len());
    for (rows, spec) in parsed {
        let bars = okx_candlesticks_to_bars(&rows, &spec, bar_spec)?;
        let summary = OkxAppendSummary {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            record_count: bars.len(),
            price_precision: spec.price_precision(),
            size_precision: spec.size_precision(),
        };
        catalog
            .write_to_parquet(bars, None, None, None)
            .with_context(|| format!("append OKX bars for {}", spec.venue_inst_id))?;
        summaries.push(summary);
    }
    Ok(summaries)
}

// ===========================================================================
// order_book_400 bulk-append path (data-derived precision via
// resolve_precisions, no clean-root guard)
// ===========================================================================

/// Distinct venue-native instrument ids (`instId`) appearing in an OKX
/// `order_book_400` JSONL stream, in first-seen order.
///
/// A staged order-book object is per-selector but can carry more than one dated
/// contract across a roll, so the bulk converter writes one catalog
/// `OrderBookDelta` stream per distinct instrument.
///
/// # Errors
///
/// Returns an error if any non-blank line is not a valid OKX book message.
pub fn okx_book_instruments(jsonl: &str) -> Result<Vec<String>> {
    let mut seen: Vec<String> = Vec::new();
    for (index, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let message: OkxBookMessage = serde_json::from_str(line)
            .with_context(|| format!("parse OKX book message on line {}", index + 1))?;
        if !seen.iter().any(|s| s == &message.inst_id) {
            seen.push(message.inst_id);
        }
    }
    Ok(seen)
}

/// Build an [`OkxBookSpec`] for a book stream from its venue-native instrument
/// id; the NautilusTrader catalog id is the venue id with the [`OKX_VENUE`]
/// suffix.
///
/// Book precision is not carried here: [`okx_book_messages_to_deltas`] already
/// data-derives it via [`resolve_precisions`] (the maximum decimal places across
/// every level), so the spec only needs the identity binding — no instrument
/// universe is read.
#[must_use]
pub fn okx_book_spec_for_instrument(venue_inst_id: &str) -> OkxBookSpec {
    OkxBookSpec {
        nt_instrument_id: format!("{venue_inst_id}.{OKX_VENUE}"),
        venue_inst_id: venue_inst_id.to_string(),
    }
}

/// Append every instrument's order-book deltas from one OKX `order_book_400`
/// archive into an already-open [`ParquetDataCatalog`] — the bulk-conversion
/// path.
///
/// Mirrors [`append_okx_trades_archive`]: enumerates the archive's distinct
/// instruments, builds each one's [`OrderBookDelta`]s with precision derived
/// from its own levels (via [`okx_book_messages_to_deltas`]/[`resolve_precisions`]),
/// and appends them with NautilusTrader's own per-instrument, per-time-range
/// file naming. Refuses no dirty root — many objects flow into one shared
/// catalog. Returns one summary per distinct instrument written.
///
/// # Errors
///
/// Returns an error if extraction, parsing, delta construction, or the catalog
/// write fails, or if the archive yields no instruments.
pub fn append_okx_book_archive(
    gz_bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<OkxAppendSummary>> {
    let jsonl = extract_jsonl_from_archive(gz_bytes)?;
    let instruments = okx_book_instruments(&jsonl)?;
    let mut summaries = Vec::new();
    for venue_inst_id in instruments {
        let messages = parse_okx_book_messages(&jsonl, &venue_inst_id)?;
        if messages.is_empty() {
            continue;
        }
        let (price_precision, size_precision) = resolve_precisions(&messages)?;
        let spec = okx_book_spec_for_instrument(&venue_inst_id);
        let deltas = okx_book_messages_to_deltas(&messages, &spec)?;
        let summary = OkxAppendSummary {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            record_count: deltas.len(),
            price_precision,
            size_precision,
        };
        catalog
            .write_to_parquet(deltas, None, None, None)
            .with_context(|| format!("append OKX order book deltas for {venue_inst_id}"))?;
        summaries.push(summary);
    }
    ensure!(
        !summaries.is_empty(),
        "OKX order_book_400 object yielded no instruments"
    );
    Ok(summaries)
}

/// Fail closed on a dirty catalog root. NautilusTrader's `write_to_parquet`
/// skips writing when a file for the same identifier/interval already exists, so
/// projecting into a non-empty root could silently read back stale data.
fn assert_clean_catalog_root(catalog_root: &Path) -> Result<()> {
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display()
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_INST: &str = "BNB-USD_UM_XPERP-310523";

    fn spec() -> OkxBookSpec {
        OkxBookSpec {
            nt_instrument_id: "BNB-USD_UM_XPERP-310523.OKX".to_string(),
            venue_inst_id: SAMPLE_INST.to_string(),
        }
    }

    fn sample_jsonl() -> String {
        // One update (ts 1), one snapshot with two levels per side (ts 2),
        // one update with a zero-size delete (ts 3), plus a foreign-instrument
        // line that must be fenced out.
        [
            r#"{"instId":"BNB-USD_UM_XPERP-310523","action":"update","ts":"1","asks":[["650.0","10000","1"]],"bids":[]}"#,
            r#"{"instId":"SOME-OTHER-INST","action":"update","ts":"1","asks":[["1.0","1","1"]],"bids":[]}"#,
            r#"{"instId":"BNB-USD_UM_XPERP-310523","action":"snapshot","ts":"2","asks":[["643.3","1","1"],["643.5","5","1"]],"bids":[["643.0","2","1"],["642.8","3","1"]]}"#,
            r#"{"instId":"BNB-USD_UM_XPERP-310523","action":"update","ts":"3","asks":[["650.0","0","0"]],"bids":[["643.0","7","1"]]}"#,
        ]
        .join("\n")
    }

    #[test]
    fn decimal_places_reads_precision() {
        assert_eq!(decimal_places("643.3").unwrap(), 1);
        assert_eq!(decimal_places("5995").unwrap(), 0);
        assert_eq!(decimal_places("0.0001").unwrap(), 4);
    }

    #[test]
    fn parse_fences_foreign_instrument() {
        let messages = parse_okx_book_messages(&sample_jsonl(), SAMPLE_INST).unwrap();
        // The SOME-OTHER-INST line is dropped.
        assert_eq!(messages.len(), 3);
        assert!(messages.iter().all(|m| m.inst_id == SAMPLE_INST));
    }

    #[test]
    fn maps_actions_per_okx_semantics() {
        let messages = parse_okx_book_messages(&sample_jsonl(), SAMPLE_INST).unwrap();
        let deltas = okx_book_messages_to_deltas(&messages, &spec()).unwrap();

        // update(ts1): 1 ask Update.
        // snapshot(ts2): Clear + 2 bid Add + 2 ask Add = 5.
        // update(ts3): 1 bid Update + 1 ask Delete (sz==0) = 2.
        assert_eq!(deltas.len(), 1 + 5 + 2);

        assert_eq!(deltas[0].action, BookAction::Update);
        assert_eq!(deltas[0].order.side, OrderSide::Sell);

        // Snapshot group opens with a Clear.
        assert_eq!(deltas[1].action, BookAction::Clear);
        // Bids come before asks within a message; both are Add in a snapshot.
        assert_eq!(deltas[2].action, BookAction::Add);
        assert_eq!(deltas[2].order.side, OrderSide::Buy);
        assert_eq!(deltas[4].order.side, OrderSide::Sell);
        assert_eq!(deltas[4].action, BookAction::Add);

        // Final update message (indices 6,7): bid Update then ask Delete
        // (bids are emitted before asks within a message).
        assert_eq!(deltas[6].action, BookAction::Update);
        assert_eq!(deltas[6].order.side, OrderSide::Buy);
        assert_eq!(deltas[7].action, BookAction::Delete);
        assert_eq!(deltas[7].order.side, OrderSide::Sell);

        // Uniform precision on every level-bearing delta: prices have one
        // decimal, sizes zero. Clear deltas carry NautilusTrader's NULL_ORDER
        // (precision 0) by design, so they are excluded.
        let level_deltas = || deltas.iter().filter(|d| d.action != BookAction::Clear);
        assert!(level_deltas().all(|d| d.order.price.precision == 1));
        assert!(level_deltas().all(|d| d.order.size.precision == 0));

        // L2 sentinel order id everywhere (NULL_ORDER also uses id 0).
        assert!(deltas.iter().all(|d| d.order.order_id == L2_ORDER_ID));

        // Snapshot deltas carry the snapshot flag; updates do not.
        assert_eq!(deltas[1].flags, RecordFlag::F_SNAPSHOT as u8);
        assert_eq!(deltas[0].flags, 0);
    }

    #[test]
    fn ms_timestamps_scale_to_nanos() {
        let messages = parse_okx_book_messages(&sample_jsonl(), SAMPLE_INST).unwrap();
        let deltas = okx_book_messages_to_deltas(&messages, &spec()).unwrap();
        assert_eq!(deltas[0].ts_event.as_u64(), NANOS_PER_MILLISECOND);
        // The snapshot group is at ts 2 ms.
        assert_eq!(deltas[1].ts_event.as_u64(), 2 * NANOS_PER_MILLISECOND);
    }

    #[test]
    fn out_of_order_messages_are_rejected() {
        let jsonl = [
            r#"{"instId":"BNB-USD_UM_XPERP-310523","action":"update","ts":"5","asks":[["1.0","1","1"]],"bids":[]}"#,
            r#"{"instId":"BNB-USD_UM_XPERP-310523","action":"update","ts":"2","asks":[["1.0","1","1"]],"bids":[]}"#,
        ]
        .join("\n");
        let messages = parse_okx_book_messages(&jsonl, SAMPLE_INST).unwrap();
        let err = okx_book_messages_to_deltas(&messages, &spec()).unwrap_err();
        assert!(err.to_string().contains("out of order"), "{err}");
    }

    // -----------------------------------------------------------------------
    // trades / candlesticks unit tests
    // -----------------------------------------------------------------------

    const TRADE_INST: &str = "DOGE-USD_UM_XPERP-310404";

    fn trade_spec() -> OkxInstrumentSpec {
        OkxInstrumentSpec {
            nt_instrument_id: "DOGE-USD_UM_XPERP-310404.OKX".to_string(),
            venue_inst_id: TRADE_INST.to_string(),
            price_increment: "0.00001".to_string(),
            size_increment: "1".to_string(),
        }
    }

    const SAMPLE_TRADES_CSV: &str = "instrument_name,trade_id,side,price,size,created_time\n\
        DOGE-USD_UM_XPERP-310404,717,buy,0.09552,1.0,1776184215764\n\
        SOME-OTHER-INST,9,sell,1.0,1.0,1776184215764\n\
        DOGE-USD_UM_XPERP-310404,718,sell,0.09554,2.0,1776184215765\n";

    #[test]
    fn trades_parse_fences_foreign_instrument_and_maps_side() {
        let rows = parse_okx_trades(SAMPLE_TRADES_CSV, TRADE_INST).unwrap();
        assert_eq!(rows.len(), 2, "foreign instrument row dropped");
        assert_eq!(rows[0].aggressor_side, OkxTradeSide::Buy);
        assert_eq!(rows[1].aggressor_side, OkxTradeSide::Sell);
        // milliseconds -> nanoseconds.
        assert_eq!(
            rows[0].event_time,
            1_776_184_215_764 * TRADES_NANOS_PER_MILLISECOND
        );
    }

    #[test]
    fn trades_reject_bad_header() {
        let bad = "ts,id,side,px,sz\n1,1,buy,1,1\n";
        let err = parse_okx_trades(bad, TRADE_INST).unwrap_err();
        assert!(err.to_string().contains("header"), "{err}");
    }

    #[test]
    fn trades_reject_unknown_side() {
        let bad = "instrument_name,trade_id,side,price,size,created_time\n\
            DOGE-USD_UM_XPERP-310404,1,hold,0.1,1,1776184215764\n";
        let err = parse_okx_trades(bad, TRADE_INST).unwrap_err();
        assert!(err.to_string().contains("side"), "{err}");
    }

    #[test]
    fn trades_map_to_trade_ticks_with_configured_precision() {
        let rows = parse_okx_trades(SAMPLE_TRADES_CSV, TRADE_INST).unwrap();
        let ticks = okx_trades_to_trade_ticks(&rows, &trade_spec()).unwrap();
        assert_eq!(ticks.len(), 2);
        assert!(ticks.iter().all(|t| t.price.precision == 5));
        assert!(ticks.iter().all(|t| t.size.precision == 0));
        // The source sizes are `"1.0"`/`"2.0"`; at an integer-contract size
        // precision (0) the trailing `.0` is lossless padding, so the values
        // survive as `1`/`2` rather than being rejected.
        assert_eq!(ticks[0].size.as_decimal(), Decimal::from(1));
        assert_eq!(ticks[1].size.as_decimal(), Decimal::from(2));
        assert_eq!(ticks[0].aggressor_side, AggressorSide::Buyer);
        assert_eq!(ticks[1].aggressor_side, AggressorSide::Seller);
        assert!(ticks.windows(2).all(|w| w[0].ts_init <= w[1].ts_init));
    }

    #[test]
    fn rescale_tolerates_trailing_zero_but_rejects_subprecision() {
        // Trailing-zero padding is dropped losslessly.
        assert_eq!(rescaled_to("1.0", 0).unwrap(), "1");
        assert_eq!(rescaled_to("764.0", 0).unwrap(), "764");
        assert_eq!(rescaled_to("636.50", 1).unwrap(), "636.5");
        // A genuine sub-precision digit is refused, never silently rounded.
        let err = rescaled_to("1.05", 0).expect_err("sub-precision must be refused");
        assert!(err.to_string().contains("more precision"), "{err}");
    }

    const CANDLE_INST: &str = "BNB-USD_UM_XPERP-310523";

    fn candle_spec() -> OkxInstrumentSpec {
        OkxInstrumentSpec {
            nt_instrument_id: "BNB-USD_UM_XPERP-310523.OKX".to_string(),
            venue_inst_id: CANDLE_INST.to_string(),
            price_increment: "0.1".to_string(),
            size_increment: "1".to_string(),
        }
    }

    fn minute_bar() -> OkxBarSpec {
        OkxBarSpec {
            step: 1,
            aggregation: BarAggregation::Minute,
        }
    }

    const SAMPLE_CANDLES_CSV: &str = "instrument_name,open,high,low,close,vol,vol_ccy,vol_quote,open_time,confirm\n\
        BNB-USD_UM_XPERP-310523,649.5,649.5,636.5,636.5,2,0.02,12.86,1779265080000,1\n\
        OTHER-INST,1.0,1.0,1.0,1.0,0,0,0,1779265080000,1\n\
        BNB-USD_UM_XPERP-310523,636.5,640.0,636.5,640.0,5,0.05,30.0,1779265140000,1\n\
        BNB-USD_UM_XPERP-310523,640.0,640.0,640.0,640.0,0,0,0,1779265200000,0\n";

    #[test]
    fn candles_parse_fences_and_drops_unconfirmed() {
        let rows = parse_okx_candlesticks(SAMPLE_CANDLES_CSV, CANDLE_INST).unwrap();
        // Foreign instrument dropped; in-progress (confirm=0) candle dropped.
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].open_time,
            1_779_265_080_000 * TRADES_NANOS_PER_MILLISECOND
        );
        assert_eq!(rows[0].high, "649.5");
        assert_eq!(rows[0].volume, "2");
    }

    #[test]
    fn candles_reject_ohlc_violation() {
        // high < low: invalid bar.
        let bad = "instrument_name,open,high,low,close,vol,vol_ccy,vol_quote,open_time,confirm\n\
            BNB-USD_UM_XPERP-310523,1.0,0.9,1.1,1.0,0,0,0,1779265080000,1\n";
        let err = parse_okx_candlesticks(bad, CANDLE_INST).unwrap_err();
        assert!(err.to_string().contains("high"), "{err}");
    }

    #[test]
    fn candles_map_to_bars_external_last() {
        let rows = parse_okx_candlesticks(SAMPLE_CANDLES_CSV, CANDLE_INST).unwrap();
        let bars = okx_candlesticks_to_bars(&rows, &candle_spec(), minute_bar()).unwrap();
        assert_eq!(bars.len(), 2);
        // Bar type is EXTERNAL-sourced LAST-price at the configured step/unit.
        let bar_type = bars[0].bar_type;
        assert_eq!(bar_type.aggregation_source(), AggregationSource::External);
        assert_eq!(bar_type.spec().price_type, PriceType::Last);
        assert_eq!(bar_type.spec().aggregation, BarAggregation::Minute);
        assert!(bars.iter().all(|b| b.open.precision == 1));
        assert!(bars.windows(2).all(|w| w[0].ts_init <= w[1].ts_init));
    }

    #[test]
    fn zip_extractor_rejects_bad_signature() {
        let err = extract_csv_from_zip(b"not a zip archive at all............").unwrap_err();
        assert!(err.to_string().contains("signature"), "{err}");
    }

    #[test]
    fn zip_extractor_resolves_zip64_sentinel_sizes() {
        // A STORED member whose local-header 32-bit size fields hold the
        // 0xFFFFFFFF sentinel, with the real sizes in a ZIP64 extra field — the
        // shape data.binance.vision monthly archives take once their member
        // exceeds 4 GiB uncompressed. Tiny payload exercises the ZIP64 path
        // without a 4 GiB fixture.
        let content = b"a,b,c\n1,2,3\n";
        let mut hasher = Crc::new();
        hasher.update(content);
        let crc = hasher.sum();
        let name = b"x";

        let mut zip = Vec::new();
        zip.extend_from_slice(&ZIP_LOCAL_HEADER_SIG);
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&ZIP_METHOD_STORED.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&ZIP32_SIZE_SENTINEL.to_le_bytes()); // compressed sentinel
        zip.extend_from_slice(&ZIP32_SIZE_SENTINEL.to_le_bytes()); // uncompressed sentinel
        zip.extend_from_slice(&(name.len() as u16).to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes()); // extra_len: 4 header + 16 data
        zip.extend_from_slice(name);
        // ZIP64 extra: id, block length, then uncompressed size, compressed size.
        zip.extend_from_slice(&ZIP64_EXTRA_ID.to_le_bytes());
        zip.extend_from_slice(&16u16.to_le_bytes());
        zip.extend_from_slice(&(content.len() as u64).to_le_bytes());
        zip.extend_from_slice(&(content.len() as u64).to_le_bytes());
        zip.extend_from_slice(content);

        let csv = extract_csv_from_zip(&zip).expect("ZIP64 member extracts");
        assert_eq!(csv, "a,b,c\n1,2,3\n");
    }

    #[test]
    fn decimal_places_rounds_f64_round_trip_artifacts() {
        // OKX renders some prices as f64 round-trip noise (a true 0.09656 as
        // "0.09655999999999999"); rounding to NT_FIXED_PRECISION recovers the
        // intended tick and keeps the derived precision within NautilusTrader's
        // 9-place cap rather than blowing past it at the 17th decimal.
        assert_eq!(decimal_places("0.09655999999999999").unwrap(), 5);
        assert_eq!(decimal_places("0.09382000000000000").unwrap(), 5);
        assert_eq!(decimal_places("0.09382").unwrap(), 5);
        assert_eq!(decimal_places("5995").unwrap(), 0);
        // Rescaling the artifact to its derived precision yields the clean tick.
        assert_eq!(rescaled("0.09655999999999999", 5).unwrap(), "0.09656");
        assert_eq!(rescaled_to("0.09655999999999999", 5).unwrap(), "0.09656");
    }
}
