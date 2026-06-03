//! Hyperliquid HIP-4 prediction-market snapshot converter (spec 023
//! `1-backtesting-engine`, venue slice `hyperliquid-hip4`).
//!
//! Hyperliquid HIP-4 publishes a *fixed-depth L2 snapshot* per prediction-market
//! outcome: each staged record (`source_family=info.l2Book`) is a full
//! point-in-time photo of one outcome's book — `raw_levels = [bids[], asks[]]`,
//! where every level is the native HL `{ px, sz, n }` triple (up to 20 levels per
//! side). There is at most one photo per outcome per backfill run, and some
//! outcomes are empty (no resting liquidity).
//!
//! The NautilusTrader-faithful mapping of a full book photo is the same one
//! NautilusTrader itself emits from `OrderBook::to_deltas`
//! (`crates/model/src/orderbook/book.rs`): a leading [`BookAction::Clear`] that
//! resets the book, followed by one [`BookAction::Add`] per level, all carrying
//! the `F_SNAPSHOT` record flag, with `F_LAST` set on the final delta of the
//! photo (or on the `Clear` itself when the book is empty). This module
//! replicates that exact flag protocol so the resulting [`OrderBookDelta`]s are
//! replayable by NautilusTrader's order-book engine.
//!
//! Each HIP-4 outcome is a distinct binary prediction-market instrument priced in
//! `[0, 1]`, so it maps to its own NautilusTrader `instrument_id` and is written
//! to its own catalog partition — mirroring how NautilusTrader's
//! `ParquetDataCatalog` partitions order-book deltas by `instrument_id`. No
//! runtime value (venue, quote token, outcome id, precision) is hardcoded: every
//! field is read from the staged record, and price/size precision is derived from
//! the data itself.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{BookOrder, OrderBookDelta, order::NULL_ORDER},
    enums::{BookAction, OrderSide, RecordFlag},
    identifiers::InstrumentId,
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// Native HL `source_family` that carries a full L2 book photo. Records with any
/// other source family are not fixed-depth snapshots and are rejected.
pub const HIP4_SNAPSHOT_SOURCE_FAMILY: &str = "info.l2Book";

/// HL snapshot timestamps are Unix milliseconds; NautilusTrader uses nanoseconds.
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// One native HL price level: `{ px, sz, n }` (n = number of resting orders at the
/// level; carried for provenance, not needed to build an aggregate L2 delta).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawLevel {
    /// Level price as the exact source decimal string.
    pub px: String,
    /// Aggregate level size as the exact source decimal string.
    pub sz: String,
    /// Number of resting orders at the level.
    pub n: u32,
}

/// One staged HIP-4 fixed-depth order-book snapshot record (`table=
/// order_book_snapshots_fixed_depth`). Only the fields this converter consumes
/// are modelled; unknown fields are ignored so the parser tolerates additive
/// schema evolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4SnapshotRecord {
    /// Source family; must be [`HIP4_SNAPSHOT_SOURCE_FAMILY`] for a valid photo.
    pub source_family: String,
    /// Venue identifier, e.g. `hyperliquid`.
    pub venue: String,
    /// Quote token, e.g. `USDC`.
    pub quote_token: String,
    /// HL trade-coin handle for the outcome, e.g. `#1010`.
    pub trade_coin: String,
    /// Numeric prediction-market outcome id.
    pub outcome: i64,
    /// Exchange snapshot time in Unix milliseconds.
    pub snapshot_time: i64,
    /// `[bids, asks]` — each a level array, best-first, native HL ordering.
    pub raw_levels: Vec<Vec<RawLevel>>,
    /// Reported bid level count (cross-checked against `raw_levels[0]`).
    pub bid_level_count: u32,
    /// Reported ask level count (cross-checked against `raw_levels[1]`).
    pub ask_level_count: u32,
}

/// How the venue-native identifiers map onto a NautilusTrader `instrument_id`.
///
/// Built by the caller (from accepted instrument-universe metadata in the real
/// pipeline), so the venue code suffix and outcome-symbol prefix are never
/// hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4InstrumentNaming {
    /// NautilusTrader venue code appended after the dot, e.g. `HYPERLIQUID`.
    pub nt_venue_code: String,
    /// Symbol prefix applied to the numeric outcome id, e.g. `OUTCOME-`.
    pub outcome_symbol_prefix: String,
    /// Expected source venue token; records with a different venue are rejected.
    pub expected_venue: String,
}

/// A parsed, validated set of HIP-4 snapshots grouped by outcome instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4SnapshotTable {
    /// One entry per outcome that appeared in the source, ordered deterministically
    /// by resolved NautilusTrader `instrument_id`.
    pub instruments: Vec<Hip4InstrumentSnapshots>,
}

/// All snapshots for a single HIP-4 outcome instrument, in ascending snapshot
/// time, with the precision derived from this instrument's own level data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4InstrumentSnapshots {
    /// Resolved NautilusTrader instrument id, e.g. `OUTCOME-101.HYPERLIQUID`.
    pub nt_instrument_id: String,
    /// Price precision (max decimal places across this instrument's level prices).
    pub price_precision: u8,
    /// Size precision (max decimal places across this instrument's level sizes).
    pub size_precision: u8,
    /// The full book photos for this instrument, ascending by snapshot time.
    pub snapshots: Vec<Hip4InstrumentSnapshot>,
}

/// One full book photo for one outcome at one snapshot time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4InstrumentSnapshot {
    /// Snapshot time in Unix nanoseconds.
    pub ts_event: UnixNanos,
    /// Bid levels, best-first.
    pub bids: Vec<RawLevel>,
    /// Ask levels, best-first.
    pub asks: Vec<RawLevel>,
}

/// Result of projecting HIP-4 snapshots into a NautilusTrader `ParquetDataCatalog`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4CatalogProjection {
    pub catalog_root: PathBuf,
    pub data_type: String,
    /// One entry per instrument written, in deterministic instrument-id order.
    pub instruments: Vec<Hip4InstrumentProjection>,
    /// Total `OrderBookDelta`s written across all instruments.
    pub total_delta_count: usize,
}

/// Per-instrument projection summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4InstrumentProjection {
    pub nt_instrument_id: String,
    /// `OrderBookDelta`s written for this instrument (1 Clear + 1 Add per level).
    pub delta_count: usize,
}

/// Decimal places implied by a decimal-string value (`0.43` -> 2, `0.00001` -> 5,
/// `22.0` -> 1, `100` -> 0). Trailing zeros are significant, matching the
/// precision `Price::from_str`/`Quantity::from_str` infer from the same string.
#[must_use]
fn decimal_places(value: &str) -> u8 {
    match value.split_once('.') {
        Some((_, frac)) => u8::try_from(frac.len()).unwrap_or(u8::MAX),
        None => 0,
    }
}

/// Rescale a decimal string to exactly `precision` decimal places, failing if the
/// source carries more precision than the target allows (which would silently
/// drop information).
fn rescaled(value: &str, precision: u8) -> Result<String> {
    let mut decimal = Decimal::from_str(value).with_context(|| format!("decimal {value:?}"))?;
    ensure!(
        decimal.scale() <= u32::from(precision),
        "value {value:?} has more precision than instrument allows ({precision})"
    );
    decimal.rescale(u32::from(precision));
    Ok(decimal.to_string())
}

/// Parse a JSONL fixed-depth snapshot object into a per-instrument snapshot table.
///
/// Each input line is one [`Hip4SnapshotRecord`]. Records are grouped by their
/// resolved NautilusTrader `instrument_id`; within each instrument the snapshots
/// are sorted ascending by snapshot time (required by NautilusTrader's catalog
/// writer, which rejects descending timestamps). Per-instrument price/size
/// precision is derived from that instrument's own level data.
///
/// # Errors
///
/// Returns an error if a line is not valid JSON, carries an unexpected source
/// family or venue, has a `raw_levels` shape other than `[bids, asks]`, or its
/// reported level counts disagree with the level arrays.
pub fn parse_hip4_snapshots(
    jsonl: &str,
    naming: &Hip4InstrumentNaming,
) -> Result<Hip4SnapshotTable> {
    // Keyed by nt_instrument_id; BTreeMap gives deterministic instrument ordering.
    let mut by_instrument: BTreeMap<String, Vec<Hip4InstrumentSnapshot>> = BTreeMap::new();

    for (line_no, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Hip4SnapshotRecord = serde_json::from_str(line)
            .with_context(|| format!("parse HIP-4 snapshot record on line {}", line_no + 1))?;

        ensure!(
            record.source_family == HIP4_SNAPSHOT_SOURCE_FAMILY,
            "line {}: unexpected source_family {:?}, expected {:?}",
            line_no + 1,
            record.source_family,
            HIP4_SNAPSHOT_SOURCE_FAMILY,
        );
        ensure!(
            record.venue == naming.expected_venue,
            "line {}: unexpected venue {:?}, expected {:?}",
            line_no + 1,
            record.venue,
            naming.expected_venue,
        );
        ensure!(
            record.raw_levels.len() == 2,
            "line {}: raw_levels must be [bids, asks], got {} arrays",
            line_no + 1,
            record.raw_levels.len(),
        );

        let mut iter = record.raw_levels.into_iter();
        let bids = iter.next().expect("len checked == 2");
        let asks = iter.next().expect("len checked == 2");
        ensure!(
            u32::try_from(bids.len()).unwrap_or(u32::MAX) == record.bid_level_count,
            "line {}: bid_level_count {} disagrees with {} bid levels",
            line_no + 1,
            record.bid_level_count,
            bids.len(),
        );
        ensure!(
            u32::try_from(asks.len()).unwrap_or(u32::MAX) == record.ask_level_count,
            "line {}: ask_level_count {} disagrees with {} ask levels",
            line_no + 1,
            record.ask_level_count,
            asks.len(),
        );

        let snapshot_nanos = record
            .snapshot_time
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("line {}: snapshot_time overflow", line_no + 1))?;
        let ts_event = UnixNanos::from(
            u64::try_from(snapshot_nanos)
                .with_context(|| format!("line {}: negative snapshot_time", line_no + 1))?,
        );

        let nt_instrument_id = format!(
            "{}{}.{}",
            naming.outcome_symbol_prefix, record.outcome, naming.nt_venue_code,
        );
        // Reject ids NautilusTrader cannot parse before they reach the catalog.
        InstrumentId::from_str(&nt_instrument_id).with_context(|| {
            format!(
                "line {}: invalid nt_instrument_id {nt_instrument_id:?}",
                line_no + 1
            )
        })?;

        by_instrument
            .entry(nt_instrument_id)
            .or_default()
            .push(Hip4InstrumentSnapshot {
                ts_event,
                bids,
                asks,
            });
    }

    let mut instruments = Vec::with_capacity(by_instrument.len());
    for (nt_instrument_id, mut snapshots) in by_instrument {
        snapshots.sort_by_key(|snapshot| snapshot.ts_event);
        let (price_precision, size_precision) = derive_precision(&snapshots);
        instruments.push(Hip4InstrumentSnapshots {
            nt_instrument_id,
            price_precision,
            size_precision,
            snapshots,
        });
    }

    Ok(Hip4SnapshotTable { instruments })
}

/// Derive uniform `(price_precision, size_precision)` for one instrument as the
/// max decimal places across all of its level prices/sizes. NautilusTrader's
/// order-book delta parquet encoder takes a single precision per instrument file,
/// so every level in the file must share it.
#[must_use]
fn derive_precision(snapshots: &[Hip4InstrumentSnapshot]) -> (u8, u8) {
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for snapshot in snapshots {
        for level in snapshot.bids.iter().chain(snapshot.asks.iter()) {
            price_precision = price_precision.max(decimal_places(&level.px));
            size_precision = size_precision.max(decimal_places(&level.sz));
        }
    }
    (price_precision, size_precision)
}

/// Build the `OrderBookDelta` photo for one instrument's snapshots, replicating
/// NautilusTrader's own snapshot-to-deltas flag protocol: each photo is a leading
/// `Clear` (with `F_LAST` when the book is empty, else `F_SNAPSHOT`) followed by
/// one `Add` per level carrying `F_SNAPSHOT`, with `F_LAST` set on the last level
/// of the photo. Bid order ids start at 1 and ask order ids continue from there so
/// every resting level carries a stable, distinct synthetic id.
///
/// # Errors
///
/// Returns an error if any level price/size cannot be represented at the
/// instrument's derived precision, or a resting level has non-positive size.
pub fn snapshots_to_deltas(instrument: &Hip4InstrumentSnapshots) -> Result<Vec<OrderBookDelta>> {
    let instrument_id = InstrumentId::from_str(&instrument.nt_instrument_id)
        .with_context(|| format!("invalid nt_instrument_id {:?}", instrument.nt_instrument_id))?;
    let mut deltas = Vec::new();

    for (sequence, snapshot) in instrument.snapshots.iter().enumerate() {
        let sequence = u64::try_from(sequence).context("snapshot sequence overflow")?;
        let total_levels = snapshot.bids.len() + snapshot.asks.len();

        // Leading Clear resets the book for this photo. `OrderBookDelta::clear`
        // already stamps the F_SNAPSHOT flag; add F_LAST when the photo carries no
        // levels so buffered consumers flush an empty book.
        let mut clear = OrderBookDelta::clear(
            instrument_id,
            sequence,
            snapshot.ts_event,
            snapshot.ts_event,
        );
        if total_levels == 0 {
            clear.flags |= RecordFlag::F_LAST as u8;
        }
        deltas.push(clear);

        let mut emitted = 0usize;
        let mut order_id = 0u64;
        for (side, levels) in [
            (OrderSide::Buy, &snapshot.bids),
            (OrderSide::Sell, &snapshot.asks),
        ] {
            for level in levels {
                order_id += 1;
                emitted += 1;
                let flags = if emitted == total_levels {
                    RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_LAST as u8
                } else {
                    RecordFlag::F_SNAPSHOT as u8
                };
                let price_str = rescaled(&level.px, instrument.price_precision)?;
                let price = Price::from_str(&price_str)
                    .map_err(|error| anyhow::anyhow!("invalid price {price_str:?}: {error}"))?;
                let size_str = rescaled(&level.sz, instrument.size_precision)?;
                let size = Quantity::from_str(&size_str)
                    .map_err(|error| anyhow::anyhow!("invalid size {size_str:?}: {error}"))?;
                ensure!(
                    size.is_positive(),
                    "snapshot level for {} has non-positive size {size_str:?}; \
                     a resting L2 level must have positive size",
                    instrument.nt_instrument_id,
                );
                deltas.push(OrderBookDelta::new(
                    instrument_id,
                    BookAction::Add,
                    BookOrder::new(side, price, size, order_id),
                    flags,
                    sequence,
                    snapshot.ts_event,
                    snapshot.ts_event,
                ));
            }
        }
        debug_assert_eq!(emitted, total_levels, "emitted every level exactly once");
    }

    // Every instrument photo set begins with a Clear (NULL_ORDER, zero precision);
    // the catalog encoder recovers the real precision from the first following Add.
    debug_assert!(
        deltas.first().map(|delta| delta.order) == Some(NULL_ORDER),
        "every instrument photo set begins with a Clear",
    );
    Ok(deltas)
}

/// Project parsed HIP-4 snapshots into a NautilusTrader `ParquetDataCatalog`,
/// writing one `OrderBookDelta` partition per outcome instrument.
///
/// Fails closed on a non-empty `catalog_root`: NautilusTrader's `write_to_parquet`
/// skips writing when a file for the same instrument/interval already exists, so
/// projecting into a dirty root could silently retain stale data.
///
/// # Errors
///
/// Returns an error if the root is dirty, delta construction fails, or a catalog
/// write fails.
pub fn project_hip4_snapshots_to_catalog(
    table: &Hip4SnapshotTable,
    catalog_root: &Path,
) -> Result<Hip4CatalogProjection> {
    ensure!(
        !table.instruments.is_empty(),
        "refusing to project an empty HIP-4 snapshot table",
    );
    if catalog_root.exists() {
        let mut entries = fs::read_dir(catalog_root)
            .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
        ensure!(
            entries.next().is_none(),
            "catalog root {} is not empty; refusing to project into a dirty catalog",
            catalog_root.display(),
        );
    }
    fs::create_dir_all(catalog_root)
        .with_context(|| format!("create catalog root {}", catalog_root.display()))?;

    // Large batch size so each instrument's photos land in a single chunk, keeping
    // the encoder's per-chunk precision metadata consistent.
    let catalog = ParquetDataCatalog::new(catalog_root, None, Some(10_000), None, None);

    let mut instruments = Vec::with_capacity(table.instruments.len());
    let mut total_delta_count = 0usize;
    for instrument in &table.instruments {
        let deltas = snapshots_to_deltas(instrument)?;
        ensure!(
            !deltas.is_empty(),
            "instrument {} produced no deltas",
            instrument.nt_instrument_id,
        );
        let delta_count = deltas.len();
        catalog
            .write_to_parquet(deltas, None, None, None)
            .with_context(|| {
                format!(
                    "write order book deltas for {} to catalog",
                    instrument.nt_instrument_id
                )
            })?;
        total_delta_count += delta_count;
        instruments.push(Hip4InstrumentProjection {
            nt_instrument_id: instrument.nt_instrument_id.clone(),
            delta_count,
        });
    }

    Ok(Hip4CatalogProjection {
        catalog_root: catalog_root.to_path_buf(),
        data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
        instruments,
        total_delta_count,
    })
}

/// Read the projected `OrderBookDelta`s for one instrument back out of the
/// catalog, proving the resolved NautilusTrader dependency can replay them.
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
        .with_context(|| format!("query order book deltas for {nt_instrument_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naming() -> Hip4InstrumentNaming {
        Hip4InstrumentNaming {
            nt_venue_code: "HYPERLIQUID".to_string(),
            outcome_symbol_prefix: "OUTCOME-".to_string(),
            expected_venue: "hyperliquid".to_string(),
        }
    }

    const ONE_NONEMPTY: &str = "{\"source_family\":\"info.l2Book\",\"venue\":\"hyperliquid\",\
        \"quote_token\":\"USDC\",\"trade_coin\":\"#1\",\"outcome\":7,\"snapshot_time\":1780331397800,\
        \"raw_levels\":[[{\"px\":\"0.43\",\"sz\":\"22.0\",\"n\":1}],\
        [{\"px\":\"0.50\",\"sz\":\"978.0\",\"n\":2}]],\
        \"bid_level_count\":1,\"ask_level_count\":1}";

    const ONE_EMPTY: &str = "{\"source_family\":\"info.l2Book\",\"venue\":\"hyperliquid\",\
        \"quote_token\":\"USDC\",\"trade_coin\":\"#2\",\"outcome\":8,\"snapshot_time\":1780331396707,\
        \"raw_levels\":[[],[]],\"bid_level_count\":0,\"ask_level_count\":0}";

    #[test]
    fn decimal_places_reads_significant_digits() {
        assert_eq!(decimal_places("0.43"), 2);
        assert_eq!(decimal_places("0.00001"), 5);
        assert_eq!(decimal_places("22.0"), 1);
        assert_eq!(decimal_places("100"), 0);
        // Trailing zeros are significant.
        assert_eq!(decimal_places("0.50"), 2);
    }

    #[test]
    fn parses_nonempty_snapshot_into_one_instrument() {
        let table = parse_hip4_snapshots(ONE_NONEMPTY, &naming()).expect("parse");
        assert_eq!(table.instruments.len(), 1);
        let inst = &table.instruments[0];
        assert_eq!(inst.nt_instrument_id, "OUTCOME-7.HYPERLIQUID");
        assert_eq!(inst.price_precision, 2);
        assert_eq!(inst.size_precision, 1);
        assert_eq!(inst.snapshots.len(), 1);
        assert_eq!(inst.snapshots[0].bids.len(), 1);
        assert_eq!(inst.snapshots[0].asks.len(), 1);
    }

    #[test]
    fn snapshot_time_ms_becomes_nanos() {
        let table = parse_hip4_snapshots(ONE_NONEMPTY, &naming()).expect("parse");
        assert_eq!(
            table.instruments[0].snapshots[0].ts_event,
            UnixNanos::from(1780331397800u64 * 1_000_000),
        );
    }

    #[test]
    fn nonempty_photo_emits_clear_then_add_per_level_with_snapshot_flags() {
        let table = parse_hip4_snapshots(ONE_NONEMPTY, &naming()).expect("parse");
        let deltas = snapshots_to_deltas(&table.instruments[0]).expect("deltas");
        // 1 Clear + 1 bid Add + 1 ask Add.
        assert_eq!(deltas.len(), 3);
        assert_eq!(deltas[0].action, BookAction::Clear);
        assert_eq!(deltas[0].order, NULL_ORDER);
        // Clear of a non-empty photo carries F_SNAPSHOT but not F_LAST.
        assert!(RecordFlag::F_SNAPSHOT.matches(deltas[0].flags));
        assert!(!RecordFlag::F_LAST.matches(deltas[0].flags));

        assert_eq!(deltas[1].action, BookAction::Add);
        assert_eq!(deltas[1].order.side, OrderSide::Buy);
        assert_eq!(deltas[1].order.price, Price::from("0.43"));
        assert!(RecordFlag::F_SNAPSHOT.matches(deltas[1].flags));
        assert!(!RecordFlag::F_LAST.matches(deltas[1].flags));

        assert_eq!(deltas[2].action, BookAction::Add);
        assert_eq!(deltas[2].order.side, OrderSide::Sell);
        // 0.5 rescaled to price precision 2 -> 0.50.
        assert_eq!(deltas[2].order.price, Price::from("0.50"));
        // Last level of the photo carries F_LAST.
        assert!(RecordFlag::F_SNAPSHOT.matches(deltas[2].flags));
        assert!(RecordFlag::F_LAST.matches(deltas[2].flags));
        // Bid and ask order ids are distinct.
        assert_ne!(deltas[1].order.order_id, deltas[2].order.order_id);
    }

    #[test]
    fn empty_photo_emits_single_clear_with_f_last() {
        let table = parse_hip4_snapshots(ONE_EMPTY, &naming()).expect("parse");
        let deltas = snapshots_to_deltas(&table.instruments[0]).expect("deltas");
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].action, BookAction::Clear);
        assert!(RecordFlag::F_SNAPSHOT.matches(deltas[0].flags));
        assert!(RecordFlag::F_LAST.matches(deltas[0].flags));
    }

    #[test]
    fn rejects_wrong_source_family() {
        let bad = ONE_NONEMPTY.replace("info.l2Book", "info.trades");
        let err = parse_hip4_snapshots(&bad, &naming()).expect_err("must reject");
        assert!(err.to_string().contains("source_family"), "{err}");
    }

    #[test]
    fn rejects_level_count_mismatch() {
        let bad = ONE_NONEMPTY.replace("\"bid_level_count\":1", "\"bid_level_count\":2");
        let err = parse_hip4_snapshots(&bad, &naming()).expect_err("must reject");
        assert!(err.to_string().contains("bid_level_count"), "{err}");
    }

    #[test]
    fn rejects_unexpected_venue() {
        let bad = ONE_NONEMPTY.replace("\"venue\":\"hyperliquid\"", "\"venue\":\"binance\"");
        let err = parse_hip4_snapshots(&bad, &naming()).expect_err("must reject");
        assert!(err.to_string().contains("venue"), "{err}");
    }

    #[test]
    fn projection_refuses_dirty_catalog_root() {
        let table = parse_hip4_snapshots(ONE_NONEMPTY, &naming()).expect("parse");
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_hip4_snapshots_to_catalog(&table, dir.path())
            .expect_err("dirty catalog root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    #[test]
    fn projects_and_reads_back_a_mixed_batch() {
        let jsonl = format!("{ONE_NONEMPTY}\n{ONE_EMPTY}\n");
        let table = parse_hip4_snapshots(&jsonl, &naming()).expect("parse");
        assert_eq!(table.instruments.len(), 2);

        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_hip4_snapshots_to_catalog(&table, dir.path()).expect("project");
        assert_eq!(projection.data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
        // 3 deltas for the non-empty outcome + 1 Clear for the empty outcome.
        assert_eq!(projection.total_delta_count, 4);

        let nonempty = read_back_order_book_deltas(dir.path(), "OUTCOME-7.HYPERLIQUID")
            .expect("read back nonempty");
        assert_eq!(nonempty.len(), 3);
        assert_eq!(nonempty[0].action, BookAction::Clear);
        assert_eq!(nonempty[1].order.price, Price::from("0.43"));
        assert_eq!(nonempty[2].order.price, Price::from("0.50"));

        let empty = read_back_order_book_deltas(dir.path(), "OUTCOME-8.HYPERLIQUID")
            .expect("read back empty");
        assert_eq!(empty.len(), 1);
        assert_eq!(empty[0].action, BookAction::Clear);
        assert!(RecordFlag::F_LAST.matches(empty[0].flags));
    }
}
