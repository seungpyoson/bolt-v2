//! Gate 2 (L2) — canonical normalized `order_book` event table.
//!
//! Normalizes an accepted Polymarket CLOB event stream into an intermediate
//! canonical representation that the NautilusTrader catalog projection consumes.
//! It is the L2 sibling of [`super::canonical_trades`]: where that module
//! produces native trade prints, this module produces the full order-book event
//! sequence (full snapshots, single-level deltas, and trade prints) for one
//! outcome token.
//!
//! Polymarket's CLOB stream interleaves three event types per `asset_id`:
//!
//! - `book` — an `asset_id`-scoped full depth snapshot. `bids`/`asks` are JSON
//!   arrays of `[price, size]` string pairs.
//! - `price_change` — a single price-level update: `price`, `size`, `side`
//!   (`BUY` => bid side, `SELL` => ask side). `size == 0` means the level was
//!   removed.
//! - `last_trade_price` — a trade print: `price`, `size`, `side`.
//!
//! Each raw row maps to exactly one [`CanonicalBookEvent`] variant
//! ([`CanonicalBookEvent::Snapshot`], [`CanonicalBookEvent::LevelChange`],
//! [`CanonicalBookEvent::Trade`]). This module deliberately does NOT emit
//! NautilusTrader types — the NT `OrderBookDelta`/`TradeTick` projection is a
//! later gate. It produces typed canonical rows with full provenance,
//! decimals-preserved-as-strings, and a monotonic-nondecreasing event time, in
//! the same style as [`super::canonical_trades`].
//!
//! Unlike the `trades` table, an L2 book table is legitimately labelled
//! [`super::source_proof::SourceProofFidelityClass::L2Replay`]; the L2_REPLAY
//! ban lives on the `trades` table's own validator, never globally.
//!
//! Input is only ever an [`AcceptedDataset`] from gate 1 — raw staged data never
//! reaches this module without first passing source-proof acceptance.

use std::{collections::HashSet, fs::File, path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use arrow::{
    array::{
        Array, BooleanArray, Decimal128Array, Int64Array, StringArray, TimestampMicrosecondArray,
        TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
    },
    datatypes::{DataType, Field, TimeUnit},
    error::ArrowError,
    record_batch::RecordBatch,
};
use nautilus_model::{
    data::{OrderBookDelta, TradeTick},
    instruments::InstrumentAny,
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use parquet::arrow::{
    ProjectionMask, RowNumber,
    arrow_reader::{
        ArrowPredicateFn, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowFilter,
    },
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    catalog_projection::{
        BinaryOptionInstrumentSpec, build_binary_option, canonical_book_rows_to_trade_ticks,
        canonical_rows_to_order_book_deltas,
    },
    source_proof::{AcceptedDataset, IngestManifestObjectRecord, SourceProofFidelityClass},
};

/// Contracted semantic schema version for canonical L2 order-book event rows.
pub const NORMALIZED_BOOK_SCHEMA_VERSION: &str = "order_book.v1";

/// Stable identity of this normalization transform, hashed into `transform_hash`.
pub const BOOK_TRANSFORM_IDENTITY: &str = "polymarket-clob-event-stream-to-canonical-order-book.v1";

/// Raw Polymarket CLOB `event_type` value for a full depth snapshot.
pub const EVENT_TYPE_BOOK: &str = "book";
/// Raw Polymarket CLOB `event_type` value for a single-level update.
pub const EVENT_TYPE_PRICE_CHANGE: &str = "price_change";
/// Raw Polymarket CLOB `event_type` value for a trade print.
pub const EVENT_TYPE_LAST_TRADE_PRICE: &str = "last_trade_price";
/// Raw Polymarket CLOB `event_type` value for a tick-size change.
pub const EVENT_TYPE_TICK_SIZE_CHANGE: &str = "tick_size_change";

/// Which side of the book a CLOB update or trade applies to.
///
/// Polymarket encodes bid-side liquidity with `BUY` and ask-side liquidity with
/// `SELL` on both `price_change` and `last_trade_price` rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BookSide {
    /// `BUY` — bid side.
    Buy,
    /// `SELL` — ask side.
    Sell,
}

impl BookSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }

    fn parse_clob(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_uppercase().as_str() {
            "BUY" => Ok(Self::Buy),
            "SELL" => Ok(Self::Sell),
            other => bail!("unknown CLOB side token: {other:?}"),
        }
    }
}

/// One price level decoded from a `book` snapshot's `bids`/`asks` JSON array,
/// with the exact source price/size strings preserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookLevel {
    /// Exact source price string (for example `0.01`).
    pub price: String,
    /// Exact source size string (for example `14926.03`).
    pub size: String,
}

/// A decoded full depth snapshot for one `asset_id` at one event instant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookSnapshot {
    /// Bid-side levels, in source order (best-first is not assumed).
    pub bids: Vec<BookLevel>,
    /// Ask-side levels, in source order.
    pub asks: Vec<BookLevel>,
}

/// A single price-level update from a `price_change` row.
///
/// `size` of decimal-zero marks the level as removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LevelChange {
    pub side: BookSide,
    /// Exact source price string of the changed level.
    pub price: String,
    /// Exact source size string after the change; decimal-zero means removed.
    pub size: String,
    /// True when `size` parses to zero — the level was removed.
    pub is_removal: bool,
}

/// A trade print from a `last_trade_price` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookTrade {
    pub side: BookSide,
    pub aggressor_side: Option<BookSide>,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
    /// On-chain transaction hash for the trade, when present.
    pub transaction_hash: String,
}

/// The canonical event payload of one CLOB row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CanonicalBookEvent {
    /// `book` — full depth snapshot.
    Snapshot(BookSnapshot),
    /// `price_change` — single-level update.
    LevelChange(LevelChange),
    /// `last_trade_price` — trade print.
    Trade(BookTrade),
}

impl CanonicalBookEvent {
    /// The raw CLOB `event_type` token this variant was decoded from.
    #[must_use]
    pub const fn event_type(&self) -> &'static str {
        match self {
            Self::Snapshot(_) => EVENT_TYPE_BOOK,
            Self::LevelChange(_) => EVENT_TYPE_PRICE_CHANGE,
            Self::Trade(_) => EVENT_TYPE_LAST_TRADE_PRICE,
        }
    }
}

/// One raw decoded Polymarket CLOB event row, as read from an accepted object.
///
/// The caller decodes the storage layer (Parquet columns) into this struct so
/// the normalizer stays storage-agnostic and exercises identical logic in tests
/// and in the runner. `timestamp_received` is the monotonic ingest/capture clock
/// converted to Unix nanoseconds (UTC) and is the replay ordering clock.
/// `event_time` is the source `timestamp` column converted to Unix nanoseconds
/// (UTC). `source_row_index` preserves physical object order for stable ties.
/// `bids`/`asks`/`price`/`size`/`side`/`transaction_hash` carry the exact source
/// strings (empty when the column was null for the row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawClobEventRow {
    pub asset_id: String,
    pub event_type: String,
    /// Ingest/capture instant in Unix nanoseconds (UTC).
    pub timestamp_received: i64,
    /// Event instant in Unix nanoseconds (UTC).
    pub event_time: i64,
    /// Physical row index within the decoded object.
    pub source_row_index: u64,
    /// Raw `bids` JSON string (`book` rows only; empty otherwise).
    pub bids: String,
    /// Raw `asks` JSON string (`book` rows only; empty otherwise).
    pub asks: String,
    /// Raw `price` string (`price_change` / `last_trade_price`; empty for `book`).
    pub price: String,
    /// Raw `size` string (`price_change` / `last_trade_price`; empty for `book`).
    pub size: String,
    /// Raw `side` string (`price_change` / `last_trade_price`; empty for `book`).
    pub side: String,
    /// Raw `transaction_hash` (`last_trade_price`; empty otherwise).
    pub transaction_hash: String,
}

/// One normalized canonical L2 event row with full provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBookRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    /// Outcome token id (the instrument), unique within `(venue, product_family)`.
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    /// Replay ordering timestamp in Unix nanoseconds (UTC), sourced from the
    /// monotonic `timestamp_received` capture clock.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Monotonic per-instrument sequence assigned at normalization (0-based).
    pub source_sequence: u64,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// The decoded canonical event.
    pub event: CanonicalBookEvent,
}

/// A validated canonical L2 order-book event table for one accepted object,
/// scoped to a single `asset_id` (the instrument).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBookTable {
    pub schema_version: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    /// Archive date partition `YYYY-MM-DD`.
    pub dt: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalBookRow>,
}

/// Lowercase SHA-256 hex of the L2 transform identity.
#[must_use]
pub fn book_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(BOOK_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// Decode a `bids`/`asks` JSON array of `[price, size]` string pairs into levels.
///
/// Preserves the exact source price/size strings and the source level order.
///
/// # Errors
///
/// Returns an error if the JSON is malformed, a pair is not a 2-element array,
/// or an element is not a parseable price/size string.
fn decode_levels(raw: &str, field: &str) -> Result<Vec<BookLevel>> {
    let parsed: serde_json::Value =
        serde_json::from_str(raw).with_context(|| format!("{field}: invalid JSON {raw:?}"))?;
    let array = parsed
        .as_array()
        .with_context(|| format!("{field}: expected a JSON array, got {raw:?}"))?;
    let mut levels = Vec::with_capacity(array.len());
    for (index, pair) in array.iter().enumerate() {
        let pair = pair
            .as_array()
            .with_context(|| format!("{field}[{index}]: expected [price, size] pair"))?;
        ensure!(
            pair.len() == 2,
            "{field}[{index}]: expected 2 elements, got {}",
            pair.len()
        );
        let price = pair[0]
            .as_str()
            .with_context(|| format!("{field}[{index}]: price is not a string"))?;
        let size = pair[1]
            .as_str()
            .with_context(|| format!("{field}[{index}]: size is not a string"))?;
        let price_dec: Decimal = price
            .parse()
            .with_context(|| format!("{field}[{index}]: invalid price {price:?}"))?;
        let size_dec: Decimal = size
            .parse()
            .with_context(|| format!("{field}[{index}]: invalid size {size:?}"))?;
        ensure!(
            price_dec > Decimal::ZERO,
            "{field}[{index}]: non-positive price {price:?}"
        );
        ensure!(
            size_dec >= Decimal::ZERO,
            "{field}[{index}]: negative size {size:?}"
        );
        levels.push(BookLevel {
            price: price.to_string(),
            size: size.to_string(),
        });
    }
    Ok(levels)
}

/// Decode a `price`/`size` decimal-string pair, enforcing a positive price and a
/// non-negative size. Returns the parsed size so callers can detect removals.
fn parse_price_size(price: &str, size: &str, ctx: &str) -> Result<(String, String, Decimal)> {
    ensure!(!price.trim().is_empty(), "{ctx}: empty price");
    ensure!(!size.trim().is_empty(), "{ctx}: empty size");
    let price_dec: Decimal = price
        .trim()
        .parse()
        .with_context(|| format!("{ctx}: invalid price {price:?}"))?;
    let size_dec: Decimal = size
        .trim()
        .parse()
        .with_context(|| format!("{ctx}: invalid size {size:?}"))?;
    ensure!(
        price_dec > Decimal::ZERO,
        "{ctx}: non-positive price {price:?}"
    );
    ensure!(size_dec >= Decimal::ZERO, "{ctx}: negative size {size:?}");
    Ok((price.trim().to_string(), size.trim().to_string(), size_dec))
}

/// Decode a single raw CLOB row into its canonical event payload.
///
/// # Errors
///
/// Returns `Ok(None)` for accepted non-book-mutating rows that produce no NT
/// record. Returns an error if the `event_type` is unknown, a required field for
/// the event type is missing/empty, or a numeric field fails to parse.
fn decode_event(row: &RawClobEventRow) -> Result<Option<CanonicalBookEvent>> {
    match row.event_type.as_str() {
        EVENT_TYPE_BOOK => {
            ensure!(!row.bids.trim().is_empty(), "book row: empty bids");
            ensure!(!row.asks.trim().is_empty(), "book row: empty asks");
            let bids = decode_levels(&row.bids, "bids")?;
            let asks = decode_levels(&row.asks, "asks")?;
            // A both-sides-empty book ("[]"/"[]") is a genuine empty-book state: a
            // market that opened with no resting liquidity. It is the first event
            // for the asset and projects to NautilusTrader's empty-book Clear
            // delta. The string guards above still reject null/empty `bids`/`asks`
            // (malformed); the literal empty JSON arrays pass them.
            Ok(Some(CanonicalBookEvent::Snapshot(BookSnapshot {
                bids,
                asks,
            })))
        }
        EVENT_TYPE_PRICE_CHANGE => {
            let side = BookSide::parse_clob(&row.side)?;
            let (price, size, size_dec) = parse_price_size(&row.price, &row.size, "price_change")?;
            Ok(Some(CanonicalBookEvent::LevelChange(LevelChange {
                side,
                price,
                size,
                is_removal: size_dec == Decimal::ZERO,
            })))
        }
        EVENT_TYPE_LAST_TRADE_PRICE => {
            let aggressor_side = if row.side.trim().is_empty() {
                None
            } else {
                Some(BookSide::parse_clob(&row.side)?)
            };
            let side = aggressor_side.unwrap_or(BookSide::Buy);
            let (price, size, size_dec) =
                parse_price_size(&row.price, &row.size, "last_trade_price")?;
            ensure!(
                size_dec > Decimal::ZERO,
                "last_trade_price: non-positive size {:?}",
                row.size
            );
            Ok(Some(CanonicalBookEvent::Trade(BookTrade {
                side,
                aggressor_side,
                price,
                size,
                transaction_hash: row.transaction_hash.trim().to_string(),
            })))
        }
        EVENT_TYPE_TICK_SIZE_CHANGE => Ok(None),
        other => bail!("unknown CLOB event_type: {other:?}"),
    }
}

/// Normalize an accepted Polymarket CLOB event stream into the canonical L2
/// order-book table for a single `asset_id`.
///
/// `rows` are the raw decoded CLOB rows for the accepted object; the normalizer
/// filters to `asset_id`, decodes each row, assigns a monotonic per-instrument
/// sequence, and validates the result. `capture_time_nanos` is the ingest
/// capture timestamp recorded for the run. `ingest_run_id` is the stable run
/// identifier recorded for lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if `ingest_run_id` or `asset_id` is empty, a row is
/// malformed, or the assembled table fails contract validation.
pub fn normalize_polymarket_clob_book(
    accepted: &AcceptedDataset,
    asset_id: &str,
    rows: &[RawClobEventRow],
    _capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<CanonicalBookTable> {
    let mut asset_rows: Vec<&RawClobEventRow> =
        rows.iter().filter(|r| r.asset_id == asset_id).collect();
    asset_rows.sort_by_key(|row| (row.timestamp_received, row.source_row_index));
    normalize_sorted_polymarket_clob_book_rows(accepted, asset_id, &asset_rows, ingest_run_id)
}

fn normalize_sorted_polymarket_clob_book_rows(
    accepted: &AcceptedDataset,
    asset_id: &str,
    asset_rows: &[&RawClobEventRow],
    ingest_run_id: &str,
) -> Result<CanonicalBookTable> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(!asset_id.trim().is_empty(), "asset_id must not be empty");

    let canonical_instrument_key = format!(
        "{}/{}/{}",
        accepted.venue, accepted.product_family, asset_id
    );
    let transform_hash = book_transform_hash();

    let mut canonical_rows = Vec::new();
    let mut sequence: u64 = 0;
    for raw in asset_rows {
        let event = decode_event(raw)
            .with_context(|| format!("sequence {sequence}: failed to decode CLOB event"))?;
        let Some(event) = event else {
            continue;
        };
        canonical_rows.push(CanonicalBookRow {
            schema_version: NORMALIZED_BOOK_SCHEMA_VERSION.to_string(),
            ingest_run_id: ingest_run_id.to_string(),
            source_binding: accepted.source_binding.clone(),
            venue: accepted.venue.clone(),
            product_family: accepted.product_family.clone(),
            product_category: accepted.product_category.clone(),
            instrument_id: asset_id.to_string(),
            canonical_instrument_key: canonical_instrument_key.clone(),
            event_time: raw.timestamp_received,
            capture_time: raw.timestamp_received,
            source_sequence: sequence,
            raw_payload_id: accepted.object.sha256.clone(),
            source_proof_id: accepted.source_proof_id.clone(),
            payload_hash: accepted.object.sha256.clone(),
            transform_hash: transform_hash.clone(),
            event,
        });
        sequence = sequence.checked_add(1).context("event sequence overflow")?;
    }

    let table = CanonicalBookTable {
        schema_version: NORMALIZED_BOOK_SCHEMA_VERSION.to_string(),
        source_binding: accepted.source_binding.clone(),
        venue: accepted.venue.clone(),
        product_family: accepted.product_family.clone(),
        product_category: accepted.product_category.clone(),
        instrument_id: asset_id.to_string(),
        dt: accepted.object.archive_date.clone(),
        source_proof_id: accepted.source_proof_id.clone(),
        source_proof_version: accepted.source_proof_version,
        fidelity_class: accepted.fidelity_class,
        forbidden_claims: accepted.forbidden_claims.clone(),
        transform_hash,
        payload_hash: accepted.object.sha256.clone(),
        rows: canonical_rows,
    };
    table.validate()?;
    Ok(table)
}

impl CanonicalBookTable {
    /// Count of `Snapshot` rows.
    #[must_use]
    pub fn snapshot_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r.event, CanonicalBookEvent::Snapshot(_)))
            .count()
    }

    /// Count of `LevelChange` rows.
    #[must_use]
    pub fn level_change_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r.event, CanonicalBookEvent::LevelChange(_)))
            .count()
    }

    /// Count of `Trade` rows.
    #[must_use]
    pub fn trade_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r.event, CanonicalBookEvent::Trade(_)))
            .count()
    }

    /// Validate required fields, timestamps, instrument scope, dense sequence,
    /// and schema version.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_BOOK_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(!self.rows.is_empty(), "canonical book table is empty");
        for field in [
            &self.source_binding,
            &self.venue,
            &self.product_family,
            &self.product_category,
            &self.instrument_id,
            &self.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            !self.forbidden_claims.is_empty(),
            "L2 replay table must carry explicit forbidden claims"
        );

        let mut previous_event_time = i64::MIN;
        let mut expected_sequence: u64 = 0;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_BOOK_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            ensure!(
                row.source_sequence == expected_sequence,
                "row {index}: source_sequence {} is not the expected {}",
                row.source_sequence,
                expected_sequence
            );
            expected_sequence = expected_sequence
                .checked_add(1)
                .context("sequence overflow during validation")?;
            ensure!(
                row.instrument_id == self.instrument_id,
                "row {index}: instrument_id does not match table scope"
            );
            for field in [
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            validate_event(&row.event, index)?;
        }
        Ok(())
    }
}

/// Validate a single canonical event's decoded payload.
fn validate_event(event: &CanonicalBookEvent, index: usize) -> Result<()> {
    match event {
        CanonicalBookEvent::Snapshot(snapshot) => {
            // A zero-level snapshot is a valid empty-book state (market open with
            // no resting liquidity); only the per-level price/size strings are
            // validated. The loop is vacuously satisfied for an empty book.
            for level in snapshot.bids.iter().chain(snapshot.asks.iter()) {
                ensure!(
                    !level.price.trim().is_empty() && !level.size.trim().is_empty(),
                    "row {index}: snapshot level has empty price/size"
                );
            }
        }
        CanonicalBookEvent::LevelChange(change) => {
            ensure!(
                !change.price.trim().is_empty() && !change.size.trim().is_empty(),
                "row {index}: level change has empty price/size"
            );
        }
        CanonicalBookEvent::Trade(trade) => {
            ensure!(
                !trade.price.trim().is_empty() && !trade.size.trim().is_empty(),
                "row {index}: trade has empty price/size"
            );
        }
    }
    Ok(())
}

// ===========================================================================
// Polymarket CLOB bulk-append path (data-derived precision, no clean-root guard)
// ===========================================================================
//
// The runner converts every staged Polymarket CLOB Parquet object into the one
// NautilusTrader catalog. Two staged families share the CLOB row schema and this
// code path:
//
// * `polymarket_book/`   — the full CLOB event stream (`book` snapshots,
//   `price_change` deltas, and `last_trade_price` prints). Projects to
//   [`OrderBookDelta`] (snapshot expansion + level deltas) AND [`TradeTick`]
//   (the prints), via [`append_polymarket_book_archive`].
// * `polymarket_trades/` — the trade-print stream only (`last_trade_price`
//   rows). Projects to [`TradeTick`] only, via
//   [`append_polymarket_trades_archive`].
//
// Both decode the same Parquet column layout into [`RawClobEventRow`]s, normalize
// per `asset_id` with the existing [`normalize_polymarket_clob_book`], derive the
// instrument precision from the object's own rows (Polymarket stages no
// instrument universe), and append into a passed-in [`ParquetDataCatalog`]
// WITHOUT the dirty-root guard — many objects flow into one shared (possibly-S3)
// catalog, relying on NautilusTrader's own per-instrument/per-time-range file
// naming. The hermetic single-object proof path stays in
// [`super::catalog_projection::project_canonical_book_to_catalog`].

/// NautilusTrader venue code for Polymarket, appended to a venue-native outcome
/// token id to form the catalog instrument id (`<asset_id>.POLYMARKET`). The
/// data-derived bulk path needs this because Polymarket stages no instrument
/// universe to carry it. This is the per-venue NT id format constant.
pub const POLYMARKET_VENUE: &str = "POLYMARKET";

/// Settlement/quote currency for every Polymarket binary-outcome market. A venue
/// fact (Polymarket settles outcome shares in USDC), not an instrument-specific
/// claim, so it is the same for every object and carries no false origin.
pub const POLYMARKET_QUOTE_CURRENCY: &str = "USDC";

/// `AssetClass` token for Polymarket prediction-market outcome shares. A
/// structural classification of the venue's product, not a per-object claim.
pub const POLYMARKET_ASSET_CLASS: &str = "ALTERNATIVE";

/// Venue token recorded in the canonical rows' provenance for this conversion.
pub const POLYMARKET_VENUE_TOKEN: &str = "polymarket";

/// Product-family token recorded in the canonical rows' provenance, matching the
/// `polymarket-parquet-archive-index` source binding's `product_family`.
pub const POLYMARKET_PRODUCT_FAMILY: &str = "prediction_market_outcome";

/// Product-category token recorded in the canonical rows' provenance.
pub const POLYMARKET_PRODUCT_CATEGORY: &str = "binary-outcome";

/// Source-binding key recorded in the canonical rows' provenance, matching the
/// `polymarket-parquet-archive-index` source binding.
pub const POLYMARKET_SOURCE_BINDING: &str = "polymarket-parquet-archive-index";

/// Stable label of THIS bulk conversion, recorded in the canonical rows'
/// `source_proof_id` provenance slot. It honestly names the converter run rather
/// than asserting a passed source-proof acceptance (the bulk path stages no
/// accepted `SourceProofReport`).
pub const POLYMARKET_INGEST_RUN_ID: &str = "polymarket-clob-bulk-convert.v1";

/// Forbidden-claim attached to every bulk-converted Polymarket book/trades table,
/// matching the L2-replay claim limit the table validator requires.
const POLYMARKET_FORBIDDEN_CLAIM: &str = "No fill claims beyond replayed top-of-book liquidity.";

/// The CLOB Parquet column layout this converter decodes. Recorded as the
/// accepted object's `schema_columns` so the provenance describes the real
/// object shape (the `transaction_hash` column is present in trade-print rows).
const POLYMARKET_CLOB_COLUMNS: [&str; 10] = [
    "timestamp_received",
    "timestamp",
    "event_type",
    "asset_id",
    "bids",
    "asks",
    "price",
    "size",
    "side",
    "transaction_hash",
];

/// The decimal-string increment whose fractional length is exactly `precision`
/// (`0 -> "1"`, `1 -> "0.1"`, `2 -> "0.01"`). Lets a data-derived precision be
/// expressed as the increment string [`BinaryOptionInstrumentSpec`] consumes.
#[must_use]
fn increment_for(precision: u8) -> String {
    match precision {
        0 => "1".to_string(),
        n => format!("0.{}1", "0".repeat(usize::from(n) - 1)),
    }
}

/// Maximum decimal places of a decimal string (`"0.51"` -> 2, `"14926.03"` -> 2,
/// `"5"` -> 0).
fn decimal_places(value: &str) -> Result<u8> {
    let decimal: Decimal = value
        .parse()
        .with_context(|| format!("decimal {value:?}"))?;
    u8::try_from(decimal.scale()).context("decimal scale exceeds u8")
}

/// One instrument's write summary produced by the bulk-append path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketAppendSummary {
    pub nt_instrument_id: String,
    /// Count of written `OrderBookDelta` records (0 for the trades-only family).
    pub delta_count: usize,
    /// Count of written `TradeTick` records (the trade prints).
    pub trade_count: usize,
    pub price_precision: u8,
    pub size_precision: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolymarketAppendProjection {
    BookAndTrades,
    TradesOnly,
}

const POLYMARKET_SOURCE_ROW_NUMBER_COLUMN: &str = "__bolt_source_row_number";

/// Extract the archive partition date (`YYYY-MM-DD`) from a staged object key's
/// `dt=<date>/` segment.
///
/// # Errors
///
/// Returns an error if the key carries no `dt=YYYY-MM-DD` segment.
fn archive_date_from_key(object_key: &str) -> Result<String> {
    for segment in object_key.split('/') {
        if let Some(date) = segment.strip_prefix("dt=") {
            let date = date.trim();
            ensure!(
                date.len() == 10 && date.as_bytes()[4] == b'-' && date.as_bytes()[7] == b'-',
                "object key dt segment {date:?} is not YYYY-MM-DD"
            );
            return Ok(date.to_string());
        }
    }
    bail!("object key {object_key:?} carries no dt=YYYY-MM-DD segment")
}

/// Build an honest [`AcceptedDataset`] describing THIS bulk conversion of one
/// staged Polymarket CLOB object.
///
/// The bulk path stages no accepted `SourceProofReport`, so this constructs the
/// provenance the existing [`normalize_polymarket_clob_book`] requires directly
/// from values that honestly describe the conversion: the `payload_hash` /
/// `raw_payload_id` is the SHA-256 of the real on-disk object bytes, the
/// `archive_date` is parsed from the object key's `dt=` segment, and the venue /
/// product / source-binding / fidelity / forbidden-claims are the fixed venue
/// facts of the `polymarket-parquet-archive-index` source binding. No field
/// asserts a passed source-proof acceptance or a false origin.
///
/// # Errors
///
/// Returns an error if the object cannot be read or the key carries no `dt=`
/// segment.
fn polymarket_accepted_dataset(object_path: &Path, object_key: &str) -> Result<AcceptedDataset> {
    let bytes = std::fs::read(object_path)
        .with_context(|| format!("read Polymarket object {}", object_path.display()))?;
    let payload_hash = hex::encode(Sha256::digest(&bytes));
    let archive_date = archive_date_from_key(object_key)?;
    Ok(AcceptedDataset {
        source_proof_id: POLYMARKET_INGEST_RUN_ID.to_string(),
        source_proof_version: 1,
        source_binding: POLYMARKET_SOURCE_BINDING.to_string(),
        venue: POLYMARKET_VENUE_TOKEN.to_string(),
        product_family: POLYMARKET_PRODUCT_FAMILY.to_string(),
        product_category: POLYMARKET_PRODUCT_CATEGORY.to_string(),
        instrument_universe_id: POLYMARKET_INGEST_RUN_ID.to_string(),
        fidelity_class: SourceProofFidelityClass::L2Replay,
        forbidden_claims: vec![POLYMARKET_FORBIDDEN_CLAIM.to_string()],
        object: IngestManifestObjectRecord {
            s3_uri: object_key.to_string(),
            source_url: object_key.to_string(),
            sha256: payload_hash,
            bytes: bytes.len() as u64,
            archive_date,
            schema_columns: POLYMARKET_CLOB_COLUMNS
                .iter()
                .map(ToString::to_string)
                .collect(),
        },
    })
}

/// Decode a staged Polymarket CLOB Parquet object into [`RawClobEventRow`]s.
///
/// Mirrors the column decode the converter's proof tests exercise: `timestamp` is
/// UTC microseconds (scaled to Unix nanoseconds), the string columns map null to
/// the empty string, and `price`/`size` `Decimal128` cells render at their
/// column scale (empty for `book` rows). This is the single shared decode the
/// runner uses, so the bulk path and the proofs cannot drift apart.
///
/// # Errors
///
/// Returns an error if the file cannot be opened/read as Parquet, a required
/// column is missing, or a column has an unexpected Arrow type.
pub fn decode_polymarket_clob_parquet(object_path: &Path) -> Result<Vec<RawClobEventRow>> {
    decode_polymarket_clob_parquet_filtered(object_path, None)
}

/// Decode only one `asset_id` from a staged Polymarket CLOB Parquet object.
///
/// This is intentionally a separate streaming pass over the Parquet object so
/// the bulk append path can hold only one outcome token's rows in memory while
/// writing that instrument's catalog data.
///
/// # Errors
///
/// Returns an error if the file cannot be opened/read as Parquet, a required
/// column is missing, or a column has an unexpected Arrow type.
pub fn decode_polymarket_clob_parquet_for_asset(
    object_path: &Path,
    asset_id: &str,
) -> Result<Vec<RawClobEventRow>> {
    let allowed_assets = HashSet::from([asset_id.to_string()]);
    decode_polymarket_clob_parquet_filtered(object_path, Some(&allowed_assets))
}

/// Decode only selected `asset_id`s from a staged Polymarket CLOB Parquet
/// object, preserving raw source row indexes for the returned rows.
///
/// # Errors
///
/// Returns an error if the file cannot be opened/read as Parquet, a required
/// column is missing, or a column has an unexpected Arrow type.
pub fn decode_polymarket_clob_parquet_for_assets(
    object_path: &Path,
    allowed_assets: &HashSet<String>,
) -> Result<Vec<RawClobEventRow>> {
    decode_polymarket_clob_parquet_filtered(object_path, Some(allowed_assets))
}

fn decode_polymarket_clob_parquet_filtered(
    object_path: &Path,
    allowed_assets: Option<&HashSet<String>>,
) -> Result<Vec<RawClobEventRow>> {
    let file = File::open(object_path)
        .with_context(|| format!("open Polymarket parquet {}", object_path.display()))?;
    let reader = polymarket_record_batch_reader(file, object_path, allowed_assets)?;

    let mut rows = Vec::new();
    let mut source_row_index: u64 = 0;
    for batch in reader {
        let batch = batch.context("read Polymarket CLOB record batch")?;
        let column = |name: &str| {
            batch
                .column_by_name(name)
                .with_context(|| format!("Polymarket CLOB column {name:?} missing"))
        };
        // The accepted archive stores both clocks as TIMESTAMP_MILLIS, but the
        // unit is read from the column type rather than assumed so the decoder
        // cannot silently misscale a producer that differs.
        let timestamp_received = timestamp_column_to_nanos(&batch, "timestamp_received")?;
        let event_time = timestamp_column_to_nanos(&batch, "timestamp")?;
        let asset_id = string_column(&batch, "asset_id")?;
        let event_type = string_column(&batch, "event_type")?;
        let bids = string_column(&batch, "bids")?;
        let asks = string_column(&batch, "asks")?;
        let side = string_column(&batch, "side")?;
        let transaction_hash = string_column(&batch, "transaction_hash")?;
        let price = column("price")?
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .context("price column is not decimal")?;
        let size = column("size")?
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .context("size column is not decimal")?;
        let price_scale = price.scale();
        let size_scale = size.scale();
        let source_row_number = source_row_number_column(&batch)?;

        for index in 0..batch.num_rows() {
            let asset = string_cell(asset_id, index);
            let raw_source_row_index =
                source_row_index_value(source_row_number, index, source_row_index)?;
            if allowed_assets.is_some_and(|assets| !assets.contains(asset.as_str())) {
                source_row_index += 1;
                continue;
            }
            rows.push(RawClobEventRow {
                asset_id: asset,
                event_type: string_cell(event_type, index),
                timestamp_received: timestamp_received[index],
                event_time: event_time[index],
                source_row_index: raw_source_row_index,
                bids: string_cell(bids, index),
                asks: string_cell(asks, index),
                price: decimal_cell(price, index, price_scale)?,
                size: decimal_cell(size, index, size_scale)?,
                side: string_cell(side, index),
                transaction_hash: string_cell(transaction_hash, index),
            });
            source_row_index += 1;
        }
    }
    Ok(rows)
}

fn polymarket_record_batch_reader(
    file: File,
    object_path: &Path,
    allowed_assets: Option<&HashSet<String>>,
) -> Result<parquet::arrow::arrow_reader::ParquetRecordBatchReader> {
    let row_number_field = Arc::new(
        Field::new(POLYMARKET_SOURCE_ROW_NUMBER_COLUMN, DataType::Int64, false)
            .with_extension_type(RowNumber),
    );
    let mut builder = if allowed_assets.is_some() {
        let options = ArrowReaderOptions::new()
            .with_virtual_columns(vec![row_number_field])
            .context("configure Polymarket row-number virtual column")?;
        ParquetRecordBatchReaderBuilder::try_new_with_options(file, options)
    } else {
        ParquetRecordBatchReaderBuilder::try_new(file)
    }
    .with_context(|| format!("open Polymarket parquet reader {}", object_path.display()))?;

    if let Some(allowed_assets) = allowed_assets {
        let schema_desc = builder.metadata().file_metadata().schema_descr_ptr();
        let asset_id_column = schema_desc
            .columns()
            .iter()
            .position(|column| column.path().string() == "asset_id")
            .context("Polymarket CLOB parquet schema has no asset_id leaf column")?;
        let projection = ProjectionMask::leaves(&schema_desc, [asset_id_column]);
        let allowed_assets = allowed_assets.clone();
        let predicate = ArrowPredicateFn::new(projection, move |batch| {
            let asset_id = batch
                .column(0)
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| ArrowError::CastError("asset_id column is not utf8".to_string()))?;
            let mask = (0..asset_id.len())
                .map(|index| {
                    !asset_id.is_null(index)
                        && allowed_assets.contains(asset_id.value(index).trim())
                })
                .collect::<Vec<bool>>();
            Ok(BooleanArray::from(mask))
        });
        builder = builder.with_row_filter(RowFilter::new(vec![Box::new(predicate)]));
    }

    builder
        .build()
        .with_context(|| format!("build Polymarket parquet reader {}", object_path.display()))
}

fn source_row_number_column(batch: &RecordBatch) -> Result<Option<&Int64Array>> {
    batch
        .column_by_name(POLYMARKET_SOURCE_ROW_NUMBER_COLUMN)
        .map(|column| {
            column
                .as_any()
                .downcast_ref::<Int64Array>()
                .context("Polymarket source row-number column is not int64")
        })
        .transpose()
}

fn source_row_index_value(
    source_row_number: Option<&Int64Array>,
    index: usize,
    fallback: u64,
) -> Result<u64> {
    match source_row_number {
        Some(row_number) => u64::try_from(row_number.value(index))
            .context("Polymarket source row number is negative"),
        None => Ok(fallback),
    }
}

/// Stream distinct outcome-token `asset_id`s from a staged Polymarket CLOB
/// Parquet object without retaining the object's full row payload.
///
/// # Errors
///
/// Returns an error if the file cannot be opened/read as Parquet or `asset_id`
/// is missing/not UTF-8.
pub fn polymarket_clob_assets_from_parquet(object_path: &Path) -> Result<Vec<String>> {
    let file = File::open(object_path)
        .with_context(|| format!("open Polymarket parquet {}", object_path.display()))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .with_context(|| format!("open Polymarket parquet reader {}", object_path.display()))?
        .build()
        .with_context(|| format!("build Polymarket parquet reader {}", object_path.display()))?;

    let mut seen = Vec::new();
    for batch in reader {
        let batch = batch.context("read Polymarket CLOB record batch")?;
        let asset_id = string_column(&batch, "asset_id")?;
        for index in 0..batch.num_rows() {
            let asset = string_cell(asset_id, index);
            let asset = asset.trim();
            if !asset.is_empty() && !seen.iter().any(|existing| existing == asset) {
                seen.push(asset.to_string());
            }
        }
    }
    Ok(seen)
}

/// Read a Parquet timestamp column to Unix nanoseconds regardless of its stored
/// time unit. The accepted Polymarket archive stores `timestamp` and
/// `timestamp_received` as `TIMESTAMP_MILLIS`, but the unit is taken from the
/// column's Arrow type rather than assumed.
fn timestamp_column_to_nanos(
    batch: &arrow::record_batch::RecordBatch,
    name: &str,
) -> Result<Vec<i64>> {
    let column = batch
        .column_by_name(name)
        .with_context(|| format!("Polymarket CLOB column {name:?} missing"))?;
    let row_count = batch.num_rows();
    let mut out = Vec::with_capacity(row_count);
    macro_rules! scale_to_nanos {
        ($arr:ty, $factor:expr, $unit:literal) => {{
            let array = column
                .as_any()
                .downcast_ref::<$arr>()
                .with_context(|| format!("column {name:?} is not {} timestamps", $unit))?;
            for index in 0..row_count {
                out.push(
                    array
                        .value(index)
                        .checked_mul($factor)
                        .with_context(|| format!("{name:?} {} overflow scaling to nanos", $unit))?,
                );
            }
        }};
    }
    match column.data_type() {
        DataType::Timestamp(TimeUnit::Second, _) => {
            scale_to_nanos!(TimestampSecondArray, 1_000_000_000, "second")
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            scale_to_nanos!(TimestampMillisecondArray, 1_000_000, "millisecond")
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            scale_to_nanos!(TimestampMicrosecondArray, 1_000, "microsecond")
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            let array = column
                .as_any()
                .downcast_ref::<TimestampNanosecondArray>()
                .with_context(|| format!("column {name:?} is not nanosecond timestamps"))?;
            for index in 0..row_count {
                out.push(array.value(index));
            }
        }
        other => bail!("Polymarket CLOB column {name:?} is not a timestamp: {other:?}"),
    }
    Ok(out)
}

/// Borrow a UTF-8 column by name from a record batch.
fn string_column<'a>(
    batch: &'a arrow::record_batch::RecordBatch,
    name: &str,
) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("Polymarket CLOB column {name:?} missing"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("Polymarket CLOB column {name:?} is not utf8"))
}

/// Read a UTF-8 cell, mapping null to the empty string.
fn string_cell(array: &StringArray, index: usize) -> String {
    if array.is_null(index) {
        String::new()
    } else {
        array.value(index).to_string()
    }
}

/// Render a `Decimal128` cell at the column scale, or the empty string when null
/// (the `price`/`size` columns are null for `book` snapshot rows).
fn decimal_cell(array: &Decimal128Array, index: usize, scale: i8) -> Result<String> {
    if array.is_null(index) {
        return Ok(String::new());
    }
    let scale = u32::try_from(scale).context("negative decimal scale")?;
    Ok(Decimal::from_i128_with_scale(array.value(index), scale).to_string())
}

/// Distinct outcome-token `asset_id`s appearing in decoded CLOB rows, in
/// first-seen order.
///
/// A staged object can interleave more than one outcome token, so the bulk
/// converter writes one catalog stream per distinct `asset_id` rather than
/// assuming a single one.
#[must_use]
pub fn polymarket_clob_assets(rows: &[RawClobEventRow]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for row in rows {
        let asset = row.asset_id.trim();
        if !asset.is_empty() && !seen.iter().any(|s| s == asset) {
            seen.push(asset.to_string());
        }
    }
    seen
}

/// Build a [`BinaryOptionInstrumentSpec`] whose price/size precision is derived
/// from the normalized table's own rows — the maximum decimal places observed
/// across every snapshot level, level change, and trade print.
///
/// Polymarket renders CLOB prices at the market's tick and share sizes at the
/// source's own scale, so the maximum observed scale is the precision
/// NautilusTrader pins per catalog file. No external instrument universe is
/// needed, and Polymarket stages none. The outcome label is the `asset_id`
/// itself (the only honest per-object outcome identity); lifecycle timestamps
/// are set open (`activation_ns = 1`, `expiration_ns = u64::MAX`) because the
/// bulk object carries no settlement metadata — these are instrument-definition
/// fields that do not affect the replayed `OrderBookDelta`/`TradeTick` payload.
///
/// # Errors
///
/// Returns an error if the table has no rows or a price/size string is not a
/// decimal.
pub fn polymarket_book_spec_from_table(
    table: &CanonicalBookTable,
) -> Result<BinaryOptionInstrumentSpec> {
    ensure!(
        !table.rows.is_empty(),
        "cannot derive Polymarket precision from an empty table"
    );
    let mut price_precision = 0u8;
    let mut size_precision = 0u8;
    for row in &table.rows {
        // (price, size) string pairs for every event variant.
        let pairs: Vec<(&str, &str)> = match &row.event {
            CanonicalBookEvent::Snapshot(snapshot) => snapshot
                .bids
                .iter()
                .chain(snapshot.asks.iter())
                .map(|level| (level.price.as_str(), level.size.as_str()))
                .collect(),
            CanonicalBookEvent::LevelChange(change) => {
                vec![(change.price.as_str(), change.size.as_str())]
            }
            CanonicalBookEvent::Trade(trade) => {
                vec![(trade.price.as_str(), trade.size.as_str())]
            }
        };
        for (price, size) in pairs {
            price_precision = price_precision.max(decimal_places(price)?);
            size_precision = size_precision.max(decimal_places(size)?);
        }
    }
    Ok(BinaryOptionInstrumentSpec {
        nt_instrument_id: format!("{}.{}", table.instrument_id, POLYMARKET_VENUE),
        raw_symbol: table.instrument_id.clone(),
        asset_class: POLYMARKET_ASSET_CLASS.to_string(),
        quote_currency: POLYMARKET_QUOTE_CURRENCY.to_string(),
        outcome: table.instrument_id.clone(),
        activation_ns: 1,
        expiration_ns: u64::MAX,
        price_increment: increment_for(price_precision),
        size_increment: increment_for(size_precision),
    })
}

fn append_polymarket_asset_run(
    accepted: &AcceptedDataset,
    asset_id: &str,
    mut asset_rows: Vec<RawClobEventRow>,
    catalog: &mut ParquetDataCatalog,
    projection: PolymarketAppendProjection,
) -> Result<Option<PolymarketAppendSummary>> {
    asset_rows.sort_by_key(|row| (row.timestamp_received, row.source_row_index));
    let asset_refs: Vec<&RawClobEventRow> = asset_rows.iter().collect();
    let table = normalize_sorted_polymarket_clob_book_rows(
        accepted,
        asset_id,
        &asset_refs,
        POLYMARKET_INGEST_RUN_ID,
    )?;
    let spec = polymarket_book_spec_from_table(&table)?;
    let instrument = build_binary_option(&spec)?;
    let ticks: Vec<TradeTick> = canonical_book_rows_to_trade_ticks(&table, &instrument)?;

    match projection {
        PolymarketAppendProjection::BookAndTrades => {
            let deltas: Vec<OrderBookDelta> =
                canonical_rows_to_order_book_deltas(&table, &instrument)?;
            let summary = PolymarketAppendSummary {
                nt_instrument_id: spec.nt_instrument_id.clone(),
                delta_count: deltas.len(),
                trade_count: ticks.len(),
                price_precision: instrument.price_precision,
                size_precision: instrument.size_precision,
            };
            catalog
                .write_instruments(vec![InstrumentAny::BinaryOption(instrument)])
                .with_context(|| {
                    format!("append Polymarket instrument for {}", spec.nt_instrument_id)
                })?;
            catalog
                .write_to_parquet(deltas, None, None, None)
                .with_context(|| {
                    format!(
                        "append Polymarket book deltas for {}",
                        spec.nt_instrument_id
                    )
                })?;
            if !ticks.is_empty() {
                catalog
                    .write_to_parquet(ticks, None, None, None)
                    .with_context(|| {
                        format!(
                            "append Polymarket book trades for {}",
                            spec.nt_instrument_id
                        )
                    })?;
            }
            Ok(Some(summary))
        }
        PolymarketAppendProjection::TradesOnly => {
            if ticks.is_empty() {
                return Ok(None);
            }
            let summary = PolymarketAppendSummary {
                nt_instrument_id: spec.nt_instrument_id.clone(),
                delta_count: 0,
                trade_count: ticks.len(),
                price_precision: instrument.price_precision,
                size_precision: instrument.size_precision,
            };
            catalog
                .write_instruments(vec![InstrumentAny::BinaryOption(instrument)])
                .with_context(|| {
                    format!("append Polymarket instrument for {}", spec.nt_instrument_id)
                })?;
            catalog
                .write_to_parquet(ticks, None, None, None)
                .with_context(|| {
                    format!(
                        "append Polymarket trade prints for {}",
                        spec.nt_instrument_id
                    )
                })?;
            Ok(Some(summary))
        }
    }
}

fn append_polymarket_contiguous_asset_runs_archive(
    object_path: &Path,
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
    allowed_assets: Option<&HashSet<String>>,
    projection: PolymarketAppendProjection,
) -> Result<Vec<PolymarketAppendSummary>> {
    let accepted = polymarket_accepted_dataset(object_path, object_key)?;
    let file = File::open(object_path)
        .with_context(|| format!("open Polymarket parquet {}", object_path.display()))?;
    let reader = polymarket_record_batch_reader(file, object_path, allowed_assets)?;

    let mut summaries = Vec::new();
    let mut finished_assets = HashSet::new();
    let mut current_asset: Option<String> = None;
    let mut current_rows = Vec::new();
    let mut source_row_index: u64 = 0;

    for batch in reader {
        let batch = batch.context("read Polymarket CLOB record batch")?;
        let column = |name: &str| {
            batch
                .column_by_name(name)
                .with_context(|| format!("Polymarket CLOB column {name:?} missing"))
        };
        let timestamp_received = timestamp_column_to_nanos(&batch, "timestamp_received")?;
        let event_time = timestamp_column_to_nanos(&batch, "timestamp")?;
        let asset_id = string_column(&batch, "asset_id")?;
        let event_type = string_column(&batch, "event_type")?;
        let bids = string_column(&batch, "bids")?;
        let asks = string_column(&batch, "asks")?;
        let side = string_column(&batch, "side")?;
        let transaction_hash = string_column(&batch, "transaction_hash")?;
        let price = column("price")?
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .context("price column is not decimal")?;
        let size = column("size")?
            .as_any()
            .downcast_ref::<Decimal128Array>()
            .context("size column is not decimal")?;
        let price_scale = price.scale();
        let size_scale = size.scale();
        let source_row_number = source_row_number_column(&batch)?;

        for index in 0..batch.num_rows() {
            let raw_source_row_index =
                source_row_index_value(source_row_number, index, source_row_index)?;
            let asset = string_cell(asset_id, index).trim().to_string();
            ensure!(
                !asset.is_empty(),
                "Polymarket CLOB row {raw_source_row_index} in {object_key:?} has empty asset_id"
            );
            if allowed_assets.is_some_and(|assets| !assets.contains(asset.as_str())) {
                if current_asset.as_deref() != Some(asset.as_str())
                    && let Some(flush_asset) = current_asset.take()
                {
                    if let Some(summary) = append_polymarket_asset_run(
                        &accepted,
                        &flush_asset,
                        std::mem::take(&mut current_rows),
                        catalog,
                        projection,
                    )? {
                        summaries.push(summary);
                    }
                    finished_assets.insert(flush_asset);
                }
                source_row_index += 1;
                continue;
            }
            if current_asset.as_deref() != Some(asset.as_str()) {
                if let Some(flush_asset) = current_asset.take() {
                    if let Some(summary) = append_polymarket_asset_run(
                        &accepted,
                        &flush_asset,
                        std::mem::take(&mut current_rows),
                        catalog,
                        projection,
                    )? {
                        summaries.push(summary);
                    }
                    finished_assets.insert(flush_asset);
                }
                ensure!(
                    !finished_assets.contains(asset.as_str()),
                    "Polymarket CLOB object {object_key:?} is not grouped by asset_id; \
                     asset_id {asset:?} appears in multiple non-contiguous runs"
                );
                current_asset = Some(asset.clone());
            }

            current_rows.push(RawClobEventRow {
                asset_id: asset,
                event_type: string_cell(event_type, index),
                timestamp_received: timestamp_received[index],
                event_time: event_time[index],
                source_row_index: raw_source_row_index,
                bids: string_cell(bids, index),
                asks: string_cell(asks, index),
                price: decimal_cell(price, index, price_scale)?,
                size: decimal_cell(size, index, size_scale)?,
                side: string_cell(side, index),
                transaction_hash: string_cell(transaction_hash, index),
            });
            source_row_index += 1;
        }
    }

    if let Some(flush_asset) = current_asset.take()
        && let Some(summary) =
            append_polymarket_asset_run(&accepted, &flush_asset, current_rows, catalog, projection)?
    {
        summaries.push(summary);
    }

    if summaries.is_empty() {
        if allowed_assets.is_some() {
            return Ok(summaries);
        }
        match projection {
            PolymarketAppendProjection::BookAndTrades => {
                bail!("Polymarket book object {object_key:?} yielded no instruments")
            }
            PolymarketAppendProjection::TradesOnly => {
                bail!("Polymarket trades object {object_key:?} yielded no trade prints")
            }
        }
    }

    Ok(summaries)
}

/// Append every outcome token's CLOB book stream from one staged Polymarket
/// `polymarket_book/` Parquet object into an already-open
/// [`ParquetDataCatalog`] — the bulk-conversion path for the full book family.
///
/// Decodes the Parquet, normalizes per `asset_id`, derives precision from each
/// instrument's own rows, then writes the binary-option instrument, the
/// [`OrderBookDelta`] projection (snapshot expansion + level deltas), and the
/// [`TradeTick`] projection (trade prints) for each instrument with
/// NautilusTrader's own `write_to_parquet`. Unlike
/// [`super::catalog_projection::project_canonical_book_to_catalog`] (the hermetic
/// single-object proof harness, which refuses a dirty root), this appends into a
/// shared, possibly-S3 catalog with no clean-root guard. Returns one summary per
/// distinct instrument written.
///
/// `object_path` is the locally-readable staged object; `object_key` is its
/// staged key (for example
/// `polymarket_parquet/polymarket_book/dt=YYYY-MM-DD/object=<hash>.parquet`),
/// whose `dt=` segment supplies the honest archive date for provenance.
///
/// # Errors
///
/// Returns an error if decoding, provenance construction, normalization,
/// projection, or the catalog write fails, or if the object yields no instruments.
pub fn append_polymarket_book_archive(
    object_path: &Path,
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<PolymarketAppendSummary>> {
    append_polymarket_contiguous_asset_runs_archive(
        object_path,
        object_key,
        catalog,
        None,
        PolymarketAppendProjection::BookAndTrades,
    )
}

/// Append only selected Polymarket outcome-token CLOB streams from one staged
/// object. Objects with no selected asset IDs are successful no-op appends so a
/// sparse market allowlist can be applied across a broad archive prefix.
///
/// # Errors
///
/// Returns an error if the selected rows fail decoding, provenance
/// construction, normalization, projection, catalog write, or grouped-run
/// validation.
pub fn append_polymarket_book_archive_for_assets(
    object_path: &Path,
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
    allowed_assets: &HashSet<String>,
) -> Result<Vec<PolymarketAppendSummary>> {
    append_polymarket_contiguous_asset_runs_archive(
        object_path,
        object_key,
        catalog,
        Some(allowed_assets),
        PolymarketAppendProjection::BookAndTrades,
    )
}

/// Append every outcome token's trade prints from one staged Polymarket
/// `polymarket_trades/` Parquet object into an already-open
/// [`ParquetDataCatalog`] — the bulk-conversion path for the trades family.
///
/// Identical decode/normalize/precision pipeline as
/// [`append_polymarket_book_archive`], but writes ONLY the [`TradeTick`]
/// projection (the `polymarket_trades/` objects carry trade prints), with no
/// clean-root guard. An object that yields a trade-free instrument is fenced out
/// (no instrument written) rather than writing an empty stream. Returns one
/// summary per distinct instrument with at least one trade print.
///
/// # Errors
///
/// Returns an error if decoding, provenance construction, normalization,
/// projection, or the catalog write fails, or if the object yields no trade
/// prints for any instrument.
pub fn append_polymarket_trades_archive(
    object_path: &Path,
    object_key: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<PolymarketAppendSummary>> {
    append_polymarket_contiguous_asset_runs_archive(
        object_path,
        object_key,
        catalog,
        None,
        PolymarketAppendProjection::TradesOnly,
    )
}
