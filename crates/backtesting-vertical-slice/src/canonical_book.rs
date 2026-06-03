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

use anyhow::{Context, Result, bail, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::source_proof::{AcceptedDataset, SourceProofFidelityClass};

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
/// and in the runner. `event_time` is the `timestamp` column converted to Unix
/// nanoseconds (UTC); `bids`/`asks`/`price`/`size`/`side`/`transaction_hash`
/// carry the exact source strings (empty when the column was null for the row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawClobEventRow {
    pub asset_id: String,
    pub event_type: String,
    /// Event instant in Unix nanoseconds (UTC).
    pub event_time: i64,
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
    /// Source event timestamp in Unix nanoseconds (UTC).
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
/// Returns an error if the `event_type` is unknown, a required field for the
/// event type is missing/empty, or a numeric field fails to parse.
fn decode_event(row: &RawClobEventRow) -> Result<CanonicalBookEvent> {
    match row.event_type.as_str() {
        EVENT_TYPE_BOOK => {
            ensure!(!row.bids.trim().is_empty(), "book row: empty bids");
            ensure!(!row.asks.trim().is_empty(), "book row: empty asks");
            let bids = decode_levels(&row.bids, "bids")?;
            let asks = decode_levels(&row.asks, "asks")?;
            ensure!(
                !bids.is_empty() || !asks.is_empty(),
                "book row: snapshot has no levels"
            );
            Ok(CanonicalBookEvent::Snapshot(BookSnapshot { bids, asks }))
        }
        EVENT_TYPE_PRICE_CHANGE => {
            let side = BookSide::parse_clob(&row.side)?;
            let (price, size, size_dec) = parse_price_size(&row.price, &row.size, "price_change")?;
            Ok(CanonicalBookEvent::LevelChange(LevelChange {
                side,
                price,
                size,
                is_removal: size_dec == Decimal::ZERO,
            }))
        }
        EVENT_TYPE_LAST_TRADE_PRICE => {
            let side = BookSide::parse_clob(&row.side)?;
            let (price, size, size_dec) =
                parse_price_size(&row.price, &row.size, "last_trade_price")?;
            ensure!(
                size_dec > Decimal::ZERO,
                "last_trade_price: non-positive size {:?}",
                row.size
            );
            Ok(CanonicalBookEvent::Trade(BookTrade {
                side,
                price,
                size,
                transaction_hash: row.transaction_hash.trim().to_string(),
            }))
        }
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
    capture_time_nanos: i64,
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
    for raw in rows.iter().filter(|r| r.asset_id == asset_id) {
        let event = decode_event(raw)
            .with_context(|| format!("sequence {sequence}: failed to decode CLOB event"))?;
        canonical_rows.push(CanonicalBookRow {
            schema_version: NORMALIZED_BOOK_SCHEMA_VERSION.to_string(),
            ingest_run_id: ingest_run_id.to_string(),
            source_binding: accepted.source_binding.clone(),
            venue: accepted.venue.clone(),
            product_family: accepted.product_family.clone(),
            product_category: accepted.product_category.clone(),
            instrument_id: asset_id.to_string(),
            canonical_instrument_key: canonical_instrument_key.clone(),
            event_time: raw.event_time,
            capture_time: capture_time_nanos,
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
            ensure!(
                !snapshot.bids.is_empty() || !snapshot.asks.is_empty(),
                "row {index}: snapshot has no levels"
            );
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
