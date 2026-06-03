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

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{BookOrder, OrderBookDelta},
    enums::{BookAction, OrderSide, RecordFlag},
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

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

/// Maximum decimal places of a decimal string (`"643.3"` -> 1, `"5995"` -> 0).
fn decimal_places(value: &str) -> Result<u8> {
    let decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.scale()).context("decimal scale exceeds u8")
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
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
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
}
