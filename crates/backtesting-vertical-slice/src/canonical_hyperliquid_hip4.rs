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
    num::NonZeroUsize,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{
        Bar, BarSpecification, BarType, BookOrder, OrderBookDelta, TradeTick, order::NULL_ORDER,
    },
    enums::{AggregationSource, AggressorSide, BookAction, OrderSide, PriceType, RecordFlag},
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// NautilusTrader data type written for this projection.
pub const NT_DATA_TYPE_ORDER_BOOK_DELTA: &str = "OrderBookDelta";

/// NautilusTrader data type written for the recent-trades family.
pub const NT_DATA_TYPE_TRADE_TICK: &str = "TradeTick";

/// NautilusTrader data type written for the candle-snapshot family.
pub const NT_DATA_TYPE_BAR: &str = "Bar";

/// Native HL `source_family` that carries a full L2 book photo. Records with any
/// other source family are not fixed-depth snapshots and are rejected.
pub const HIP4_SNAPSHOT_SOURCE_FAMILY: &str = "info.l2Book";

/// Native HL `source_family` that carries native trade prints (`recentTrades`).
pub const HIP4_TRADES_SOURCE_FAMILY: &str = "info.recentTrades";

/// Native HL `source_family` that carries OHLCV candles (`candleSnapshot`).
pub const HIP4_BARS_SOURCE_FAMILY: &str = "info.candleSnapshot";

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
/// In the live pipeline the caller builds this from accepted instrument-universe
/// metadata. The bulk historical conversion has no staged HIP-4 instrument
/// universe to read from, so it single-sources the venue's mapping facts via
/// [`hip4_canonical_naming`] (the same way the OKX path owns its `OKX_VENUE`
/// code) — these are fixed HYPERLIQUID->NautilusTrader format facts, not
/// per-run runtime values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4InstrumentNaming {
    /// NautilusTrader venue code appended after the dot, e.g. `HYPERLIQUID`.
    pub nt_venue_code: String,
    /// Symbol prefix applied to the numeric outcome id, e.g. `OUTCOME-`.
    pub outcome_symbol_prefix: String,
    /// Expected source venue token; records with a different venue are rejected.
    pub expected_venue: String,
}

/// The canonical HIP-4 instrument naming the bulk historical converter uses.
///
/// HIP-4 stages no instrument universe, so the HYPERLIQUID->NautilusTrader
/// mapping facts (venue code, outcome-symbol prefix, source-venue token) live
/// here as the single source of truth, consumed by both the bulk append paths
/// and their round-trip tests.
#[must_use]
pub fn hip4_canonical_naming() -> Hip4InstrumentNaming {
    Hip4InstrumentNaming {
        nt_venue_code: "HYPERLIQUID".to_string(),
        outcome_symbol_prefix: "OUTCOME-".to_string(),
        expected_venue: "hyperliquid".to_string(),
    }
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
        decimal.normalize().scale() <= u32::from(precision),
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

    // Build every instrument's deltas first so the catalog batch size is derived
    // from the data instead of guessed: sizing it to the largest instrument's
    // delta count keeps each instrument's snapshots in a single chunk (so the
    // encoder's per-chunk precision metadata stays consistent) for any data size.
    let mut prepared = Vec::with_capacity(table.instruments.len());
    for instrument in &table.instruments {
        let deltas = snapshots_to_deltas(instrument)?;
        ensure!(
            !deltas.is_empty(),
            "instrument {} produced no deltas",
            instrument.nt_instrument_id,
        );
        prepared.push((instrument, deltas));
    }
    let batch_size = prepared
        .iter()
        .map(|(_, deltas)| deltas.len())
        .max()
        .unwrap_or(1);
    let catalog = ParquetDataCatalog::new(catalog_root, None, Some(batch_size), None, None);

    let mut instruments = Vec::with_capacity(prepared.len());
    let mut total_delta_count = 0usize;
    for (instrument, deltas) in prepared {
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

// ===========================================================================
// HIP-4 recent trades (`info.recentTrades`)  ->  NautilusTrader `TradeTick`
// and HIP-4 candle snapshots (`info.candleSnapshot`)  ->  NautilusTrader `Bar`
//
// Both families are keyed by the HL `trade_coin` (for example `#1010`): each
// HIP-4 outcome publishes one tradeable coin per binary leg (one per `side`),
// and a `trade_coin` maps to exactly one `(outcome, side)` pair, with
// `tid`/candle open time unique within a coin. So the NautilusTrader instrument
// for these two families is the `(outcome, side)` pair the `trade_coin` denotes.
//
// CATALOG IDENTITY: the NautilusTrader `instrument_id` is NOT the raw
// `trade_coin` handle. The HL handle carries a leading `#` (for example
// `#1010`), which is a URI fragment delimiter; an id such as `#1010.HYPERLIQUID`
// is mangled by `ParquetDataCatalog`/`object_store` on read-back (the write
// "succeeds" but `query_typed_data` finds nothing). The honest catalog-safe id is
// derived from each record's own `outcome` + `side` integers the SAME way the L2
// snapshot family derives its id — via [`Hip4InstrumentNaming`]
// (`outcome_symbol_prefix` + `nt_venue_code`) — so a market's trades, bars, and
// book land under matching, URI-safe ids (`OUTCOME-<outcome>-<side>.HYPERLIQUID`).
// The `trade_coin` handle is retained only as the per-coin fence the normalizers
// filter on; it never reaches the catalog. All venue/symbol/precision/bar-spec
// values are supplied by the caller via [`Hip4InstrumentNaming`] /
// [`Hip4MarketDataSpec`] — nothing is hardcoded here.
// ===========================================================================

/// Caller-supplied identity + precision + bar specification for one HIP-4
/// `trade_coin` instrument. Built by the caller from the accepted instrument
/// universe; precision is derived from the increment strings (never hardcoded),
/// and the bar specification comes from the `interval=` archive partition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4MarketDataSpec {
    /// Expected source venue token; records with a different venue are rejected.
    pub expected_venue: String,
    /// HL-native tradeable coin handle, for example `#1010`. Records whose
    /// `trade_coin` differs are not part of this instrument and are skipped. This
    /// is the per-coin fence only; it never reaches the catalog id (see
    /// [`Hip4InstrumentNaming`] for how the catalog id is derived).
    pub trade_coin: String,
    /// NautilusTrader instrument id, for example `OUTCOME-101-0.HYPERLIQUID`.
    /// Must be URI-safe (no `#`); the bulk path derives it from each record's
    /// own `outcome` + `side` via [`Hip4InstrumentNaming`].
    pub nt_instrument_id: String,
    /// Price tick size as a decimal string, for example `0.001`.
    pub price_increment: String,
    /// Size step as a decimal string, for example `0.1`.
    pub size_increment: String,
    /// Bar step (for example `1`) for the candle interval.
    pub bar_step: usize,
    /// Bar aggregation unit for the candle interval.
    pub bar_aggregation: Hip4BarAggregation,
}

impl Hip4MarketDataSpec {
    fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("expected_venue", &self.expected_venue),
            ("trade_coin", &self.trade_coin),
            ("nt_instrument_id", &self.nt_instrument_id),
            ("price_increment", &self.price_increment),
            ("size_increment", &self.size_increment),
        ] {
            ensure!(!value.trim().is_empty(), "empty spec field: {name}");
        }
        ensure!(self.bar_step > 0, "bar_step must be positive");
        InstrumentId::from_str(&self.nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {:?}", self.nt_instrument_id))?;
        Ok(())
    }

    fn price_precision(&self) -> u8 {
        decimal_places(&self.price_increment)
    }

    fn size_precision(&self) -> u8 {
        decimal_places(&self.size_increment)
    }

    fn instrument_id(&self) -> Result<InstrumentId> {
        InstrumentId::from_str(&self.nt_instrument_id)
            .with_context(|| format!("invalid nt_instrument_id {:?}", self.nt_instrument_id))
    }

    fn bar_type(&self) -> Result<BarType> {
        let step = NonZeroUsize::new(self.bar_step).context("bar_step must be non-zero")?;
        // HIP-4 candles are aggregated by the exchange, outside the Nautilus
        // boundary, so they replay as `EXTERNAL`-sourced, `LAST`-price bars.
        let spec = BarSpecification {
            step,
            aggregation: self.bar_aggregation.to_nt(),
            price_type: PriceType::Last,
        };
        Ok(BarType::new(
            self.instrument_id()?,
            spec,
            AggregationSource::External,
        ))
    }
}

/// Bar aggregation unit, supplied by the caller from the candle `interval`
/// partition (for example `1h` -> step 1, [`Self::Hour`]). A small explicit enum
/// (rather than reusing NautilusTrader's full [`nautilus_model::enums::BarAggregation`])
/// keeps the spec serde-stable and decoupled from the HL interval vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Hip4BarAggregation {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
}

impl Hip4BarAggregation {
    fn to_nt(self) -> nautilus_model::enums::BarAggregation {
        use nautilus_model::enums::BarAggregation as B;
        match self {
            Self::Second => B::Second,
            Self::Minute => B::Minute,
            Self::Hour => B::Hour,
            Self::Day => B::Day,
            Self::Week => B::Week,
            Self::Month => B::Month,
        }
    }
}

/// Aggressor side of a HL `recentTrades` print.
///
/// HL `recentTrades` reports `side` as `"A"` (the aggressor lifted the ask /
/// crossed the offer) or `"B"` (the aggressor hit the bid). An ask-aggressor
/// trade is buy-initiated only at the offer — in HL's wire convention `"A"`
/// means the trade was **seller**-initiated (the resting ask was taken by a
/// market sell against the bid is `"B"`). We follow HL's documented mapping:
/// `"A"` (ask) -> [`AggressorSide::Seller`], `"B"` (bid) -> [`AggressorSide::Buyer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Hip4TradeAggressorSide {
    Buyer,
    Seller,
}

impl Hip4TradeAggressorSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SELLER",
        }
    }

    /// Map the HL `side` token (`"A"`/`"B"`) to the aggressor side.
    fn from_hl_side(raw: &str) -> Result<Self> {
        match raw.trim() {
            "A" => Ok(Self::Seller),
            "B" => Ok(Self::Buyer),
            other => bail!("unknown HL trade side token: {other:?}"),
        }
    }

    fn to_nt(self) -> AggressorSide {
        match self {
            Self::Buyer => AggressorSide::Buyer,
            Self::Seller => AggressorSide::Seller,
        }
    }
}

// ---------------------------------------------------------------------------
// Trades: source records + canonical rows
// ---------------------------------------------------------------------------

/// One staged HIP-4 `recentTrades` record. Only the consumed fields are
/// modelled; unknown fields are tolerated for additive schema evolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4TradeRecord {
    /// Source family; must be [`HIP4_TRADES_SOURCE_FAMILY`] for a valid print.
    pub source_family: String,
    /// Venue identifier, for example `hyperliquid`.
    pub venue: String,
    /// HL tradeable coin handle for this print, for example `#1010`.
    pub trade_coin: String,
    /// HL trade id, unique within a coin.
    pub tid: i64,
    /// Exchange event time in Unix milliseconds.
    pub time: i64,
    /// Exact source price string.
    pub px: String,
    /// Exact source size string.
    pub sz: String,
    /// HL aggressor token (`"A"`/`"B"`).
    pub trade_side: String,
}

/// One normalized HIP-4 trade row at nanosecond resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4TradeRow {
    /// Exchange event time in Unix nanoseconds.
    pub event_time: i64,
    /// Trade id (the HL `tid` rendered as a string).
    pub trade_id: String,
    pub aggressor_side: Hip4TradeAggressorSide,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
}

/// A validated canonical HIP-4 trades table for one `trade_coin` instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4TradesTable {
    pub nt_instrument_id: String,
    pub trade_coin: String,
    pub rows: Vec<Hip4TradeRow>,
}

/// Normalize a JSONL `info.recentTrades` object into the canonical trades table
/// for one `trade_coin`.
///
/// Records whose `trade_coin` differs from `spec.trade_coin` belong to a
/// different instrument and are skipped (a HIP-4 staged object holds every
/// coin's prints interleaved). Records with the wrong source family or venue are
/// rejected. Prints are sorted ascending by event time (HL stages them
/// newest-first / interleaved) so the NautilusTrader catalog write is monotonic.
///
/// # Errors
///
/// Returns an error if a line is not valid JSON, carries the wrong source family
/// or venue, has an unknown aggressor token, a non-positive price/size, or the
/// resulting table fails contract validation (including: no row matched the
/// requested coin).
pub fn normalize_hip4_trades(jsonl: &str, spec: &Hip4MarketDataSpec) -> Result<Hip4TradesTable> {
    spec.validate()?;

    let mut rows = Vec::new();
    for (line_no, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Hip4TradeRecord = serde_json::from_str(line)
            .with_context(|| format!("parse HIP-4 trade record on line {}", line_no + 1))?;

        ensure!(
            record.source_family == HIP4_TRADES_SOURCE_FAMILY,
            "line {}: unexpected source_family {:?}, expected {:?}",
            line_no + 1,
            record.source_family,
            HIP4_TRADES_SOURCE_FAMILY,
        );
        ensure!(
            record.venue == spec.expected_venue,
            "line {}: unexpected venue {:?}, expected {:?}",
            line_no + 1,
            record.venue,
            spec.expected_venue,
        );

        // Skip prints for other coins; one staged object interleaves all coins.
        if record.trade_coin != spec.trade_coin {
            continue;
        }

        let aggressor = Hip4TradeAggressorSide::from_hl_side(&record.trade_side)
            .with_context(|| format!("line {}: invalid trade side", line_no + 1))?;

        let price: Decimal = record
            .px
            .parse()
            .with_context(|| format!("line {}: invalid price {:?}", line_no + 1, record.px))?;
        let size: Decimal = record
            .sz
            .parse()
            .with_context(|| format!("line {}: invalid size {:?}", line_no + 1, record.sz))?;
        ensure!(
            price > Decimal::ZERO,
            "line {}: non-positive price",
            line_no + 1
        );
        ensure!(
            size > Decimal::ZERO,
            "line {}: non-positive size",
            line_no + 1
        );

        let event_time = record
            .time
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("line {}: event time overflow", line_no + 1))?;

        rows.push(Hip4TradeRow {
            event_time,
            trade_id: record.tid.to_string(),
            aggressor_side: aggressor,
            price: record.px.clone(),
            size: record.sz.clone(),
        });
    }

    // HL stages prints interleaved/newest-first; NautilusTrader requires ascending ts.
    rows.sort_by_key(|row| row.event_time);

    let table = Hip4TradesTable {
        nt_instrument_id: spec.nt_instrument_id.clone(),
        trade_coin: spec.trade_coin.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl Hip4TradesTable {
    /// Validate non-emptiness, positive monotonic event timestamps, and required
    /// fields.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.rows.is_empty(),
            "HIP-4 trades table for {} is empty (no print matched the requested coin {})",
            self.nt_instrument_id,
            self.trade_coin,
        );
        for field in [&self.nt_instrument_id, &self.trade_coin] {
            ensure!(!field.trim().is_empty(), "empty identity field");
        }
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.event_time > 0,
                "trade row {index}: non-positive event_time"
            );
            ensure!(
                row.event_time >= previous,
                "trade row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous,
            );
            previous = row.event_time;
            for field in [&row.trade_id, &row.price, &row.size] {
                ensure!(!field.trim().is_empty(), "trade row {index}: empty field");
            }
        }
        Ok(())
    }

    /// Convert the canonical rows into NautilusTrader `TradeTick`s at the
    /// instrument's price/size precision.
    ///
    /// # Errors
    ///
    /// Returns an error if a price/size cannot be represented at the precision or
    /// a trade id exceeds the NautilusTrader limit.
    pub fn to_trade_ticks(&self, spec: &Hip4MarketDataSpec) -> Result<Vec<TradeTick>> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id,
        );
        let instrument_id = spec.instrument_id()?;
        let price_precision = spec.price_precision();
        let size_precision = spec.size_precision();
        self.rows
            .iter()
            .map(|row| {
                let price = price_at(&row.price, price_precision)?;
                let size = quantity_at(&row.size, size_precision)?;
                let trade_id = TradeId::new_checked(&row.trade_id).map_err(|error| {
                    anyhow::anyhow!("invalid trade id {:?}: {error}", row.trade_id)
                })?;
                let ts =
                    UnixNanos::from(u64::try_from(row.event_time).context("negative event_time")?);
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
}

// ---------------------------------------------------------------------------
// Bars: source records + canonical rows
// ---------------------------------------------------------------------------

/// One staged HIP-4 `candleSnapshot` record. Only the consumed fields are
/// modelled; unknown fields are tolerated for additive schema evolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4BarRecord {
    /// Source family; must be [`HIP4_BARS_SOURCE_FAMILY`] for a valid candle.
    pub source_family: String,
    /// Venue identifier, for example `hyperliquid`.
    pub venue: String,
    /// HL tradeable coin handle for this candle, for example `#1010`.
    pub trade_coin: String,
    /// Candle interval token, for example `1h` or `1M`. A staged object
    /// interleaves a coin's candles at many intervals; each `(coin, interval)` is
    /// its own NautilusTrader bar stream, so normalization keeps only the records
    /// matching the spec's interval.
    pub interval: String,
    /// Candle open (start) time in Unix milliseconds.
    pub open_time: i64,
    /// Candle close time in Unix milliseconds.
    pub close_time: i64,
    /// Exact source OHLCV strings.
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

/// One normalized HIP-4 bar row at nanosecond resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4BarRow {
    /// Candle open time in Unix nanoseconds.
    pub open_time: i64,
    /// Candle close time in Unix nanoseconds.
    pub close_time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
}

/// A validated canonical HIP-4 bars table for one `trade_coin` instrument.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hip4BarsTable {
    pub nt_instrument_id: String,
    pub trade_coin: String,
    pub rows: Vec<Hip4BarRow>,
}

/// Normalize a JSONL `info.candleSnapshot` object into the canonical bars table
/// for one `(trade_coin, interval)` bar stream.
///
/// A HIP-4 staged object interleaves every coin's candles at every published
/// interval (1m, 3m, ... 1w, 1M). Records whose `trade_coin` differs from
/// `spec.trade_coin`, or whose `interval` differs from the spec's bar interval
/// (its `(bar_step, bar_aggregation)`), are skipped — so one call yields exactly
/// one NautilusTrader bar stream. Records with the wrong source family or venue,
/// or violating OHLC integrity, are rejected. Candles are sorted ascending by
/// open time so the NautilusTrader catalog write is monotonic.
///
/// # Errors
///
/// Returns an error if a line is not valid JSON, carries the wrong source family
/// or venue, violates OHLC integrity, has a negative volume, or the resulting
/// table fails contract validation (including: no candle matched the coin).
pub fn normalize_hip4_bars(jsonl: &str, spec: &Hip4MarketDataSpec) -> Result<Hip4BarsTable> {
    spec.validate()?;

    let mut rows = Vec::new();
    for (line_no, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: Hip4BarRecord = serde_json::from_str(line)
            .with_context(|| format!("parse HIP-4 candle record on line {}", line_no + 1))?;

        ensure!(
            record.source_family == HIP4_BARS_SOURCE_FAMILY,
            "line {}: unexpected source_family {:?}, expected {:?}",
            line_no + 1,
            record.source_family,
            HIP4_BARS_SOURCE_FAMILY,
        );
        ensure!(
            record.venue == spec.expected_venue,
            "line {}: unexpected venue {:?}, expected {:?}",
            line_no + 1,
            record.venue,
            spec.expected_venue,
        );

        if record.trade_coin != spec.trade_coin {
            continue;
        }

        // A staged object interleaves a coin's candles at many intervals; keep
        // only those matching this spec's bar interval. The spec fully identifies
        // ONE `(coin, interval)` bar stream, and the `(step, aggregation)` pair is
        // a unique key for the interval token, so this can never lump intervals.
        let (record_step, record_aggregation) = parse_hip4_bar_interval(&record.interval)
            .with_context(|| format!("line {}: candle interval", line_no + 1))?;
        if record_step != spec.bar_step || record_aggregation != spec.bar_aggregation {
            continue;
        }

        let (o, h, l, c) = (
            Decimal::from_str(&record.open)
                .with_context(|| format!("line {}: open {:?}", line_no + 1, record.open))?,
            Decimal::from_str(&record.high)
                .with_context(|| format!("line {}: high {:?}", line_no + 1, record.high))?,
            Decimal::from_str(&record.low)
                .with_context(|| format!("line {}: low {:?}", line_no + 1, record.low))?,
            Decimal::from_str(&record.close)
                .with_context(|| format!("line {}: close {:?}", line_no + 1, record.close))?,
        );
        ensure!(o > Decimal::ZERO, "line {}: non-positive open", line_no + 1);
        // NautilusTrader's `Bar::new_checked` re-asserts this on the rounded
        // prices; checking here fails loudly on the source values first.
        ensure!(
            h >= o && h >= l && h >= c && l <= o && l <= c,
            "line {}: OHLC integrity violated (o={o} h={h} l={l} c={c})",
            line_no + 1,
        );
        let volume = Decimal::from_str(&record.volume)
            .with_context(|| format!("line {}: volume {:?}", line_no + 1, record.volume))?;
        ensure!(
            volume >= Decimal::ZERO,
            "line {}: negative volume",
            line_no + 1
        );

        let open_time = record
            .open_time
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("line {}: open_time overflow", line_no + 1))?;
        let close_time = record
            .close_time
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("line {}: close_time overflow", line_no + 1))?;
        ensure!(
            close_time >= open_time,
            "line {}: close_time precedes open_time",
            line_no + 1,
        );

        rows.push(Hip4BarRow {
            open_time,
            close_time,
            open: record.open.clone(),
            high: record.high.clone(),
            low: record.low.clone(),
            close: record.close.clone(),
            volume: record.volume.clone(),
        });
    }

    rows.sort_by_key(|row| row.open_time);

    let table = Hip4BarsTable {
        nt_instrument_id: spec.nt_instrument_id.clone(),
        trade_coin: spec.trade_coin.clone(),
        rows,
    };
    table.validate()?;
    Ok(table)
}

impl Hip4BarsTable {
    /// Validate non-emptiness, positive monotonic open timestamps, and required
    /// fields.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.rows.is_empty(),
            "HIP-4 bars table for {} is empty (no candle matched the requested coin {})",
            self.nt_instrument_id,
            self.trade_coin,
        );
        for field in [&self.nt_instrument_id, &self.trade_coin] {
            ensure!(!field.trim().is_empty(), "empty identity field");
        }
        let mut previous = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(row.open_time > 0, "bar row {index}: non-positive open_time");
            ensure!(
                row.open_time >= previous,
                "bar row {index}: open_time {} precedes previous {}",
                row.open_time,
                previous,
            );
            previous = row.open_time;
            for field in [&row.open, &row.high, &row.low, &row.close, &row.volume] {
                ensure!(!field.trim().is_empty(), "bar row {index}: empty field");
            }
        }
        Ok(())
    }

    /// The NautilusTrader bar-type string for catalog identifier resolution.
    ///
    /// # Errors
    ///
    /// Returns an error if the spec instrument id is invalid.
    pub fn bar_type_string(&self, spec: &Hip4MarketDataSpec) -> Result<String> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id,
        );
        Ok(spec.bar_type()?.to_string())
    }

    /// Convert the canonical rows into NautilusTrader `Bar`s at the instrument's
    /// price/size precision under the candle bar type.
    ///
    /// Bars are stamped at candle open time (`ts_event`), matching the open-time
    /// monotonicity the table validates.
    ///
    /// # Errors
    ///
    /// Returns an error if an OHLCV value cannot be represented at the precision
    /// or fails NautilusTrader's OHLC checks.
    pub fn to_bars(&self, spec: &Hip4MarketDataSpec) -> Result<Vec<Bar>> {
        ensure!(
            spec.nt_instrument_id == self.nt_instrument_id,
            "spec instrument {:?} does not match table {:?}",
            spec.nt_instrument_id,
            self.nt_instrument_id,
        );
        let bar_type = spec.bar_type()?;
        let price_precision = spec.price_precision();
        let size_precision = spec.size_precision();
        self.rows
            .iter()
            .map(|row| {
                let open = price_at(&row.open, price_precision)?;
                let high = price_at(&row.high, price_precision)?;
                let low = price_at(&row.low, price_precision)?;
                let close = price_at(&row.close, price_precision)?;
                let volume = quantity_at(&row.volume, size_precision)?;
                let ts =
                    UnixNanos::from(u64::try_from(row.open_time).context("negative open_time")?);
                Bar::new_checked(bar_type, open, high, low, close, volume, ts, ts)
                    .context("build NautilusTrader bar")
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Shared price/size conversion (trades + bars)
// ---------------------------------------------------------------------------

fn price_at(value: &str, precision: u8) -> Result<Price> {
    let scaled = rescaled(value, precision)?;
    Price::from_str(&scaled).map_err(|error| anyhow::anyhow!("invalid price {scaled:?}: {error}"))
}

fn quantity_at(value: &str, precision: u8) -> Result<Quantity> {
    let scaled = rescaled(value, precision)?;
    Quantity::from_str(&scaled)
        .map_err(|error| anyhow::anyhow!("invalid quantity {scaled:?}: {error}"))
}

// ---------------------------------------------------------------------------
// Catalog projection + read-back (trades + bars)
// ---------------------------------------------------------------------------

/// Result of projecting a HIP-4 trades/bars table into a NautilusTrader catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4MarketDataProjection {
    pub catalog_root: PathBuf,
    /// The catalog identifier the data was written under (instrument id for
    /// trades, bar-type string for bars).
    pub nt_identifier: String,
    pub data_type: String,
    pub record_count: usize,
}

/// Project a canonical HIP-4 trades table into a NautilusTrader
/// `ParquetDataCatalog` as `TradeTick` data.
///
/// Fails closed on a non-empty `catalog_root`: NautilusTrader's
/// `write_to_parquet` skips writing when a file for the same identifier/interval
/// already exists, so projecting into a dirty root could silently read back stale
/// data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails, or the root is dirty.
pub fn project_hip4_trades_to_catalog(
    table: &Hip4TradesTable,
    spec: &Hip4MarketDataSpec,
    catalog_root: &Path,
) -> Result<Hip4MarketDataProjection> {
    table.validate()?;
    let ticks = table.to_trade_ticks(spec)?;
    let record_count = ticks.len();
    ensure_clean_market_data_root(catalog_root)?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    Ok(Hip4MarketDataProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: spec.nt_instrument_id.clone(),
        data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
        record_count,
    })
}

/// Project a canonical HIP-4 bars table into a NautilusTrader
/// `ParquetDataCatalog` as `Bar` data.
///
/// # Errors
///
/// Returns an error if conversion or the catalog write fails, or the root is dirty.
pub fn project_hip4_bars_to_catalog(
    table: &Hip4BarsTable,
    spec: &Hip4MarketDataSpec,
    catalog_root: &Path,
) -> Result<Hip4MarketDataProjection> {
    table.validate()?;
    let bars = table.to_bars(spec)?;
    let record_count = bars.len();
    ensure_clean_market_data_root(catalog_root)?;

    let catalog = ParquetDataCatalog::new(catalog_root, None, None, None, None);
    catalog
        .write_to_parquet(bars, None, None, None)
        .context("write bars to catalog")?;

    Ok(Hip4MarketDataProjection {
        catalog_root: catalog_root.to_path_buf(),
        nt_identifier: table.bar_type_string(spec)?,
        data_type: NT_DATA_TYPE_BAR.to_string(),
        record_count,
    })
}

/// Read the projected `TradeTick` data back from `catalog_root`.
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
        .context("query trade ticks from catalog")
}

/// Read the projected `Bar` data back from `catalog_root` by its bar-type id.
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
        .context("query bars from catalog")
}

fn ensure_clean_market_data_root(catalog_root: &Path) -> Result<()> {
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
    Ok(())
}

// ===========================================================================
// Bulk-append path (data-derived identity + precision, no clean-root guard)
//
// The `project_*_to_catalog` functions above are the hermetic single-object
// TEST harness: they refuse a dirty catalog root so a round-trip proof can read
// back exactly what it wrote. The bulk-conversion path below instead appends
// every instrument of one staged object into an already-open, shared (possibly
// S3-backed) [`ParquetDataCatalog`] with NO clean-root guard, relying on
// NautilusTrader's own per-instrument / per-time-range file naming so many
// objects flow into one catalog.
//
// HIP-4 stages no instrument universe, so identity and precision are derived
// from the object's own rows. All three families resolve their catalog id the
// same way — via [`Hip4InstrumentNaming`] (per-venue `outcome_symbol_prefix` +
// `nt_venue_code`) applied to the record's own integers:
//   * L2 snapshots are per-outcome; [`parse_hip4_snapshots`] already enumerates
//     every outcome in the object and derives each instrument's precision from
//     its own level data, so the append fn reuses it directly.
//   * trades/bars are per-`trade_coin`; the append fns enumerate the distinct
//     `(trade_coin, outcome, side)` tuples, derive each instrument's
//     `nt_instrument_id` as `<prefix><outcome>-<side>.<venue_code>` (URI-safe,
//     matching the snapshot scheme so a market's trades, bars, and book share a
//     consistent id), and derive price/size precision as the max decimal places
//     observed in that instrument's own rows. The raw `#`-prefixed `trade_coin`
//     handle is used only as the per-coin fence the normalizers filter on; it
//     never reaches the catalog id (an id with `#` is mangled on read-back).
//
// HIP-4's converters require no provenance/identity struct beyond this derived
// `nt_instrument_id` + precision (and, for bars, the candle interval read from
// the object), so nothing about the object's origin is fabricated and the append
// fns need no `object_key` argument.
// ===========================================================================

use std::collections::BTreeSet;

/// The decimal-string increment whose fractional length is exactly `precision`
/// (`0 -> "1"`, `1 -> "0.1"`, `5 -> "0.00001"`) — the inverse of
/// [`decimal_places`]. Lets a data-derived precision be expressed as the
/// [`Hip4MarketDataSpec`] increment the converter consumes.
#[must_use]
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}

/// One instrument's write summary produced by the bulk-append functions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hip4AppendSummary {
    /// Resolved NautilusTrader catalog identifier (instrument id for snapshots
    /// and trades, bar-type string for bars).
    pub nt_identifier: String,
    /// NautilusTrader data type written (`OrderBookDelta`, `TradeTick`, `Bar`).
    pub data_type: String,
    /// Records written for this instrument.
    pub record_count: usize,
    /// Data-derived price precision (max decimal places observed).
    pub price_precision: u8,
    /// Data-derived size precision (max decimal places observed).
    pub size_precision: u8,
}

/// Append every outcome's fixed-depth L2 snapshots from one staged
/// `table=order_book_snapshots_fixed_depth` JSONL object into an already-open
/// [`ParquetDataCatalog`] — the bulk-conversion path for `OrderBookDelta` data.
///
/// Reuses [`parse_hip4_snapshots`] (which enumerates every outcome in the object
/// and derives each instrument's precision from its own level data) and
/// [`snapshots_to_deltas`]. Unlike [`project_hip4_snapshots_to_catalog`] (the
/// hermetic proof harness, which refuses a dirty root), this appends into a
/// shared catalog with no clean-root guard. The supplied [`Hip4InstrumentNaming`]
/// is a per-venue format constant (NT venue code + outcome-symbol prefix +
/// expected source venue), not per-instrument universe metadata: every numeric
/// outcome id is read from the object's own records. Returns one summary per
/// distinct outcome instrument written.
///
/// # Errors
///
/// Returns an error if parsing/delta construction fails or a catalog write fails.
pub fn append_hip4_snapshots_archive(
    jsonl: &str,
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    let table = parse_hip4_snapshots(jsonl, naming)?;
    ensure!(
        !table.instruments.is_empty(),
        "HIP-4 snapshot object yielded no instruments",
    );
    let mut summaries = Vec::with_capacity(table.instruments.len());
    for instrument in &table.instruments {
        let deltas = snapshots_to_deltas(instrument)?;
        ensure!(
            !deltas.is_empty(),
            "instrument {} produced no deltas",
            instrument.nt_instrument_id,
        );
        let record_count = deltas.len();
        // Keep this instrument's deltas in a single encoder chunk so the
        // per-chunk precision metadata stays consistent: NautilusTrader's
        // `OrderBookDelta` chunk_metadata reads precision from the chunk's first
        // (or second, when the first is a zero-precision Clear) delta, so a chunk
        // boundary that split an instrument's photos could mis-stamp precision.
        // The proof harness achieves the same by sizing batch_size at construction;
        // here the catalog is supplied already-open, so the public `batch_size`
        // field is set to fit this instrument before its write.
        catalog.batch_size = catalog.batch_size.max(record_count);
        catalog
            .write_to_parquet(deltas, None, None, None)
            .with_context(|| {
                format!(
                    "append HIP-4 order book deltas for {} to catalog",
                    instrument.nt_instrument_id
                )
            })?;
        summaries.push(Hip4AppendSummary {
            nt_identifier: instrument.nt_instrument_id.clone(),
            data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
            record_count,
            price_precision: instrument.price_precision,
            size_precision: instrument.size_precision,
        });
    }
    Ok(summaries)
}

/// Append every outcome's book photos from SEVERAL staged
/// `order_book_snapshots_fixed_depth` objects into an already-open
/// [`ParquetDataCatalog`], deduplicating photos by `snapshot_time` per outcome
/// across all objects.
///
/// Like trades and bars, a future overlapping capture would re-stage the same
/// outcome's photo at a shared `snapshot_time` across `run=` partitions; each
/// photo expands to deltas at that time, so per-object writes overlap and
/// NautilusTrader's `write_to_parquet` rejects the non-disjoint second write.
/// This collects every object's photos keyed by `(outcome, snapshot_time)` so
/// duplicates collapse, takes the max precision per outcome across all objects,
/// then writes one ascending `OrderBookDelta` stream per outcome. The snapshots
/// analogue of [`append_hip4_trades_archive_batch`].
///
/// A `snapshot_time` seen twice for one outcome with disagreeing levels is
/// corrupt and fails loud rather than silently keeping last-seen.
///
/// # Errors
///
/// Returns an error if an object is not UTF-8 or valid JSON, a duplicate photo
/// disagrees, a photo produces no deltas, table validation fails, or a catalog
/// write fails.
pub fn append_hip4_snapshots_archive_batch(
    objects: &[(String, Vec<u8>)],
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    // instrument id -> (ts_event -> photo); the inner BTreeMap dedups by the
    // per-outcome unique snapshot time and keeps photos ascending. instrument id
    // -> (price_prec, size_prec) takes the max precision observed across objects.
    let mut photos_by_instrument: BTreeMap<String, BTreeMap<u64, Hip4InstrumentSnapshot>> =
        BTreeMap::new();
    let mut prec_by_instrument: BTreeMap<String, (u8, u8)> = BTreeMap::new();

    for (object_key, bytes) in objects {
        let jsonl = std::str::from_utf8(bytes)
            .with_context(|| format!("HIP-4 snapshot object {object_key} is not UTF-8"))?;
        let table = parse_hip4_snapshots(jsonl, naming)?;
        for instrument in table.instruments {
            let prec = prec_by_instrument
                .entry(instrument.nt_instrument_id.clone())
                .or_insert((0, 0));
            prec.0 = prec.0.max(instrument.price_precision);
            prec.1 = prec.1.max(instrument.size_precision);
            let dedup = photos_by_instrument
                .entry(instrument.nt_instrument_id.clone())
                .or_default();
            for photo in instrument.snapshots {
                let key = photo.ts_event.as_u64();
                if let Some(existing) = dedup.get(&key) {
                    ensure!(
                        existing == &photo,
                        "outcome {} snapshot_time {} disagrees on levels across overlapping objects",
                        instrument.nt_instrument_id,
                        key,
                    );
                    continue;
                }
                dedup.insert(key, photo);
            }
        }
    }

    let mut summaries = Vec::new();
    for (nt_instrument_id, photos_map) in photos_by_instrument {
        let (price_precision, size_precision) = prec_by_instrument
            .remove(&nt_instrument_id)
            .expect("instrument has a precision template");
        // The dedup map is keyed by ts_event, so the photos are already ascending.
        let snapshots: Vec<Hip4InstrumentSnapshot> = photos_map.into_values().collect();
        let instrument = Hip4InstrumentSnapshots {
            nt_instrument_id: nt_instrument_id.clone(),
            price_precision,
            size_precision,
            snapshots,
        };
        let deltas = snapshots_to_deltas(&instrument)?;
        ensure!(
            !deltas.is_empty(),
            "instrument {nt_instrument_id} produced no deltas",
        );
        let record_count = deltas.len();
        // Keep this instrument's deltas in a single encoder chunk so the per-chunk
        // precision metadata stays consistent (see append_hip4_snapshots_archive).
        catalog.batch_size = catalog.batch_size.max(record_count);
        catalog
            .write_to_parquet(deltas, None, None, None)
            .with_context(|| {
                format!("append deduplicated HIP-4 order book deltas for {nt_instrument_id}")
            })?;
        summaries.push(Hip4AppendSummary {
            nt_identifier: nt_instrument_id,
            data_type: NT_DATA_TYPE_ORDER_BOOK_DELTA.to_string(),
            record_count,
            price_precision,
            size_precision,
        });
    }
    ensure!(
        !summaries.is_empty(),
        "HIP-4 snapshots batch yielded no outcomes"
    );
    Ok(summaries)
}

/// One distinct tradeable HIP-4 coin in a staged trades/bars object: the HL-native
/// handle (the per-coin fence) plus the `(outcome, side)` integers that resolve
/// its catalog id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Hip4Coin {
    /// Numeric prediction-market outcome id (for example `101`).
    outcome: i64,
    /// Binary-leg selector within the outcome (for example `0`/`1`).
    side: i64,
    /// HL-native tradeable coin handle (for example `#1010`); the per-coin fence.
    trade_coin: String,
}

/// Distinct tradeable coins appearing in a staged HIP-4 trades or bars JSONL
/// object, in deterministic order (by `(outcome, side, trade_coin)`).
///
/// A staged object interleaves every coin's records, so the bulk converters write
/// one catalog stream per distinct coin rather than assuming a single one. Each
/// record carries the coin handle plus its `(outcome, side)` integers; both are
/// read so the catalog id can be derived the same data-driven way the L2
/// snapshot family derives its id. A `(outcome, side)` pair that maps to more than
/// one `trade_coin` (or a coin that maps to more than one `(outcome, side)`)
/// fails loud rather than silently colliding two instruments under one id.
///
/// # Errors
///
/// Returns an error if a non-blank line is not valid JSON, or the object's
/// `(outcome, side)` <-> `trade_coin` mapping is not one-to-one.
fn hip4_distinct_coins(jsonl: &str) -> Result<Vec<Hip4Coin>> {
    /// Minimal projection of a staged record: the coin handle and the
    /// `(outcome, side)` integers that resolve its catalog id.
    #[derive(Deserialize)]
    struct CoinIdentity {
        trade_coin: String,
        outcome: i64,
        side: i64,
    }
    let mut seen: BTreeSet<Hip4Coin> = BTreeSet::new();
    for (line_no, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: CoinIdentity = serde_json::from_str(line)
            .with_context(|| format!("read coin identity on line {}", line_no + 1))?;
        seen.insert(Hip4Coin {
            outcome: record.outcome,
            side: record.side,
            trade_coin: record.trade_coin,
        });
    }
    let coins: Vec<Hip4Coin> = seen.into_iter().collect();
    // A staged object's `(outcome, side)` <-> `trade_coin` mapping must be
    // one-to-one: two coins resolving to the same `(outcome, side)` would collide
    // under one catalog id, and one coin carrying two `(outcome, side)` pairs
    // would be split across ids. Either is a corrupt object; fail loud. Checked
    // against the full set (not just neighbours) so it is independent of ordering.
    let mut by_outcome_side: BTreeMap<(i64, i64), &str> = BTreeMap::new();
    let mut by_coin: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    for coin in &coins {
        if let Some(other) = by_outcome_side.insert((coin.outcome, coin.side), &coin.trade_coin) {
            bail!(
                "HIP-4 object maps outcome {} side {} to two coins {:?} and {:?}",
                coin.outcome,
                coin.side,
                other,
                coin.trade_coin,
            );
        }
        if let Some((o, s)) = by_coin.insert(&coin.trade_coin, (coin.outcome, coin.side)) {
            bail!(
                "HIP-4 object maps coin {:?} to two (outcome, side) pairs ({o}, {s}) and ({}, {})",
                coin.trade_coin,
                coin.outcome,
                coin.side,
            );
        }
    }
    Ok(coins)
}

/// Build a [`Hip4MarketDataSpec`] for one coin whose `nt_instrument_id` and
/// price/size precision are derived from the object's own rows.
///
/// The catalog `nt_instrument_id` is resolved the SAME way the L2 snapshot family
/// resolves its id — `<outcome_symbol_prefix><outcome>-<side>.<nt_venue_code>`
/// (for example `OUTCOME-101-0.HYPERLIQUID`) — using the per-venue
/// [`Hip4InstrumentNaming`] format constant and the coin's own `(outcome, side)`
/// integers. This is URI-safe (unlike the raw `#1010` handle, whose `#` is a URI
/// fragment delimiter that `ParquetDataCatalog` mangles on read-back) and aligns
/// trades, bars, and book for the same market under matching ids; no instrument
/// universe is consulted (HIP-4 stages none). The `trade_coin` handle is kept as
/// the per-coin fence the normalizers filter on, not as the catalog id.
///
/// Price/size precision is the max decimal places the exchange rendered in
/// `price`/`size` for this coin (expressed as the decimal-string increment the
/// spec consumes). The bar step/aggregation come from the supplied
/// `bar_aggregation`/`bar_step` (read from the candle `interval` by the bars
/// append path; the trades path passes a placeholder bar spec that the trade
/// projection never reads).
fn hip4_spec_from_precision(
    coin: &Hip4Coin,
    naming: &Hip4InstrumentNaming,
    price_precision: u8,
    size_precision: u8,
    bar_step: usize,
    bar_aggregation: Hip4BarAggregation,
) -> Hip4MarketDataSpec {
    Hip4MarketDataSpec {
        expected_venue: naming.expected_venue.clone(),
        trade_coin: coin.trade_coin.clone(),
        nt_instrument_id: format!(
            "{}{}-{}.{}",
            naming.outcome_symbol_prefix, coin.outcome, coin.side, naming.nt_venue_code,
        ),
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
        bar_step,
        bar_aggregation,
    }
}

/// Append every coin's prints from one staged `table=trades_recent` JSONL object
/// into an already-open [`ParquetDataCatalog`] — the bulk-conversion path for
/// `TradeTick` data.
///
/// Enumerates the distinct `(trade_coin, outcome, side)` tuples, derives each
/// instrument's catalog `nt_instrument_id` from its `(outcome, side)` via the
/// per-venue [`Hip4InstrumentNaming`] (the same URI-safe scheme the L2 snapshot
/// family uses) and its price/size precision from that coin's own rows
/// ([`hip4_spec_from_precision`]), then reuses [`normalize_hip4_trades`] (which
/// fences out other coins by `trade_coin`, sorts ascending, and validates) and
/// [`Hip4TradesTable::to_trade_ticks`]. No clean-root guard. `naming` is the same
/// per-venue format constant the L2 path consumes. The trade projection never
/// reads the bar spec, so a unit `Minute` step is used as a harmless placeholder.
/// Returns one summary per distinct coin written.
///
/// # Errors
///
/// Returns an error if enumeration, normalization, tick construction, or a
/// catalog write fails, or if the object yields no coins.
pub fn append_hip4_trades_archive(
    jsonl: &str,
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    let coins = hip4_distinct_coins(jsonl)?;
    let mut summaries = Vec::new();
    for coin in &coins {
        // Derive precision from this coin's own rows. A pre-pass with precision 0
        // collects the rows so the max observed decimals can be measured, then the
        // real spec is built and the ticks rebuilt at that precision.
        let probe = hip4_spec_from_precision(coin, naming, 0, 0, 1, Hip4BarAggregation::Minute);
        let table = normalize_hip4_trades(jsonl, &probe)?;
        let mut price_precision = 0u8;
        let mut size_precision = 0u8;
        for row in &table.rows {
            price_precision = price_precision.max(decimal_places(&row.price));
            size_precision = size_precision.max(decimal_places(&row.size));
        }
        let spec = hip4_spec_from_precision(
            coin,
            naming,
            price_precision,
            size_precision,
            1,
            Hip4BarAggregation::Minute,
        );
        let ticks = table.to_trade_ticks(&spec)?;
        let record_count = ticks.len();
        catalog
            .write_to_parquet(ticks, None, None, None)
            .with_context(|| format!("append HIP-4 trade ticks for {}", coin.trade_coin))?;
        summaries.push(Hip4AppendSummary {
            nt_identifier: spec.nt_instrument_id.clone(),
            data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
            record_count,
            price_precision,
            size_precision,
        });
    }
    ensure!(
        !summaries.is_empty(),
        "HIP-4 trades object yielded no coins"
    );
    Ok(summaries)
}

/// Append every coin's prints from SEVERAL staged `table=trades_recent` objects
/// into an already-open [`ParquetDataCatalog`], deduplicating prints by `tid` per
/// coin across all objects.
///
/// HL `recentTrades` is a rolling buffer: successive `run=` partitions re-fetch
/// overlapping prints, so writing each object's stream as its own catalog file
/// produces non-disjoint time intervals for the same instrument and
/// NautilusTrader's `write_to_parquet` rejects the overlapping write. This
/// collects every object's prints keyed by `(coin, tid)` (the per-coin unique
/// trade id) so duplicates collapse, derives one uniform precision per coin
/// across all objects, sorts the deduped union by event time, then writes one
/// ascending `TradeTick` stream per coin — the single disjoint write the catalog
/// contract expects. The trades analogue of
/// [`append_bybit_mark_price_kline_1m_batch`](crate::canonical_bybit::append_bybit_mark_price_kline_1m_batch).
///
/// A `tid` seen twice with disagreeing price/size/time is corrupt (a tid is
/// unique within a coin) and fails loud rather than silently keeping last-seen.
///
/// # Errors
///
/// Returns an error if an object is not UTF-8 or valid JSON, the
/// `(outcome, side)` <-> coin mapping is inconsistent across objects, a duplicate
/// tid disagrees, a value cannot be represented at the derived precision, table
/// validation fails, or a catalog write fails.
pub fn append_hip4_trades_archive_batch(
    objects: &[(String, Vec<u8>)],
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    // coin handle -> (trade_id -> row); the inner BTreeMap key dedups by the
    // per-coin unique tid. coin handle -> Hip4Coin keeps the (outcome, side)
    // catalog identity, asserted consistent across objects.
    let mut rows_by_coin: BTreeMap<String, BTreeMap<String, Hip4TradeRow>> = BTreeMap::new();
    let mut coin_meta: BTreeMap<String, Hip4Coin> = BTreeMap::new();

    for (object_key, bytes) in objects {
        let jsonl = std::str::from_utf8(bytes)
            .with_context(|| format!("HIP-4 trades object {object_key} is not UTF-8"))?;
        for coin in hip4_distinct_coins(jsonl)? {
            match coin_meta.get(&coin.trade_coin) {
                Some(existing) => ensure!(
                    existing == &coin,
                    "coin {} maps to different (outcome, side) across objects",
                    coin.trade_coin,
                ),
                None => {
                    coin_meta.insert(coin.trade_coin.clone(), coin.clone());
                }
            }
            let probe =
                hip4_spec_from_precision(&coin, naming, 0, 0, 1, Hip4BarAggregation::Minute);
            let table = normalize_hip4_trades(jsonl, &probe)?;
            let dedup = rows_by_coin.entry(coin.trade_coin.clone()).or_default();
            for row in &table.rows {
                if let Some(existing) = dedup.get(&row.trade_id) {
                    ensure!(
                        existing == row,
                        "coin {} tid {} disagrees on px/sz/time across overlapping objects",
                        coin.trade_coin,
                        row.trade_id,
                    );
                    continue;
                }
                dedup.insert(row.trade_id.clone(), row.clone());
            }
        }
    }

    let mut summaries = Vec::new();
    for (coin_handle, rows_map) in rows_by_coin {
        let coin = coin_meta
            .remove(&coin_handle)
            .expect("coin has a metadata template");
        // The dedup map is keyed by tid; the catalog contract needs ascending
        // event time, so re-sort the deduped union by event time.
        let mut rows: Vec<Hip4TradeRow> = rows_map.into_values().collect();
        rows.sort_by_key(|row| row.event_time);
        let mut price_precision = 0u8;
        let mut size_precision = 0u8;
        for row in &rows {
            price_precision = price_precision.max(decimal_places(&row.price));
            size_precision = size_precision.max(decimal_places(&row.size));
        }
        let spec = hip4_spec_from_precision(
            &coin,
            naming,
            price_precision,
            size_precision,
            1,
            Hip4BarAggregation::Minute,
        );
        let table = Hip4TradesTable {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            trade_coin: coin.trade_coin.clone(),
            rows,
        };
        table.validate()?;
        let ticks = table.to_trade_ticks(&spec)?;
        let record_count = ticks.len();
        catalog
            .write_to_parquet(ticks, None, None, None)
            .with_context(|| {
                format!(
                    "append deduplicated HIP-4 trade ticks for {}",
                    coin.trade_coin
                )
            })?;
        summaries.push(Hip4AppendSummary {
            nt_identifier: spec.nt_instrument_id.clone(),
            data_type: NT_DATA_TYPE_TRADE_TICK.to_string(),
            record_count,
            price_precision,
            size_precision,
        });
    }
    ensure!(!summaries.is_empty(), "HIP-4 trades batch yielded no coins");
    Ok(summaries)
}

/// Map a HL candle `interval` token (for example `1h`, `15m`, `1d`) to a
/// `(bar_step, Hip4BarAggregation)` pair.
///
/// The token is `<step><unit>` where unit is `s` (second), `m` (minute), `h`
/// (hour), `d` (day), `w` (week), or `M` (month) — the full HL candle vocabulary,
/// each of which NautilusTrader's [`nautilus_model::enums::BarAggregation`]
/// models. Note `m` (minute) and `M` (month) are case-distinct, matching HL.
///
/// # Errors
///
/// Returns an error if the token is empty, the step is not a positive integer,
/// or the unit is unsupported.
fn parse_hip4_bar_interval(interval: &str) -> Result<(usize, Hip4BarAggregation)> {
    let interval = interval.trim();
    ensure!(!interval.is_empty(), "empty candle interval");
    let (digits, unit) = interval.split_at(
        interval
            .find(|c: char| !c.is_ascii_digit())
            .with_context(|| format!("candle interval {interval:?} has no unit suffix"))?,
    );
    let step: usize = digits
        .parse()
        .with_context(|| format!("candle interval {interval:?} has non-integer step"))?;
    ensure!(
        step > 0,
        "candle interval {interval:?} has non-positive step"
    );
    let aggregation = match unit {
        "s" => Hip4BarAggregation::Second,
        "m" => Hip4BarAggregation::Minute,
        "h" => Hip4BarAggregation::Hour,
        "d" => Hip4BarAggregation::Day,
        "w" => Hip4BarAggregation::Week,
        "M" => Hip4BarAggregation::Month,
        other => bail!("unsupported candle interval unit {other:?} in {interval:?}"),
    };
    Ok((step, aggregation))
}

/// Append every `(trade_coin, interval)` group's candles from one staged
/// `table=bars` JSONL object into an already-open [`ParquetDataCatalog`] — the
/// bulk-conversion path for `Bar` data.
///
/// Enumerates the distinct `(trade_coin, outcome, side)` tuples; for each, walks
/// every candle `interval` that coin carries ([`hip4_bar_intervals_for_coin`])
/// and writes one NautilusTrader bar stream per `(coin, interval)`. A staged
/// object interleaves every coin's candles at every published interval (1m, 3m,
/// ... 1w, 1M), so a single coin yields several bar streams — not one. Per
/// stream: the bar step/aggregation come from the interval token
/// ([`parse_hip4_bar_interval`]); the catalog `nt_instrument_id` comes from the
/// coin's `(outcome, side)` via the per-venue [`Hip4InstrumentNaming`] (the same
/// URI-safe scheme the L2 snapshot family uses); price/size precision is derived
/// from that stream's own OHLCV rows ([`hip4_spec_from_precision`]); then reuses
/// [`normalize_hip4_bars`] (interval-filtered) and [`Hip4BarsTable::to_bars`]. No
/// clean-root guard. Returns one summary per `(coin, interval)` stream written.
///
/// # Errors
///
/// Returns an error if enumeration, interval parsing, normalization, bar
/// construction, or a catalog write fails, or if the object yields no coins.
pub fn append_hip4_bars_archive(
    jsonl: &str,
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    let coins = hip4_distinct_coins(jsonl)?;
    let mut summaries = Vec::new();
    for coin in &coins {
        // Each coin carries candles at several intervals; each is its own NT bar
        // stream (a distinct bar type), so write one per interval.
        let intervals = hip4_bar_intervals_for_coin(jsonl, &coin.trade_coin)?;
        for interval in &intervals {
            let (bar_step, bar_aggregation) = parse_hip4_bar_interval(interval)?;
            // Pre-pass at precision 0 to collect this (coin, interval)'s rows,
            // then derive precision from the observed OHLCV decimals and rebuild.
            let probe = hip4_spec_from_precision(coin, naming, 0, 0, bar_step, bar_aggregation);
            let table = normalize_hip4_bars(jsonl, &probe)?;
            let mut price_precision = 0u8;
            let mut size_precision = 0u8;
            for row in &table.rows {
                for field in [&row.open, &row.high, &row.low, &row.close] {
                    price_precision = price_precision.max(decimal_places(field));
                }
                size_precision = size_precision.max(decimal_places(&row.volume));
            }
            let spec = hip4_spec_from_precision(
                coin,
                naming,
                price_precision,
                size_precision,
                bar_step,
                bar_aggregation,
            );
            let bars = table.to_bars(&spec)?;
            let record_count = bars.len();
            catalog
                .write_to_parquet(bars, None, None, None)
                .with_context(|| {
                    format!(
                        "append HIP-4 bars for {} interval {interval}",
                        coin.trade_coin
                    )
                })?;
            summaries.push(Hip4AppendSummary {
                nt_identifier: table.bar_type_string(&spec)?,
                data_type: NT_DATA_TYPE_BAR.to_string(),
                record_count,
                price_precision,
                size_precision,
            });
        }
    }
    ensure!(!summaries.is_empty(), "HIP-4 bars object yielded no coins");
    Ok(summaries)
}

/// Append every coin's candles from SEVERAL staged `table=bars` objects into an
/// already-open [`ParquetDataCatalog`], deduplicating candles by `open_time` per
/// `(coin, interval)` stream across all objects.
///
/// HL `candleSnapshot` is a rolling buffer like `recentTrades`: successive `run=`
/// partitions re-fetch overlapping candles for the same `(coin, interval)`, so
/// writing each object's stream as its own catalog file produces non-disjoint
/// time intervals and NautilusTrader's `write_to_parquet` rejects the overlapping
/// write. This collects every object's candles keyed by
/// `(coin, interval, open_time)` (the per-stream unique candle key) so duplicates
/// collapse, derives one uniform precision per stream, then writes one ascending
/// `Bar` stream per `(coin, interval)`. The bars analogue of
/// [`append_hip4_trades_archive_batch`].
///
/// An `open_time` seen twice within a stream keeps the later batch object's row:
/// HIP-4 staged `candleSnapshot` captures can revise still-open candles across
/// overlapping `run=` partitions.
///
/// # Errors
///
/// Returns an error if an object is not UTF-8 or valid JSON, the
/// `(outcome, side)` <-> coin mapping is inconsistent across objects, an interval
/// is unsupported, a value cannot be represented at the derived precision, table
/// validation fails, or a catalog write fails.
pub fn append_hip4_bars_archive_batch(
    objects: &[(String, Vec<u8>)],
    naming: &Hip4InstrumentNaming,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<Hip4AppendSummary>> {
    // (coin handle, interval) -> (open_time -> row); the inner BTreeMap key dedups
    // by the per-stream unique open_time and keeps candles ascending. coin handle
    // -> Hip4Coin keeps the (outcome, side) identity, asserted consistent.
    let mut rows_by_stream: BTreeMap<(String, String), BTreeMap<i64, Hip4BarRow>> = BTreeMap::new();
    let mut coin_meta: BTreeMap<String, Hip4Coin> = BTreeMap::new();

    for (object_key, bytes) in objects {
        let jsonl = std::str::from_utf8(bytes)
            .with_context(|| format!("HIP-4 bars object {object_key} is not UTF-8"))?;
        for coin in hip4_distinct_coins(jsonl)? {
            match coin_meta.get(&coin.trade_coin) {
                Some(existing) => ensure!(
                    existing == &coin,
                    "coin {} maps to different (outcome, side) across objects",
                    coin.trade_coin,
                ),
                None => {
                    coin_meta.insert(coin.trade_coin.clone(), coin.clone());
                }
            }
            for interval in hip4_bar_intervals_for_coin(jsonl, &coin.trade_coin)? {
                let (bar_step, bar_aggregation) = parse_hip4_bar_interval(&interval)?;
                let probe =
                    hip4_spec_from_precision(&coin, naming, 0, 0, bar_step, bar_aggregation);
                let table = normalize_hip4_bars(jsonl, &probe)?;
                let dedup = rows_by_stream
                    .entry((coin.trade_coin.clone(), interval.clone()))
                    .or_default();
                for row in &table.rows {
                    dedup.insert(row.open_time, row.clone());
                }
            }
        }
    }

    let mut summaries = Vec::new();
    for ((coin_handle, interval), rows_map) in rows_by_stream {
        let coin = coin_meta
            .get(&coin_handle)
            .expect("coin has a metadata template")
            .clone();
        let (bar_step, bar_aggregation) = parse_hip4_bar_interval(&interval)?;
        // The dedup map is keyed by open_time, so the values are already ascending.
        let rows: Vec<Hip4BarRow> = rows_map.into_values().collect();
        let mut price_precision = 0u8;
        let mut size_precision = 0u8;
        for row in &rows {
            for field in [&row.open, &row.high, &row.low, &row.close] {
                price_precision = price_precision.max(decimal_places(field));
            }
            size_precision = size_precision.max(decimal_places(&row.volume));
        }
        let spec = hip4_spec_from_precision(
            &coin,
            naming,
            price_precision,
            size_precision,
            bar_step,
            bar_aggregation,
        );
        let table = Hip4BarsTable {
            nt_instrument_id: spec.nt_instrument_id.clone(),
            trade_coin: coin.trade_coin.clone(),
            rows,
        };
        table.validate()?;
        let bars = table.to_bars(&spec)?;
        let record_count = bars.len();
        catalog
            .write_to_parquet(bars, None, None, None)
            .with_context(|| {
                format!(
                    "append deduplicated HIP-4 bars for {} interval {interval}",
                    coin.trade_coin
                )
            })?;
        summaries.push(Hip4AppendSummary {
            nt_identifier: table.bar_type_string(&spec)?,
            data_type: NT_DATA_TYPE_BAR.to_string(),
            record_count,
            price_precision,
            size_precision,
        });
    }
    ensure!(!summaries.is_empty(), "HIP-4 bars batch yielded no coins");
    Ok(summaries)
}

/// The distinct candle intervals carried for one `trade_coin` in a staged bars
/// object, in deterministic (sorted) order.
///
/// A HIP-4 `info.candleSnapshot` object stages every interval the venue
/// publishes (1m, 3m, ... 1w, 1M) for each coin; each is its own NautilusTrader
/// bar stream, so the bulk converter writes one per interval. Every interval is
/// validated against [`parse_hip4_bar_interval`] (fail loud on an unknown unit)
/// before it becomes a stream.
///
/// # Errors
///
/// Returns an error if a record is not valid JSON, an interval is unsupported, or
/// the coin has no candles.
fn hip4_bar_intervals_for_coin(jsonl: &str, trade_coin: &str) -> Result<Vec<String>> {
    /// Minimal projection: the coin handle and the candle interval.
    #[derive(Deserialize)]
    struct CoinInterval {
        trade_coin: String,
        interval: String,
    }
    let mut intervals: BTreeSet<String> = BTreeSet::new();
    for (line_no, line) in jsonl.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let record: CoinInterval = serde_json::from_str(line)
            .with_context(|| format!("read interval on line {}", line_no + 1))?;
        if record.trade_coin != trade_coin {
            continue;
        }
        parse_hip4_bar_interval(&record.interval)
            .with_context(|| format!("line {}", line_no + 1))?;
        intervals.insert(record.interval);
    }
    ensure!(
        !intervals.is_empty(),
        "no candle interval found for coin {trade_coin}"
    );
    Ok(intervals.into_iter().collect())
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
    fn rescale_tolerates_trailing_zero_but_rejects_subprecision() {
        assert_eq!(rescaled("1.0", 0).unwrap(), "1");
        assert_eq!(rescaled("22.0", 0).unwrap(), "22");
        assert_eq!(rescaled("0.50", 1).unwrap(), "0.5");
        let err = rescaled("1.05", 0).expect_err("sub-precision must be refused");
        assert!(err.to_string().contains("more precision"), "{err}");
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

    // -------------------------------------------------------------------
    // Trades + bars unit coverage
    // -------------------------------------------------------------------

    fn trades_spec() -> Hip4MarketDataSpec {
        Hip4MarketDataSpec {
            expected_venue: "hyperliquid".to_string(),
            trade_coin: "#1010".to_string(),
            nt_instrument_id: "OUTCOME-101-UP.HYPERLIQUID".to_string(),
            price_increment: "0.001".to_string(),
            size_increment: "0.1".to_string(),
            bar_step: 1,
            bar_aggregation: Hip4BarAggregation::Hour,
        }
    }

    // Two interleaved coins, newest-first; only #1010 belongs to the spec.
    const TRADES_JSONL: &str = "{\"source_family\":\"info.recentTrades\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1010\",\"tid\":2,\"time\":1780326777342,\"px\":\"0.422\",\"sz\":\"22.0\",\
        \"trade_side\":\"A\"}\n\
        {\"source_family\":\"info.recentTrades\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1011\",\"tid\":99,\"time\":1780326777342,\"px\":\"0.578\",\"sz\":\"22.0\",\
        \"trade_side\":\"B\"}\n\
        {\"source_family\":\"info.recentTrades\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1010\",\"tid\":1,\"time\":1780326740660,\"px\":\"0.5\",\"sz\":\"10.0\",\
        \"trade_side\":\"B\"}";

    const BARS_JSONL: &str = "{\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1010\",\"interval\":\"1h\",\"open_time\":1779710400000,\"close_time\":1779713999999,\
        \"open\":\"0.40\",\"high\":\"0.45\",\"low\":\"0.39\",\"close\":\"0.42\",\"volume\":\"125.0\"}\n\
        {\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1011\",\"interval\":\"1h\",\"open_time\":1779710400000,\"close_time\":1779713999999,\
        \"open\":\"0.60\",\"high\":\"0.61\",\"low\":\"0.55\",\"close\":\"0.58\",\"volume\":\"125.0\"}\n\
        {\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
        \"trade_coin\":\"#1010\",\"interval\":\"1h\",\"open_time\":1779706800000,\"close_time\":1779710399999,\
        \"open\":\"0.41\",\"high\":\"0.42\",\"low\":\"0.40\",\"close\":\"0.40\",\"volume\":\"0.0\"}";

    #[test]
    fn aggressor_token_maps_ask_to_seller_bid_to_buyer() {
        assert_eq!(
            Hip4TradeAggressorSide::from_hl_side("A").unwrap(),
            Hip4TradeAggressorSide::Seller
        );
        assert_eq!(
            Hip4TradeAggressorSide::from_hl_side("B").unwrap(),
            Hip4TradeAggressorSide::Buyer
        );
        assert!(Hip4TradeAggressorSide::from_hl_side("X").is_err());
    }

    #[test]
    fn normalize_trades_filters_coin_sorts_and_maps_aggressor() {
        let table = normalize_hip4_trades(TRADES_JSONL, &trades_spec()).expect("normalize");
        // Only the two #1010 prints are kept; the #1011 print is filtered out.
        assert_eq!(table.rows.len(), 2);
        // Sorted ascending by time: the 1780326740660 print comes first.
        assert_eq!(
            table.rows[0].event_time,
            1_780_326_740_660 * NANOS_PER_MILLISECOND
        );
        assert_eq!(table.rows[0].trade_id, "1");
        assert_eq!(table.rows[0].aggressor_side, Hip4TradeAggressorSide::Buyer);
        assert_eq!(table.rows[1].trade_id, "2");
        assert_eq!(table.rows[1].aggressor_side, Hip4TradeAggressorSide::Seller);
    }

    #[test]
    fn normalize_trades_rejects_empty_when_no_coin_matches() {
        let mut spec = trades_spec();
        spec.trade_coin = "#9999".to_string();
        let err = normalize_hip4_trades(TRADES_JSONL, &spec).expect_err("must reject empty");
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn normalize_trades_rejects_wrong_source_family() {
        let bad = TRADES_JSONL.replace("info.recentTrades", "info.l2Book");
        let err = normalize_hip4_trades(&bad, &trades_spec()).expect_err("must reject");
        assert!(err.to_string().contains("source_family"), "{err}");
    }

    #[test]
    fn normalize_trades_rejects_unexpected_venue() {
        let bad = TRADES_JSONL.replace("\"venue\":\"hyperliquid\"", "\"venue\":\"binance\"");
        let err = normalize_hip4_trades(&bad, &trades_spec()).expect_err("must reject");
        assert!(err.to_string().contains("venue"), "{err}");
    }

    #[test]
    fn normalize_bars_filters_coin_sorts_and_checks_ohlc() {
        let table = normalize_hip4_bars(BARS_JSONL, &trades_spec()).expect("normalize");
        assert_eq!(table.rows.len(), 2);
        // Sorted ascending by open time: the 1779706800000 candle comes first.
        assert_eq!(
            table.rows[0].open_time,
            1_779_706_800_000 * NANOS_PER_MILLISECOND
        );
        assert_eq!(table.rows[0].open, "0.41");
        assert_eq!(table.rows[1].open, "0.40");
    }

    #[test]
    fn normalize_bars_rejects_ohlc_violation() {
        // high < low for the #1010 candle.
        let bad = BARS_JSONL.replace("\"high\":\"0.45\"", "\"high\":\"0.30\"");
        let err = normalize_hip4_bars(&bad, &trades_spec()).expect_err("must reject");
        assert!(err.to_string().contains("OHLC"), "{err}");
    }

    #[test]
    fn trades_project_and_read_back_round_trip() {
        let spec = trades_spec();
        let table = normalize_hip4_trades(TRADES_JSONL, &spec).expect("normalize");
        let built = table.to_trade_ticks(&spec).expect("ticks");

        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection =
            project_hip4_trades_to_catalog(&table, &spec, dir.path()).expect("project");
        assert_eq!(projection.data_type, NT_DATA_TYPE_TRADE_TICK);
        assert_eq!(projection.record_count, 2);

        let loaded = read_back_trade_ticks(dir.path(), &spec.nt_instrument_id).expect("read back");
        assert_eq!(
            loaded, built,
            "round-tripped ticks identical to built ticks"
        );
    }

    #[test]
    fn bars_project_and_read_back_round_trip() {
        let spec = trades_spec();
        let table = normalize_hip4_bars(BARS_JSONL, &spec).expect("normalize");
        let built = table.to_bars(&spec).expect("bars");

        let dir = tempfile::TempDir::new().expect("temp dir");
        let projection = project_hip4_bars_to_catalog(&table, &spec, dir.path()).expect("project");
        assert_eq!(projection.data_type, NT_DATA_TYPE_BAR);
        assert_eq!(projection.record_count, 2);

        let bar_type = table.bar_type_string(&spec).expect("bar type");
        let loaded = read_back_bars(dir.path(), &bar_type).expect("read back");
        assert_eq!(loaded, built, "round-tripped bars identical to built bars");
    }

    #[test]
    fn market_data_projection_refuses_dirty_root() {
        let spec = trades_spec();
        let table = normalize_hip4_trades(TRADES_JSONL, &spec).expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        fs::write(dir.path().join("stale.parquet"), b"stale").unwrap();
        let err = project_hip4_trades_to_catalog(&table, &spec, dir.path())
            .expect_err("dirty root must be refused");
        assert!(err.to_string().contains("not empty"), "{err}");
    }

    /// A staged bars object for one coin across several candle intervals (1m, 1h
    /// twice, 1w, 1M), shaped like the real `info.candleSnapshot` records.
    fn multi_interval_bars_object() -> String {
        [
            ("1m", 1_779_710_400_000i64, 1_779_710_459_999i64, "0.40"),
            ("1h", 1_779_706_800_000, 1_779_710_399_999, "0.41"),
            ("1h", 1_779_710_400_000, 1_779_713_999_999, "0.42"),
            ("1w", 1_779_408_000_000, 1_780_012_799_999, "0.43"),
            ("1M", 1_777_939_200_000, 1_780_531_199_999, "0.44"),
        ]
        .iter()
        .map(|(interval, open, close, o)| {
            format!(
                "{{\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
                 \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"interval\":\"{interval}\",\
                 \"open_time\":{open},\"close_time\":{close},\
                 \"open\":\"{o}\",\"high\":\"0.99\",\"low\":\"0.10\",\"close\":\"0.50\",\"volume\":\"1.0\"}}"
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    #[test]
    fn parse_hip4_bar_interval_handles_week_and_month() {
        // HL publishes 1w (week) and 1M (month) candles; NautilusTrader's
        // BarAggregation models Week and Month, so these map rather than failing.
        assert_eq!(
            parse_hip4_bar_interval("1w").expect("week"),
            (1, Hip4BarAggregation::Week)
        );
        assert_eq!(
            parse_hip4_bar_interval("1M").expect("month"),
            (1, Hip4BarAggregation::Month)
        );
        // The existing units are unchanged.
        assert_eq!(
            parse_hip4_bar_interval("15m").expect("minute"),
            (15, Hip4BarAggregation::Minute)
        );
    }

    #[test]
    fn normalize_bars_filters_by_spec_interval() {
        // A coin's candles at TWO intervals (1h and 5m); the spec selects 1h, so
        // only the 1h candle belongs to that bar stream. Without an interval
        // fence the 5m candle would leak into the 1h stream.
        let jsonl = "{\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
            \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"interval\":\"1h\",\
            \"open_time\":1779710400000,\"close_time\":1779713999999,\
            \"open\":\"0.40\",\"high\":\"0.45\",\"low\":\"0.39\",\"close\":\"0.42\",\"volume\":\"1.0\"}\n\
            {\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
            \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"interval\":\"5m\",\
            \"open_time\":1779710700000,\"close_time\":1779710999999,\
            \"open\":\"0.41\",\"high\":\"0.42\",\"low\":\"0.40\",\"close\":\"0.41\",\"volume\":\"1.0\"}";
        // trades_spec() is the 1-HOUR spec.
        let table = normalize_hip4_bars(jsonl, &trades_spec()).expect("normalize 1h");
        assert_eq!(
            table.rows.len(),
            1,
            "only the 1h candle is in the 1h stream"
        );
        assert_eq!(table.rows[0].open, "0.40");
    }

    #[test]
    fn append_hip4_bars_archive_emits_one_stream_per_interval() {
        // The real staged bars object carries every coin's candles at MANY
        // intervals. The converter must emit one NT bar stream per (coin,
        // interval) — not assume a single interval per coin (which lost ~99% of
        // the data) — and must handle week/month units end-to-end.
        let object = multi_interval_bars_object();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let summaries = append_hip4_bars_archive(&object, &hip4_canonical_naming(), &mut catalog)
            .expect("append multi-interval bars");

        // Four distinct intervals (1m, 1h, 1w, 1M) -> four bar streams, even
        // though 1h appears twice (those two candles collapse into one stream).
        assert_eq!(summaries.len(), 4, "one bar stream per (coin, interval)");
        let ids: std::collections::BTreeSet<&str> =
            summaries.iter().map(|s| s.nt_identifier.as_str()).collect();
        assert_eq!(ids.len(), 4, "four distinct bar-type identifiers");

        // Every stream round-trips through the NautilusTrader catalog (proving the
        // Week/Month aggregation is accepted by write_to_parquet + read-back). The
        // 1h stream carries its two candles; the rest carry one each.
        let mut total = 0usize;
        for summary in &summaries {
            let loaded =
                read_back_bars(dir.path(), &summary.nt_identifier).expect("read back bars");
            assert_eq!(loaded.len(), summary.record_count, "summary count matches");
            total += loaded.len();
        }
        assert_eq!(total, 5, "all five candles land across the four streams");
    }

    /// A staged `recentTrades` object for coin `#1010` with the given `(tid,
    /// time_ms)` prints, shaped like the real records (carrying `outcome`/`side`
    /// for the catalog id). All prints share px/sz so a shared tid is a true
    /// duplicate.
    fn trades_object(prints: &[(i64, i64)]) -> String {
        prints
            .iter()
            .map(|(tid, time_ms)| {
                format!(
                    "{{\"source_family\":\"info.recentTrades\",\"venue\":\"hyperliquid\",\
                     \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"tid\":{tid},\
                     \"time\":{time_ms},\"px\":\"0.42\",\"sz\":\"10.0\",\"trade_side\":\"A\"}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn hip4_trades_batch_dedups_overlapping_runs_one_disjoint_write_per_coin() {
        // HL `recentTrades` is a rolling buffer, so successive run= partitions
        // re-fetch overlapping prints. Object A carries tids 1,2,3; object B
        // carries 2,3,4 (2,3 shared, identical). Their time windows overlap, so
        // writing each as its own catalog file produces non-disjoint intervals
        // and NautilusTrader's write_to_parquet rejects the second (the RUN2
        // hyperliquid-hip4 trades failure).
        let object_a = trades_object(&[(1, 1000), (2, 2000), (3, 3000)]);
        let object_b = trades_object(&[(2, 2000), (3, 3000), (4, 4000)]);

        // Baseline: per-object writes collide on the overlapping second object.
        {
            let dir = tempfile::TempDir::new().expect("temp dir");
            let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
            append_hip4_trades_archive(&object_a, &hip4_canonical_naming(), &mut catalog)
                .expect("first object writes");
            let err = append_hip4_trades_archive(&object_b, &hip4_canonical_naming(), &mut catalog)
                .expect_err("overlapping second object must collide");
            // The NautilusTrader disjoint rejection is the error's cause; read the
            // full anyhow chain ({:#}), not just the top context.
            let chain = format!("{err:#}").to_lowercase();
            assert!(
                chain.contains("disjoint") || chain.contains("interval"),
                "expected a non-disjoint-intervals rejection, got: {err:#}"
            );
        }

        // Batch: dedup by tid across both objects, one disjoint write per coin.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let summaries =
            append_hip4_trades_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect("batch append");
        assert_eq!(summaries.len(), 1, "one trade stream for the one coin");
        // Union of tids {1,2,3,4} = 4 prints; the shared 2,3 are deduped, not doubled.
        assert_eq!(summaries[0].record_count, 4, "deduped union count");

        let loaded =
            read_back_trade_ticks(dir.path(), &summaries[0].nt_identifier).expect("read back");
        assert_eq!(loaded.len(), 4);
        let mut prev = 0u64;
        for tick in &loaded {
            let ts = tick.ts_event.as_u64();
            assert!(ts >= prev, "ascending ts");
            prev = ts;
        }
    }

    #[test]
    fn hip4_trades_batch_fails_loud_on_disagreeing_duplicate_tid() {
        // Same tid carrying different px/sz across overlapping objects is corrupt
        // (a tid is unique within a coin); fail loud rather than silently keeping
        // last-seen — the same fail-loud invariant the core converter preserves.
        let object_a = trades_object(&[(1, 1000), (2, 2000)]);
        let object_b = "{\"source_family\":\"info.recentTrades\",\"venue\":\"hyperliquid\",\
            \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"tid\":2,\"time\":2000,\
            \"px\":\"0.99\",\"sz\":\"10.0\",\"trade_side\":\"A\"}"
            .to_string();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let err = append_hip4_trades_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
            .expect_err("disagreeing duplicate tid must fail loud");
        assert!(err.to_string().contains("disagree"), "{err}");
    }

    /// A staged `candleSnapshot` object for coin `#1010` at one `interval`, with
    /// the given `(open_time_ms, open_price)` candles (close = open_time + an
    /// interval span, OHLC fixed). Carries `outcome`/`side` for the catalog id.
    fn bars_object(interval: &str, candles: &[(i64, &str)]) -> String {
        candles
            .iter()
            .map(|(open_time, o)| {
                format!(
                    "{{\"source_family\":\"info.candleSnapshot\",\"venue\":\"hyperliquid\",\
                     \"trade_coin\":\"#1010\",\"outcome\":101,\"side\":0,\"interval\":\"{interval}\",\
                     \"open_time\":{open_time},\"close_time\":{},\
                     \"open\":\"{o}\",\"high\":\"0.99\",\"low\":\"0.10\",\"close\":\"0.50\",\"volume\":\"1.0\"}}",
                    open_time + 3_599_999
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn hip4_bars_batch_dedups_overlapping_open_times_one_stream_per_interval() {
        // Two run= partitions re-fetch overlapping candles for the same (coin,
        // interval). Object A has open_times t1,t2,t3; object B has t2,t3,t4
        // (t2,t3 shared, identical). Per-object writes overlap in time and
        // NautilusTrader rejects the second; the batch dedups by open_time into
        // one ascending Bar stream.
        let (t1, t2, t3, t4) = (
            1_779_700_400_000i64,
            1_779_704_000_000,
            1_779_707_600_000,
            1_779_711_200_000,
        );
        let object_a = bars_object("1h", &[(t1, "0.41"), (t2, "0.42"), (t3, "0.43")]);
        let object_b = bars_object("1h", &[(t2, "0.42"), (t3, "0.43"), (t4, "0.44")]);

        // Baseline: per-object writes collide on the overlapping second object.
        {
            let dir = tempfile::TempDir::new().expect("temp dir");
            let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
            append_hip4_bars_archive(&object_a, &hip4_canonical_naming(), &mut catalog)
                .expect("first object writes");
            let err = append_hip4_bars_archive(&object_b, &hip4_canonical_naming(), &mut catalog)
                .expect_err("overlapping second object must collide");
            let chain = format!("{err:#}").to_lowercase();
            assert!(
                chain.contains("disjoint") || chain.contains("interval"),
                "expected a non-disjoint-intervals rejection, got: {err:#}"
            );
        }

        // Batch: dedup by open_time, one disjoint write per (coin, interval).
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let summaries =
            append_hip4_bars_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect("batch append");
        assert_eq!(
            summaries.len(),
            1,
            "one bar stream for the one (coin, interval)"
        );
        // Union of open_times {t1,t2,t3,t4} = 4 candles; shared t2,t3 deduped.
        assert_eq!(summaries[0].record_count, 4, "deduped union count");

        let loaded = read_back_bars(dir.path(), &summaries[0].nt_identifier).expect("read back");
        assert_eq!(loaded.len(), 4);
        let mut prev = 0u64;
        for bar in &loaded {
            let ts = bar.ts_event.as_u64();
            assert!(ts >= prev, "ascending ts");
            prev = ts;
        }
    }

    #[test]
    fn hip4_bars_batch_keeps_each_interval_its_own_stream() {
        // One coin staged at two intervals across two objects: the batch must keep
        // them as two distinct bar streams (not merge intervals).
        let object_a = bars_object("1h", &[(1_779_700_400_000, "0.41")]);
        let object_b = bars_object("1m", &[(1_779_700_400_000, "0.42")]);
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let summaries =
            append_hip4_bars_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect("batch append");
        assert_eq!(summaries.len(), 2, "one stream per (coin, interval)");
        let ids: std::collections::BTreeSet<&str> =
            summaries.iter().map(|s| s.nt_identifier.as_str()).collect();
        assert_eq!(ids.len(), 2, "two distinct bar-type identifiers");
    }

    #[test]
    fn hip4_bars_batch_keeps_latest_revision_for_overlapping_open_time() {
        // HL candleSnapshot re-fetches rolling candles. A later run can revise a
        // still-open candle for the same (coin, interval, open_time); the batch
        // keeps the later object's candle instead of treating it as corruption.
        let object_a = bars_object("1h", &[(1_779_700_400_000, "0.41")]);
        let object_b = bars_object("1h", &[(1_779_700_400_000, "0.88")]);
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let summaries =
            append_hip4_bars_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect("batch append");

        assert_eq!(summaries.len(), 1, "one stream for the revised candle");
        assert_eq!(summaries[0].record_count, 1, "one deduplicated candle");
        let loaded = read_back_bars(dir.path(), &summaries[0].nt_identifier).expect("read back");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].open.to_string(), "0.88");
    }

    /// A staged `order_book_snapshots_fixed_depth` object for outcome `7` with the
    /// given `snapshot_time`s (one bid + one ask level each, fixed). Shared times
    /// across objects are identical photos (dedup-safe).
    fn snapshot_object(times: &[i64]) -> String {
        times
            .iter()
            .map(|t| {
                format!(
                    "{{\"source_family\":\"info.l2Book\",\"venue\":\"hyperliquid\",\
                     \"quote_token\":\"USDC\",\"trade_coin\":\"#1\",\"outcome\":7,\
                     \"snapshot_time\":{t},\"raw_levels\":[[{{\"px\":\"0.43\",\"sz\":\"22.0\",\"n\":1}}],\
                     [{{\"px\":\"0.50\",\"sz\":\"978.0\",\"n\":2}}]],\
                     \"bid_level_count\":1,\"ask_level_count\":1}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn hip4_snapshots_batch_dedups_shared_snapshot_time_per_outcome() {
        // Two objects for the same outcome with a shared snapshot_time (t2). Each
        // photo expands to deltas at its snapshot time, so per-object writes
        // overlap in time and NautilusTrader rejects the second; the batch dedups
        // by (outcome, snapshot_time) into one ascending delta stream.
        let (t1, t2, t3) = (1_780_331_396_000i64, 1_780_331_397_000, 1_780_331_398_000);
        let object_a = snapshot_object(&[t1, t2]);
        let object_b = snapshot_object(&[t2, t3]);

        // Baseline: per-object writes collide on the overlapping snapshot range.
        {
            let dir = tempfile::TempDir::new().expect("temp dir");
            let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
            append_hip4_snapshots_archive(&object_a, &hip4_canonical_naming(), &mut catalog)
                .expect("first object writes");
            let err =
                append_hip4_snapshots_archive(&object_b, &hip4_canonical_naming(), &mut catalog)
                    .expect_err("overlapping second object must collide");
            let chain = format!("{err:#}").to_lowercase();
            assert!(
                chain.contains("disjoint") || chain.contains("interval"),
                "expected a non-disjoint-intervals rejection, got: {err:#}"
            );
        }

        // Batch: dedup by (outcome, snapshot_time), one disjoint write per outcome.
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let summaries =
            append_hip4_snapshots_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect("batch append");
        assert_eq!(summaries.len(), 1, "one outcome instrument");

        // Three deduped photos {t1,t2,t3}; each non-empty photo -> Clear + 1 bid
        // Add + 1 ask Add = 3 deltas. Union dedups t2 (3 photos, not 4 -> 9 deltas,
        // not 12).
        assert_eq!(summaries[0].record_count, 9, "deduped 3 photos x 3 deltas");
        let loaded = read_back_order_book_deltas(dir.path(), &summaries[0].nt_identifier)
            .expect("read back");
        assert_eq!(loaded.len(), 9);
        let times: std::collections::BTreeSet<u64> =
            loaded.iter().map(|d| d.ts_event.as_u64()).collect();
        assert_eq!(times.len(), 3, "three distinct snapshot times (t2 deduped)");
        let mut prev = 0u64;
        for delta in &loaded {
            let ts = delta.ts_event.as_u64();
            assert!(ts >= prev, "ascending ts");
            prev = ts;
        }
    }

    #[test]
    fn hip4_snapshots_batch_fails_loud_on_disagreeing_snapshot_time() {
        // Same (outcome, snapshot_time) with different levels across objects is
        // corrupt; fail loud rather than silently keeping last-seen.
        let object_a = snapshot_object(&[1_780_331_397_000]);
        let object_b = "{\"source_family\":\"info.l2Book\",\"venue\":\"hyperliquid\",\
            \"quote_token\":\"USDC\",\"trade_coin\":\"#1\",\"outcome\":7,\
            \"snapshot_time\":1780331397000,\"raw_levels\":[[{\"px\":\"0.88\",\"sz\":\"5.0\",\"n\":1}],\
            [{\"px\":\"0.91\",\"sz\":\"7.0\",\"n\":1}]],\"bid_level_count\":1,\"ask_level_count\":1}"
            .to_string();
        let dir = tempfile::TempDir::new().expect("temp dir");
        let mut catalog = ParquetDataCatalog::new(dir.path(), None, None, None, None);
        let batch = vec![
            ("a.jsonl".to_string(), object_a.into_bytes()),
            ("b.jsonl".to_string(), object_b.into_bytes()),
        ];
        let err =
            append_hip4_snapshots_archive_batch(&batch, &hip4_canonical_naming(), &mut catalog)
                .expect_err("disagreeing duplicate snapshot_time must fail loud");
        assert!(err.to_string().contains("disagree"), "{err}");
    }
}
