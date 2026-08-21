//! Gate 2 — canonical normalized order-book-delta, bar, quote, index-price,
//! mark-price, and funding-rate tables.
//!
//! Extends the canonical normalization layer beyond native `trades`
//! ([`super::canonical_trades`]) to the additional NautilusTrader data families
//! this slice projects: aggregated L2 order-book deltas, externally-aggregated
//! OHLCV bars, top-of-book quotes, index-price reference updates, mark-price
//! reference updates, and funding-rate updates. Every table carries the same
//! identity and provenance
//! header shape as [`super::canonical_trades::CanonicalTradesTable`] and
//! preserves the exact source price/size strings, so the catalog projection in
//! [`super::catalog_projection`] is the single bridge from accepted evidence to
//! the NautilusTrader catalog.
//!
//! These tables are produced from accepted evidence only — raw staged data never
//! reaches this module without first passing source-proof acceptance — and each
//! family binds to its own fidelity class: order-book deltas require
//! [`SourceProofFidelityClass::L2Replay`], bars require
//! [`SourceProofFidelityClass::TradeBarReplay`], top-of-book quotes require
//! [`SourceProofFidelityClass::QuoteReplay`], index-price updates require
//! [`SourceProofFidelityClass::IndexReplay`], mark-price updates require
//! [`SourceProofFidelityClass::MarkReplay`], and funding-rate updates require
//! [`SourceProofFidelityClass::FundingReplay`].

use std::{fs::File, path::Path, sync::Arc};

use anyhow::{Context, Result, ensure};
use arrow::{
    array::{ArrayRef, Int64Array, StringArray, UInt8Array, UInt16Array, UInt64Array},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use nautilus_model::{
    data::BarSpecification,
    enums::{BarAggregation, PriceType, RecordFlag},
};
use parquet::arrow::ArrowWriter;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{canonical_trades::TradesPartition, source_proof::SourceProofFidelityClass};

/// Contracted semantic schema version for normalized market-data rows.
///
/// Shared with [`super::canonical_trades::NORMALIZED_SCHEMA_VERSION`]: the
/// order-book-delta and bar families live in the same `market_data.v1`
/// contract as the native `trades` family.
pub const NORMALIZED_SCHEMA_VERSION: &str = "market_data.v1";

/// Order-book delta action in the canonical L2 vocabulary.
///
/// Mirrors the `as_str` pattern of
/// [`super::canonical_trades::TradeAggressorSide`]: the string form is the
/// stable wire/serialization token, distinct from NautilusTrader's own
/// `BookAction` (which this maps onto at projection).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DeltaAction {
    Clear,
    Add,
    Update,
    Delete,
}

impl DeltaAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "CLEAR",
            Self::Add => "ADD",
            Self::Update => "UPDATE",
            Self::Delete => "DELETE",
        }
    }
}

/// Book side token for an order-book delta row.
///
/// Empty only for [`DeltaAction::Clear`] rows, which carry no order side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum DeltaSide {
    Buy,
    Sell,
}

impl DeltaSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "BUY",
            Self::Sell => "SELL",
        }
    }
}

/// One normalized order-book delta row with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the payload fields
/// (`event_time`, `action`, `side`, `price`, `size`, `order_id`, and `flags`)
/// describe a single NautilusTrader `OrderBookDelta`. `sequence` is canonical
/// audit ordering; NT sequence comes from `source_sequence` when available.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalOrderBookDeltaRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Exchange/source event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from event time.
    pub availability_time: Option<i64>,
    /// Native source sequence/print identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Delta action token (`CLEAR`/`ADD`/`UPDATE`/`DELETE`).
    pub action: String,
    /// Order side token (`BUY`/`SELL`), empty only for `CLEAR`.
    pub side: String,
    /// Exact source price string; empty for `CLEAR`.
    pub price: String,
    /// Exact source size string; empty for `CLEAR`.
    pub size: String,
    /// Order id; `0` for price-keyed L2 (MBP) levels.
    pub order_id: u64,
    /// `RecordFlag` bitmask carried verbatim into the NautilusTrader delta.
    pub flags: u8,
    /// Dense monotonic converter row ordinal used for canonical audit ordering.
    /// This is distinct from the venue-native [`Self::source_sequence`].
    pub sequence: u64,
}

/// A validated canonical normalized order-book-delta table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalOrderBookDeltasTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalOrderBookDeltaRow>,
}

impl CanonicalOrderBookDeltasTable {
    /// Validate required fields, fidelity class, timestamps, audit ordering,
    /// L2 flags, and `F_LAST`-closed source-event consistency.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(
            !self.rows.is_empty(),
            "canonical order book deltas table is empty"
        );
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::L2Replay,
            "order book deltas must be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "order-book-delta table must carry explicit forbidden claims"
        );

        let snapshot_flag = RecordFlag::F_SNAPSHOT as u8;
        let last_flag = RecordFlag::F_LAST as u8;
        let mbp_flag = RecordFlag::F_MBP as u8;
        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
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
                row.sequence == index as u64,
                "row {index}: sequence {} is not dense ascending from 0",
                row.sequence
            );
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
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
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            ensure!(
                row.flags & mbp_flag == mbp_flag,
                "row {index}: L2 order-book delta flags must contain F_MBP"
            );
            validate_delta_action_payload(index, row, snapshot_flag)?;
        }

        validate_delta_events(&self.rows, last_flag, snapshot_flag)?;
        Ok(())
    }
}

/// Validate the action-specific payload contract for one delta row.
fn validate_delta_action_payload(
    index: usize,
    row: &CanonicalOrderBookDeltaRow,
    snapshot_flag: u8,
) -> Result<()> {
    match row.action.as_str() {
        action if action == DeltaAction::Clear.as_str() => {
            ensure!(
                row.side.is_empty(),
                "row {index}: CLEAR row must have empty side"
            );
            ensure!(
                row.price.is_empty(),
                "row {index}: CLEAR row must have empty price"
            );
            ensure!(
                row.size.is_empty(),
                "row {index}: CLEAR row must have empty size"
            );
            // The table-wide L2 contract already requires F_MBP. CLEAR also
            // requires F_SNAPSHOT so replay can reset the book atomically.
            ensure!(
                row.flags & snapshot_flag == snapshot_flag,
                "row {index}: CLEAR row flags must contain F_SNAPSHOT"
            );
        }
        action
            if action == DeltaAction::Add.as_str()
                || action == DeltaAction::Update.as_str()
                || action == DeltaAction::Delete.as_str() =>
        {
            ensure!(
                row.side == DeltaSide::Buy.as_str() || row.side == DeltaSide::Sell.as_str(),
                "row {index}: {action} row side {:?} must be BUY or SELL",
                row.side
            );
            ensure!(
                !row.price.trim().is_empty(),
                "row {index}: {action} row must have non-empty price"
            );
            ensure!(
                !row.size.trim().is_empty(),
                "row {index}: {action} row must have non-empty size"
            );
            if action == DeltaAction::Add.as_str() || action == DeltaAction::Update.as_str() {
                let size = row.size.parse::<Decimal>().map_err(|error| {
                    anyhow::anyhow!("row {index}: invalid size {:?}: {error}", row.size)
                })?;
                ensure!(
                    size > Decimal::ZERO,
                    "row {index}: {action} row must have positive size"
                );
            }
        }
        other => {
            anyhow::bail!("row {index}: unknown delta action {other:?}");
        }
    }
    Ok(())
}

/// Validate that every source event closes with exactly one final `F_LAST`.
///
/// Every book event ends with exactly one `F_LAST` row. An event starts at row
/// 0 or immediately after a row carrying `F_LAST`. A snapshot expansion is one
/// such event whose first row is a `CLEAR` (and whose final row carries
/// `F_LAST`), except that a snapshot immediately following a lone `CLEAR` may
/// contain only `ADD` rows because the book is already established empty. An
/// incremental source event can carry one or more level changes; only its final
/// row carries `F_LAST`. A `CLEAR` may therefore appear only at an event start,
/// and the final row of the table must close its event.
fn validate_delta_events(
    rows: &[CanonicalOrderBookDeltaRow],
    last_flag: u8,
    snapshot_flag: u8,
) -> Result<()> {
    let mut event_start = 0;
    let mut book_established_empty = false;
    for (index, row) in rows.iter().enumerate() {
        if index != event_start {
            validate_same_delta_event(index, &rows[event_start], row)?;
        }
        let is_clear = row.action == DeltaAction::Clear.as_str();
        if is_clear {
            ensure!(
                index == event_start,
                "row {index}: CLEAR may only begin a book event (previous event not closed with F_LAST)"
            );
        }
        let event_is_snapshot = rows[event_start].flags & snapshot_flag != 0;
        if index == event_start && event_is_snapshot && !is_clear {
            ensure!(
                book_established_empty,
                "row {index}: F_SNAPSHOT event without CLEAR requires an immediately preceding lone CLEAR"
            );
        }
        let carries_snapshot = row.flags & snapshot_flag != 0;
        if event_is_snapshot {
            ensure!(
                carries_snapshot,
                "row {index}: every row in a snapshot event must contain F_SNAPSHOT"
            );
            if !is_clear {
                ensure!(
                    row.action == DeltaAction::Add.as_str(),
                    "row {index}: snapshot payload rows must use ADD"
                );
            }
        } else {
            ensure!(
                !carries_snapshot,
                "row {index}: incremental event rows must not contain F_SNAPSHOT"
            );
        }
        let closes_event = row.flags & last_flag != 0;
        if closes_event {
            book_established_empty = index == event_start && is_clear;
            event_start = index + 1;
        }
    }
    ensure!(
        event_start == rows.len(),
        "row {}: final book event is not closed with F_LAST",
        rows.len() - 1
    );
    Ok(())
}

fn validate_same_delta_event(
    index: usize,
    first: &CanonicalOrderBookDeltaRow,
    row: &CanonicalOrderBookDeltaRow,
) -> Result<()> {
    ensure!(
        row.event_time == first.event_time,
        "row {index}: event_time {} differs from source-event start {}",
        row.event_time,
        first.event_time
    );
    ensure!(
        row.availability_time == first.availability_time,
        "row {index}: availability_time {:?} differs from source-event start {:?}",
        row.availability_time,
        first.availability_time
    );
    ensure!(
        row.source_sequence == first.source_sequence,
        "row {index}: source_sequence {:?} differs from source-event start {:?}",
        row.source_sequence,
        first.source_sequence
    );
    Ok(())
}

/// Bar aggregation specification for a canonical bar table.
///
/// `aggregation` is NautilusTrader's own [`BarAggregation`] (it serializes as a
/// stable SCREAMING_SNAKE_CASE token), so the canonical bar spec is the same
/// vocabulary the catalog projection binds to without a parallel local enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBarSpec {
    pub step: usize,
    pub aggregation: BarAggregation,
}

/// One normalized OHLCV bar row with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the payload fields
/// describe a single externally-aggregated NautilusTrader `Bar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBarRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Bar open timestamp in Unix nanoseconds.
    pub open_time: i64,
    /// Bar close timestamp in Unix nanoseconds.
    pub close_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from close time.
    pub availability_time: Option<i64>,
    /// Native source sequence identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Exact source open price string.
    pub open: String,
    /// Exact source high price string.
    pub high: String,
    /// Exact source low price string.
    pub low: String,
    /// Exact source close price string.
    pub close: String,
    /// Exact source volume string.
    pub volume: String,
}

/// A validated canonical normalized bar table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalBarsTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub bar_spec: CanonicalBarSpec,
    pub rows: Vec<CanonicalBarRow>,
}

impl CanonicalBarsTable {
    /// Validate required fields, fidelity class, bar step, OHLC ordering, and
    /// bar timestamps.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(!self.rows.is_empty(), "canonical bars table is empty");
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(self.bar_spec.step > 0, "bar step must be positive");
        // The canonical table is the single source of truth for bar-spec
        // admissibility: probe NautilusTrader's own step/aggregation
        // periodicity rules here instead of deferring the failure to
        // catalog projection.
        BarSpecification::new_checked(
            self.bar_spec.step,
            self.bar_spec.aggregation,
            PriceType::Last,
        )
        .with_context(|| {
            format!(
                "bar_spec step {} is not a valid {:?} specification",
                self.bar_spec.step, self.bar_spec.aggregation
            )
        })?;
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::TradeBarReplay,
            "bars must be labelled TRADE_BAR_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "bar table must carry explicit forbidden claims"
        );

        let mut previous_open_time = i64::MIN;
        let mut previous_close_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
                &row.open,
                &row.high,
                &row.low,
                &row.close,
                &row.volume,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            ensure!(row.open_time > 0, "row {index}: non-positive open_time");
            ensure!(
                row.open_time > previous_open_time,
                "row {index}: open_time {} does not strictly increase from previous {}",
                row.open_time,
                previous_open_time
            );
            previous_open_time = row.open_time;
            ensure!(
                row.close_time >= row.open_time,
                "row {index}: close_time {} precedes open_time {}",
                row.close_time,
                row.open_time
            );
            // The catalog write orders bars by ts_init (= close_time) and
            // requires it non-decreasing; enforce that here so the validated
            // contract matches what the write step accepts.
            ensure!(
                row.close_time >= previous_close_time,
                "row {index}: close_time {} precedes previous {}",
                row.close_time,
                previous_close_time
            );
            previous_close_time = row.close_time;
            validate_bar_ohlcv(index, row)?;
        }
        Ok(())
    }
}

/// Validate the OHLC ordering invariant and non-negative volume for one bar row.
fn validate_bar_ohlcv(index: usize, row: &CanonicalBarRow) -> Result<()> {
    let parse = |value: &str, label: &str| -> Result<Decimal> {
        value
            .parse::<Decimal>()
            .map_err(|error| anyhow::anyhow!("row {index}: invalid {label} {value:?}: {error}"))
    };
    let open = parse(&row.open, "open")?;
    let high = parse(&row.high, "high")?;
    let low = parse(&row.low, "low")?;
    let close = parse(&row.close, "close")?;
    let volume = parse(&row.volume, "volume")?;
    ensure!(high >= open, "row {index}: high {high} below open {open}");
    ensure!(high >= low, "row {index}: high {high} below low {low}");
    ensure!(
        high >= close,
        "row {index}: high {high} below close {close}"
    );
    ensure!(low <= open, "row {index}: low {low} above open {open}");
    ensure!(low <= close, "row {index}: low {low} above close {close}");
    ensure!(
        volume >= Decimal::ZERO,
        "row {index}: negative volume {volume}"
    );
    Ok(())
}

/// One normalized top-of-book quote row with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the payload fields
/// (`bid`, `ask`, `bid_size`, `ask_size`) describe a single NautilusTrader
/// `QuoteTick` (best bid/ask snapshot), preserving the exact source decimal
/// strings with the same discipline as the bar OHLCV columns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalQuoteRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Exchange/source event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from event time.
    pub availability_time: Option<i64>,
    /// Native source sequence/print identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Exact source best-bid price string.
    pub bid: String,
    /// Exact source best-ask price string.
    pub ask: String,
    /// Exact source best-bid size string (`0` for an empty bid side).
    pub bid_size: String,
    /// Exact source best-ask size string (`0` for an empty ask side).
    pub ask_size: String,
}

/// A validated canonical normalized top-of-book quote table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalQuotesTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalQuoteRow>,
}

impl CanonicalQuotesTable {
    /// Validate required fields, fidelity class, timestamps, and the top-of-book
    /// spread/positivity contract.
    ///
    /// Top-of-book best bid/ask is a SNAPSHOT of the order book, not full L2
    /// depth, so the table binds [`SourceProofFidelityClass::QuoteReplay`] — the
    /// dedicated NT quote-stream replay class admitted by the run-manifest gate
    /// (`ADMITTANCE_TABLE`) — never [`SourceProofFidelityClass::L2Replay`]
    /// (which would conflate top-of-book with full-depth and trip the
    /// L2-evidence gate).
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(!self.rows.is_empty(), "canonical quotes table is empty");
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::QuoteReplay,
            "quotes must be labelled QUOTE_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "quote table must carry explicit forbidden claims"
        );

        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            // Quotes share an event_time clock with trades/deltas: multiple
            // quotes can carry the same timestamp, so the rule is non-decreasing
            // (>=), not the bars' strictly-increasing open_time rule.
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
                &row.bid,
                &row.ask,
                &row.bid_size,
                &row.ask_size,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            validate_quote_spread(index, row)?;
        }
        Ok(())
    }
}

/// Validate the top-of-book spread/positivity invariant for one quote row.
///
/// Both sides must carry a price (`bid > 0`, `ask > 0`) and the book must not be
/// crossed (`ask >= bid`). Sizes must be non-negative; a zero side size is a
/// legitimate empty side, so sizes are not required to be strictly positive.
fn validate_quote_spread(index: usize, row: &CanonicalQuoteRow) -> Result<()> {
    let parse = |value: &str, label: &str| -> Result<Decimal> {
        value
            .parse::<Decimal>()
            .map_err(|error| anyhow::anyhow!("row {index}: invalid {label} {value:?}: {error}"))
    };
    let bid = parse(&row.bid, "bid")?;
    let ask = parse(&row.ask, "ask")?;
    let bid_size = parse(&row.bid_size, "bid_size")?;
    let ask_size = parse(&row.ask_size, "ask_size")?;
    ensure!(bid > Decimal::ZERO, "row {index}: non-positive bid {bid}");
    ensure!(ask > Decimal::ZERO, "row {index}: non-positive ask {ask}");
    ensure!(
        bid_size >= Decimal::ZERO,
        "row {index}: negative bid_size {bid_size}"
    );
    ensure!(
        ask_size >= Decimal::ZERO,
        "row {index}: negative ask_size {ask_size}"
    );
    ensure!(ask >= bid, "row {index}: ask {ask} below bid {bid}");
    Ok(())
}

/// Run-spec owned top-of-book quote column mapping for a snapshot-quote source
/// adapter.
///
/// A new source that emits the same top-of-book best-bid/ask snapshot shape
/// selects the quote converter from TOML and supplies its field mapping here.
/// Mirrors [`DeltaMappingConfig`]: the timestamp unit is the shared
/// [`super::canonical_trades::CsvTimestampUnit`] and field names are resolved
/// against each source record rather than positionally, so no column literal is
/// hardcoded in code.
///
/// SCOPE: this struct + the registered adapter + the canonical table +
/// projection land in slice S3quote; the wire-format parser that fills a
/// [`CanonicalQuotesTable`] from raw bytes is a follow-up slice. The
/// snapshot-quotes operator dispatch arm names that follow-up and fails loud
/// (the registered seam is real, not a TODO); the projection is proven by the
/// synthetic round-trip test in [`super::catalog_projection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteMappingConfig {
    /// Field name carrying the exchange book time.
    pub event_time_field: String,
    /// Unit of `event_time_field`.
    pub event_time_unit: super::canonical_trades::CsvTimestampUnit,
    /// Field name carrying the best-bid price.
    pub bid_field: String,
    /// Field name carrying the best-ask price.
    pub ask_field: String,
    /// Field name carrying the best-bid size.
    pub bid_size_field: String,
    /// Field name carrying the best-ask size.
    pub ask_size_field: String,
}

/// One normalized index-price reference update with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the single payload
/// field (`value`) is the exact source index-price string for one NautilusTrader
/// `IndexPriceUpdate` (a point update — no size, aggressor, or side; the NT
/// `IndexPriceUpdate.value` is a `Price`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalIndexPriceRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Exchange/source event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from event time.
    pub availability_time: Option<i64>,
    /// Native source sequence/print identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Exact source index-price string (the NT `IndexPriceUpdate.value` is a `Price`).
    pub value: String,
}

/// A validated canonical normalized index-price table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalIndexPricesTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalIndexPriceRow>,
}

impl CanonicalIndexPricesTable {
    /// Validate required fields, fidelity class, timestamps, and parseable values.
    ///
    /// An index/oracle reference series is a point-update SIGNAL feed, not a
    /// book/trade/bar replay. It binds [`SourceProofFidelityClass::IndexReplay`]
    /// — the dedicated NT index-price replay class admitted by the run-manifest
    /// gate (`ADMITTANCE_TABLE` binds `IndexPriceUpdate` to `IndexReplay` as a
    /// runnable Primary row). `SignalOnly` has no primary row and is deliberately
    /// non-runnable, so labelling the table `SignalOnly` would create a dead,
    /// non-runnable gate-4 binding (a dual/dead path); `IndexReplay` matches the
    /// S2 admittance table end to end.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(
            !self.rows.is_empty(),
            "canonical index prices table is empty"
        );
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::IndexReplay,
            "index prices must be labelled INDEX_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "index price table must carry explicit forbidden claims"
        );

        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            // Index prices share an event_time clock with trades/deltas: multiple
            // prints can carry the same nanosecond timestamp (NT windows/orders by
            // ts_init), so the rule is non-decreasing (>=), not the bars' strictly
            // increasing open_time rule.
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
                &row.value,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            let value = row.value.parse::<Decimal>().map_err(|error| {
                anyhow::anyhow!("row {index}: invalid value {:?}: {error}", row.value)
            })?;
            // An index reference price is strictly positive, like the quote
            // bid/ask; reject zero/negative so the projection never emits a
            // non-positive NT `IndexPriceUpdate.value` (fail loud, same class
            // of guard as `validate_quote_spread`).
            ensure!(
                value > Decimal::ZERO,
                "row {index}: non-positive value {value}"
            );
        }
        Ok(())
    }
}

/// One normalized mark-price reference update with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the single payload
/// field (`value`) is the exact source mark-price string for one NautilusTrader
/// `MarkPriceUpdate` (a point update — no size, aggressor, or side; the NT
/// `MarkPriceUpdate.value` is a `Price`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalMarkPriceRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Exchange/source event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from event time.
    pub availability_time: Option<i64>,
    /// Native source sequence/print identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Exact source mark-price string (the NT `MarkPriceUpdate.value` is a `Price`).
    pub value: String,
}

/// A validated canonical normalized mark-price table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalMarkPricesTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalMarkPriceRow>,
}

impl CanonicalMarkPricesTable {
    /// Validate required fields, fidelity class, timestamps, and parseable values.
    ///
    /// A mark/reference price series is a point-update reference feed, not a
    /// book/trade/bar replay. It binds [`SourceProofFidelityClass::MarkReplay`]
    /// — the dedicated NT mark-price replay class admitted by the run-manifest
    /// gate (`ADMITTANCE_TABLE` binds `MarkPriceUpdate` to `MarkReplay` as a
    /// runnable Primary row). `SignalOnly` has no primary row and is deliberately
    /// non-runnable, so labelling the table `SignalOnly` would create a dead,
    /// non-runnable gate-4 binding (a dual/dead path); `MarkReplay` matches the
    /// S2 admittance table end to end (the same drift the sibling S3index slice
    /// resolved by binding `IndexReplay` instead of the plan's `SignalOnly`).
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(
            !self.rows.is_empty(),
            "canonical mark prices table is empty"
        );
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::MarkReplay,
            "mark prices must be labelled MARK_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "mark price table must carry explicit forbidden claims"
        );

        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            // Mark prices share an event_time clock with trades/deltas: multiple
            // prints can carry the same nanosecond timestamp (NT windows/orders by
            // ts_init), so the rule is non-decreasing (>=), not the bars' strictly
            // increasing open_time rule.
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
                &row.value,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            let value = row.value.parse::<Decimal>().map_err(|error| {
                anyhow::anyhow!("row {index}: invalid value {:?}: {error}", row.value)
            })?;
            // A mark reference price is strictly positive, like the quote
            // bid/ask; reject zero/negative so the projection never emits a
            // non-positive NT `MarkPriceUpdate.value` (fail loud, same class
            // of guard as `validate_quote_spread`).
            ensure!(
                value > Decimal::ZERO,
                "row {index}: non-positive value {value}"
            );
        }
        Ok(())
    }
}

/// One normalized funding-rate update with full provenance.
///
/// The provenance prefix mirrors
/// [`super::canonical_trades::CanonicalTradeRow`] exactly; the payload fields
/// map onto NautilusTrader's `FundingRateUpdate` (`rate`, optional
/// `interval_minutes`, and optional `next_funding_time`). Funding rates are not
/// prices, so negative and zero rates are valid when the source reports them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalFundingRateRow {
    pub schema_version: String,
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    /// Exchange/source event timestamp in Unix nanoseconds.
    pub event_time: i64,
    /// Worker receipt/capture timestamp in Unix nanoseconds.
    pub capture_time: i64,
    /// Source availability timestamp in Unix nanoseconds, when distinct from event time.
    pub availability_time: Option<i64>,
    /// Native source sequence identity, when present.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    /// Exact source funding-rate decimal string.
    pub rate: String,
    /// Funding interval in minutes, when supplied by the venue/source.
    pub interval_minutes: Option<u16>,
    /// Next funding timestamp in Unix nanoseconds, when supplied.
    pub next_funding_time: Option<i64>,
}

/// A validated canonical normalized funding-rate table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalFundingRatesTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalFundingRateRow>,
}

impl CanonicalFundingRatesTable {
    /// Validate required fields, fidelity class, timestamps, and parseable rates.
    ///
    /// A funding-rate series is a point update stream for perpetual swaps. It
    /// binds [`SourceProofFidelityClass::FundingReplay`] and allows negative
    /// rates because exchange funding can credit either long or short side.
    ///
    /// # Errors
    ///
    /// Returns an error describing the first contract violation.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == NORMALIZED_SCHEMA_VERSION,
            "unexpected schema_version {:?}",
            self.schema_version
        );
        ensure!(
            !self.rows.is_empty(),
            "canonical funding rates table is empty"
        );
        for field in [
            &self.partition.venue,
            &self.partition.product_family,
            &self.partition.product_category,
            &self.partition.instrument_id,
            &self.partition.dt,
            &self.source_proof_id,
            &self.transform_hash,
            &self.payload_hash,
        ] {
            ensure!(!field.trim().is_empty(), "empty partition/provenance field");
        }
        ensure!(
            self.fidelity_class == SourceProofFidelityClass::FundingReplay,
            "funding rates must be labelled FUNDING_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "funding rate table must carry explicit forbidden claims"
        );

        let mut previous_event_time = i64::MIN;
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.schema_version == NORMALIZED_SCHEMA_VERSION,
                "row {index}: schema_version mismatch"
            );
            ensure!(row.event_time > 0, "row {index}: non-positive event_time");
            ensure!(
                row.capture_time > 0,
                "row {index}: non-positive capture_time"
            );
            if let Some(availability_time) = row.availability_time {
                ensure!(
                    availability_time > 0,
                    "row {index}: non-positive availability_time"
                );
            }
            ensure!(
                row.event_time >= previous_event_time,
                "row {index}: event_time {} precedes previous {}",
                row.event_time,
                previous_event_time
            );
            previous_event_time = row.event_time;
            ensure!(
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            for field in [
                &row.ingest_run_id,
                &row.source_binding,
                &row.venue,
                &row.product_family,
                &row.product_category,
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.transform_hash,
                &row.rate,
            ] {
                ensure!(
                    !field.trim().is_empty(),
                    "row {index}: empty required field"
                );
            }
            for (name, field) in [
                ("nt_instrument_id", &row.nt_instrument_id),
                ("source_sequence", &row.source_sequence),
            ] {
                if let Some(field) = field {
                    ensure!(
                        !field.trim().is_empty(),
                        "row {index}: empty nullable field {name}"
                    );
                }
            }
            row.rate.parse::<Decimal>().map_err(|error| {
                anyhow::anyhow!("row {index}: invalid rate {:?}: {error}", row.rate)
            })?;
            if let Some(interval) = row.interval_minutes {
                ensure!(interval > 0, "row {index}: interval must be positive");
            }
            if let Some(next_funding_time) = row.next_funding_time {
                ensure!(
                    next_funding_time > 0,
                    "row {index}: non-positive next_funding_time"
                );
                ensure!(
                    next_funding_time > row.event_time,
                    "row {index}: next_funding_time {} is not after event_time {}",
                    next_funding_time,
                    row.event_time
                );
            }
        }
        Ok(())
    }
}

/// Write one canonical table record batch as a Parquet artifact.
///
/// Shared by the bar and order-book-delta canonical writers; mirrors the
/// trades writer in [`super::canonical_trades::CanonicalTradesTable`].
fn write_record_batch_parquet(batch: &RecordBatch, path: &Path) -> Result<()> {
    let file = File::create(path)
        .with_context(|| format!("failed to create canonical artifact {}", path.display()))?;
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None)
        .context("failed to construct parquet writer")?;
    writer.write(batch).context("failed to write batch")?;
    writer.close().context("failed to finalize parquet")?;
    Ok(())
}

impl CanonicalOrderBookDeltasTable {
    /// Arrow schema for the canonical order-book-delta table.
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("event_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            utf8("action"),
            utf8("side"),
            utf8("price"),
            utf8("size"),
            Field::new("order_id", DataType::UInt64, false),
            Field::new("flags", DataType::UInt8, false),
            Field::new("sequence", DataType::UInt64, false),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalOrderBookDeltaRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalOrderBookDeltaRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalOrderBookDeltaRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalOrderBookDeltaRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let uint64_col = |f: fn(&CanonicalOrderBookDeltaRow) -> u64| {
            Arc::new(UInt64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let uint8_col = |f: fn(&CanonicalOrderBookDeltaRow) -> u8| {
            Arc::new(UInt8Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.event_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                utf8_col(|r| r.action.as_str()),
                utf8_col(|r| r.side.as_str()),
                utf8_col(|r| r.price.as_str()),
                utf8_col(|r| r.size.as_str()),
                uint64_col(|r| r.order_id),
                uint8_col(|r| r.flags),
                uint64_col(|r| r.sequence),
            ],
        )
        .context("failed to build canonical order-book-delta record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

impl CanonicalBarsTable {
    /// Arrow schema for the canonical bar table.
    ///
    /// `bar_step`/`bar_aggregation` repeat the table-level
    /// [`CanonicalBarSpec`] on every row so the artifact is self-describing.
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("open_time"),
            int64("close_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            int64("bar_step"),
            utf8("bar_aggregation"),
            utf8("open"),
            utf8("high"),
            utf8("low"),
            utf8("close"),
            utf8("volume"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalBarRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalBarRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalBarRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalBarRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let bar_step = i64::try_from(self.bar_spec.step).context("bar_spec step overflow")?;
        let bar_step_col = Arc::new(Int64Array::from(vec![bar_step; self.rows.len()])) as ArrayRef;
        let bar_aggregation = self.bar_spec.aggregation.to_string();
        let bar_aggregation_col = Arc::new(StringArray::from(vec![
            bar_aggregation.as_str();
            self.rows.len()
        ])) as ArrayRef;
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.open_time),
                int64_col(|r| r.close_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                bar_step_col,
                bar_aggregation_col,
                utf8_col(|r| r.open.as_str()),
                utf8_col(|r| r.high.as_str()),
                utf8_col(|r| r.low.as_str()),
                utf8_col(|r| r.close.as_str()),
                utf8_col(|r| r.volume.as_str()),
            ],
        )
        .context("failed to build canonical bar record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

impl CanonicalQuotesTable {
    /// Arrow schema for the canonical top-of-book quote table.
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("event_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            utf8("bid"),
            utf8("ask"),
            utf8("bid_size"),
            utf8("ask_size"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalQuoteRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalQuoteRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalQuoteRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalQuoteRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.event_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                utf8_col(|r| r.bid.as_str()),
                utf8_col(|r| r.ask.as_str()),
                utf8_col(|r| r.bid_size.as_str()),
                utf8_col(|r| r.ask_size.as_str()),
            ],
        )
        .context("failed to build canonical quote record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

impl CanonicalIndexPricesTable {
    /// Arrow schema for the canonical index-price table.
    ///
    /// Lists the IDENTICAL provenance columns as the quote/delta schema plus the
    /// single `value` payload column (no size/aggressor columns — an index price
    /// is a point update).
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("event_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            utf8("value"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalIndexPriceRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalIndexPriceRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalIndexPriceRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalIndexPriceRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.event_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                utf8_col(|r| r.value.as_str()),
            ],
        )
        .context("failed to build canonical index price record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

impl CanonicalMarkPricesTable {
    /// Arrow schema for the canonical mark-price table.
    ///
    /// Lists the IDENTICAL provenance columns as the quote/index/delta schema
    /// plus the single `value` payload column (no size/aggressor columns — a
    /// mark price is a point update).
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("event_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            utf8("value"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalMarkPriceRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalMarkPriceRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalMarkPriceRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalMarkPriceRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.event_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                utf8_col(|r| r.value.as_str()),
            ],
        )
        .context("failed to build canonical mark price record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

impl CanonicalFundingRatesTable {
    /// Arrow schema for the canonical funding-rate table.
    ///
    /// Lists the IDENTICAL provenance columns as the quote/index/mark schema
    /// plus the funding-specific `rate`, `interval_minutes`, and
    /// `next_funding_time` payload columns.
    #[must_use]
    pub fn arrow_schema() -> Arc<Schema> {
        let utf8 = |name: &str| Field::new(name, DataType::Utf8, false);
        let utf8_nullable = |name: &str| Field::new(name, DataType::Utf8, true);
        let int64 = |name: &str| Field::new(name, DataType::Int64, false);
        let int64_nullable = |name: &str| Field::new(name, DataType::Int64, true);
        let uint16_nullable = |name: &str| Field::new(name, DataType::UInt16, true);
        Arc::new(Schema::new(vec![
            utf8("schema_version"),
            utf8("ingest_run_id"),
            utf8("source_binding"),
            utf8("venue"),
            utf8("product_family"),
            utf8("product_category"),
            utf8("instrument_id"),
            utf8("canonical_instrument_key"),
            utf8("venue_symbol"),
            utf8_nullable("nt_instrument_id"),
            int64("event_time"),
            int64("capture_time"),
            int64_nullable("availability_time"),
            utf8_nullable("source_sequence"),
            utf8("raw_payload_id"),
            utf8("source_proof_id"),
            utf8("payload_hash"),
            utf8("transform_hash"),
            utf8("rate"),
            uint16_nullable("interval_minutes"),
            int64_nullable("next_funding_time"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalFundingRateRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let int64_col = |f: fn(&CanonicalFundingRateRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalFundingRateRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalFundingRateRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        let opt_u16_col = |f: fn(&CanonicalFundingRateRow) -> Option<u16>| {
            Arc::new(UInt16Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as ArrayRef
        };
        RecordBatch::try_new(
            Self::arrow_schema(),
            vec![
                utf8_col(|r| r.schema_version.as_str()),
                utf8_col(|r| r.ingest_run_id.as_str()),
                utf8_col(|r| r.source_binding.as_str()),
                utf8_col(|r| r.venue.as_str()),
                utf8_col(|r| r.product_family.as_str()),
                utf8_col(|r| r.product_category.as_str()),
                utf8_col(|r| r.instrument_id.as_str()),
                utf8_col(|r| r.canonical_instrument_key.as_str()),
                utf8_col(|r| r.venue_symbol.as_str()),
                opt_utf8_col(|r| r.nt_instrument_id.as_deref()),
                int64_col(|r| r.event_time),
                int64_col(|r| r.capture_time),
                opt_int64_col(|r| r.availability_time),
                opt_utf8_col(|r| r.source_sequence.as_deref()),
                utf8_col(|r| r.raw_payload_id.as_str()),
                utf8_col(|r| r.source_proof_id.as_str()),
                utf8_col(|r| r.payload_hash.as_str()),
                utf8_col(|r| r.transform_hash.as_str()),
                utf8_col(|r| r.rate.as_str()),
                opt_u16_col(|r| r.interval_minutes),
                opt_int64_col(|r| r.next_funding_time),
            ],
        )
        .context("failed to build canonical funding rate record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        write_record_batch_parquet(&self.to_record_batch()?, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn partition() -> TradesPartition {
        TradesPartition {
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            dt: "2026-05-22".to_string(),
        }
    }

    fn delta_row(
        sequence: u64,
        event_time: i64,
        action: DeltaAction,
        side: &str,
        price: &str,
        size: &str,
        flags: u8,
    ) -> CanonicalOrderBookDeltaRow {
        CanonicalOrderBookDeltaRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "testvenue/prediction-market/YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            event_time,
            capture_time: event_time,
            availability_time: None,
            source_sequence: None,
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            action: action.as_str().to_string(),
            side: side.to_string(),
            price: price.to_string(),
            size: size.to_string(),
            order_id: 0,
            flags,
            sequence,
        }
    }

    fn snapshot_table() -> CanonicalOrderBookDeltasTable {
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let last = RecordFlag::F_LAST as u8;
        let event_time = 1_700_000_000_000_000_000;
        let rows = vec![
            delta_row(
                0,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot_flags,
            ),
            delta_row(
                1,
                event_time,
                DeltaAction::Add,
                DeltaSide::Buy.as_str(),
                "0.49",
                "10",
                snapshot_flags,
            ),
            delta_row(
                2,
                event_time,
                DeltaAction::Add,
                DeltaSide::Sell.as_str(),
                "0.51",
                "12",
                snapshot_flags | last,
            ),
        ];
        CanonicalOrderBookDeltasTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: partition(),
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::L2Replay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn deltas_validate_accepts_snapshot_expansion() {
        snapshot_table()
            .validate()
            .expect("snapshot expansion is valid");
    }

    #[test]
    fn deltas_validate_accepts_snapshot_adds_after_lone_clear() {
        let mut table = snapshot_table();
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let last = RecordFlag::F_LAST as u8;
        let event_time = table.rows[0].event_time;
        table.rows = vec![
            delta_row(
                0,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot_flags | last,
            ),
            delta_row(
                1,
                event_time + 1,
                DeltaAction::Add,
                DeltaSide::Buy.as_str(),
                "0.49",
                "10",
                snapshot_flags | last,
            ),
        ];

        table
            .validate()
            .expect("a snapshot after an established empty book may omit CLEAR");
    }

    #[test]
    fn deltas_validate_rejects_empty_table() {
        let mut table = snapshot_table();
        table.rows.clear();
        let error = table.validate().expect_err("empty table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical order book deltas table is empty"),
            "{error}"
        );
    }

    #[test]
    fn deltas_validate_rejects_wrong_fidelity_class() {
        let mut table = snapshot_table();
        table.fidelity_class = SourceProofFidelityClass::TradeReplay;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("L2_REPLAY"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_empty_forbidden_claims() {
        let mut table = snapshot_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_non_dense_sequence() {
        let mut table = snapshot_table();
        table.rows[2].sequence = 9;
        let error = table.validate().expect_err("sparse sequence rejected");
        assert!(error.to_string().contains("dense ascending"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_decreasing_event_time() {
        let mut table = snapshot_table();
        table.rows[2].event_time = table.rows[1].event_time - 1;
        let error = table
            .validate()
            .expect_err("decreasing event_time rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_clear_with_non_empty_payload() {
        let mut table = snapshot_table();
        table.rows[0].price = "0.50".to_string();
        let error = table.validate().expect_err("clear with price rejected");
        assert!(
            error
                .to_string()
                .contains("CLEAR row must have empty price"),
            "{error}"
        );
    }

    #[test]
    fn deltas_validate_rejects_clear_missing_snapshot_flag() {
        let mut table = snapshot_table();
        table.rows[0].flags = RecordFlag::F_MBP as u8;
        let error = table
            .validate()
            .expect_err("clear missing F_SNAPSHOT rejected");
        assert!(
            error.to_string().contains("must contain F_SNAPSHOT"),
            "{error}"
        );
    }

    #[test]
    fn deltas_validate_rejects_snapshot_payload_missing_snapshot_flag() {
        let mut table = snapshot_table();
        table.rows[1].flags = RecordFlag::F_MBP as u8;
        let error = table
            .validate()
            .expect_err("every row in a snapshot event must carry F_SNAPSHOT");
        assert!(error.to_string().contains("F_SNAPSHOT"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_non_add_snapshot_payload() {
        let mut table = snapshot_table();
        table.rows[1].action = DeltaAction::Update.as_str().to_string();

        let error = table
            .validate()
            .expect_err("snapshot payloads must use ADD semantics");

        assert!(error.to_string().contains("must use ADD"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_incremental_event_with_snapshot_flag() {
        let mut table = snapshot_table();
        table.rows.push(delta_row(
            3,
            table.rows[2].event_time + 1,
            DeltaAction::Update,
            DeltaSide::Buy.as_str(),
            "0.48",
            "5",
            RecordFlag::F_MBP as u8 | RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_LAST as u8,
        ));
        let error = table
            .validate()
            .expect_err("incremental events must not claim snapshot semantics");
        assert!(error.to_string().contains("F_SNAPSHOT"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_snapshot_rows_missing_mbp() {
        // Native NT helpers do not always add F_MBP, but this canonical table
        // specifically claims full-depth L2/MBP evidence and must retain that
        // semantic marker on every row before entering the catalog bridge.
        let snapshot = RecordFlag::F_SNAPSHOT as u8;
        let last = RecordFlag::F_LAST as u8;
        let event_time = 1_700_000_000_000_000_000;
        let rows = vec![
            delta_row(0, event_time, DeltaAction::Clear, "", "", "", snapshot),
            delta_row(
                1,
                event_time,
                DeltaAction::Add,
                DeltaSide::Buy.as_str(),
                "0.49",
                "10",
                snapshot,
            ),
            delta_row(
                2,
                event_time,
                DeltaAction::Add,
                DeltaSide::Sell.as_str(),
                "0.51",
                "12",
                snapshot | last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        let error = table
            .validate()
            .expect_err("snapshot rows missing F_MBP must be rejected");
        assert!(error.to_string().contains("F_MBP"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_event_time_change_before_f_last() {
        let mut table = snapshot_table();
        table.rows[1].event_time += 1;
        table.rows[2].event_time += 1;

        let error = table
            .validate()
            .expect_err("one source event cannot carry multiple event times");

        assert!(error.to_string().contains("event_time"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_availability_time_change_before_f_last() {
        let mut table = snapshot_table();
        table.rows[0].availability_time = Some(table.rows[0].event_time);

        let error = table
            .validate()
            .expect_err("one source event cannot carry multiple availability times");

        assert!(error.to_string().contains("availability_time"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_native_sequence_change_before_f_last() {
        let mut table = snapshot_table();
        for row in &mut table.rows {
            row.source_sequence = Some("77".to_string());
        }
        table.rows[1].source_sequence = Some("78".to_string());

        let error = table
            .validate()
            .expect_err("one source event cannot carry multiple native sequences");

        assert!(error.to_string().contains("source_sequence"), "{error}");
    }

    #[test]
    fn deltas_validate_accepts_consecutive_closed_empty_snapshot_events() {
        let snapshot = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let last = RecordFlag::F_LAST as u8;
        let event_time = 1_700_000_000_000_000_000;
        let rows = vec![
            delta_row(
                0,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot | last,
            ),
            delta_row(
                1,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot | last,
            ),
            delta_row(
                2,
                event_time,
                DeltaAction::Update,
                DeltaSide::Buy.as_str(),
                "0.48",
                "5",
                RecordFlag::F_MBP as u8 | last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        table
            .validate()
            .expect("distinct closed empty snapshot events remain replayable");
    }

    #[test]
    fn deltas_validate_rejects_add_with_empty_side() {
        let mut table = snapshot_table();
        table.rows[1].side = String::new();
        let error = table.validate().expect_err("add with empty side rejected");
        assert!(error.to_string().contains("must be BUY or SELL"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_add_with_non_positive_size() {
        let mut table = snapshot_table();
        table.rows[1].size = "0".to_string();
        let error = table.validate().expect_err("add with zero size rejected");
        assert!(error.to_string().contains("positive size"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_snapshot_missing_f_last() {
        let mut table = snapshot_table();
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        table.rows[2].flags = snapshot_flags;
        let error = table
            .validate()
            .expect_err("snapshot missing F_LAST rejected");
        assert!(error.to_string().contains("F_LAST"), "{error}");
    }

    #[test]
    fn deltas_validate_rejects_standalone_delta_missing_f_last() {
        // A single-level UPDATE that is not part of a snapshot expansion must
        // carry F_LAST (it both opens and closes its own event).
        let mbp = RecordFlag::F_MBP as u8;
        let rows = vec![delta_row(
            0,
            1_700_000_000_000_000_000,
            DeltaAction::Update,
            DeltaSide::Buy.as_str(),
            "0.48",
            "5",
            mbp,
        )];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        let error = table
            .validate()
            .expect_err("standalone delta missing F_LAST rejected");
        assert!(error.to_string().contains("F_LAST"), "{error}");
    }

    // Fix 2 — DELETE branch coverage: validation accepts a well-formed DELETE
    // row (non-empty side/price/size, no positive-size requirement) and the
    // reject tests below confirm the branch is fully reachable.
    #[test]
    fn deltas_validate_accepts_delete_row() {
        // A DELETE carries side/price/size but the validator intentionally skips
        // the positive-size check (level-removal may carry size 0).
        let last = RecordFlag::F_LAST as u8;
        let mbp = RecordFlag::F_MBP as u8;
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let event_time = 1_700_000_000_000_000_000;
        // Build a minimal valid table: snapshot (Clear+Add) then a DELETE.
        let rows = vec![
            delta_row(
                0,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot_flags,
            ),
            delta_row(
                1,
                event_time,
                DeltaAction::Add,
                DeltaSide::Sell.as_str(),
                "0.51",
                "10",
                snapshot_flags | last,
            ),
            delta_row(
                2,
                event_time + 1,
                DeltaAction::Delete,
                DeltaSide::Sell.as_str(),
                "0.51",
                "10",
                mbp | last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        table
            .validate()
            .expect("well-formed DELETE row must be accepted");
    }

    #[test]
    fn deltas_validate_accepts_delete_row_with_zero_size() {
        // DELETE is the only action where size 0 is valid (level-removal
        // carrying no residual quantity). This validates the positive-size
        // carve-out at validate_delta_action_payload line ~291.
        let last = RecordFlag::F_LAST as u8;
        let mbp = RecordFlag::F_MBP as u8;
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let event_time = 1_700_000_000_000_000_000;
        let rows = vec![
            delta_row(
                0,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot_flags,
            ),
            delta_row(
                1,
                event_time,
                DeltaAction::Add,
                DeltaSide::Buy.as_str(),
                "0.49",
                "10",
                snapshot_flags | last,
            ),
            delta_row(
                2,
                event_time + 1,
                DeltaAction::Delete,
                DeltaSide::Buy.as_str(),
                "0.49",
                "0",
                mbp | last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        table
            .validate()
            .expect("DELETE with size 0 must be accepted (level-removal)");
    }

    // Negative test for the "CLEAR may only begin a book event" rule.
    #[test]
    fn deltas_validate_rejects_mid_event_clear() {
        // Row 0: ADD without F_LAST — opens an event but does not close it.
        // Row 1: CLEAR — appears mid-event; predecessor is not event-closing.
        // This shape must be rejected with the "CLEAR may only begin a book
        // event" message.
        let mbp = RecordFlag::F_MBP as u8;
        let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
        let last = RecordFlag::F_LAST as u8;
        let event_time = 1_700_000_000_000_000_000;
        let rows = vec![
            // An ADD that does NOT carry F_LAST — event remains open.
            delta_row(
                0,
                event_time,
                DeltaAction::Add,
                DeltaSide::Buy.as_str(),
                "0.49",
                "10",
                mbp, // no F_LAST intentionally
            ),
            // CLEAR mid-event: at_event_start is false here.
            delta_row(
                1,
                event_time,
                DeltaAction::Clear,
                "",
                "",
                "",
                snapshot_flags | last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        let error = table
            .validate()
            .expect_err("mid-event CLEAR must be rejected");
        assert!(
            error
                .to_string()
                .contains("CLEAR may only begin a book event"),
            "expected mid-event CLEAR rejection; got: {error}"
        );
    }

    fn bar_row(
        open_time: i64,
        open: &str,
        high: &str,
        low: &str,
        close: &str,
        volume: &str,
    ) -> CanonicalBarRow {
        CanonicalBarRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "testvenue/prediction-market/YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            open_time,
            close_time: open_time + 60_000_000_000,
            capture_time: open_time + 60_000_000_000,
            availability_time: None,
            source_sequence: Some(open_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            open: open.to_string(),
            high: high.to_string(),
            low: low.to_string(),
            close: close.to_string(),
            volume: volume.to_string(),
        }
    }

    fn bars_table() -> CanonicalBarsTable {
        let base = 1_700_000_000_000_000_000;
        let rows = vec![
            bar_row(base, "0.50", "0.55", "0.49", "0.52", "100"),
            bar_row(base + 60_000_000_000, "0.52", "0.58", "0.51", "0.57", "120"),
        ];
        CanonicalBarsTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: partition(),
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            bar_spec: CanonicalBarSpec {
                step: 1,
                aggregation: BarAggregation::Minute,
            },
            rows,
        }
    }

    #[test]
    fn bars_validate_accepts_well_formed_table() {
        bars_table()
            .validate()
            .expect("well-formed bar table is valid");
    }

    #[test]
    fn bars_validate_rejects_empty_table() {
        let mut table = bars_table();
        table.rows.clear();
        let error = table.validate().expect_err("empty bar table rejected");
        assert!(
            error.to_string().contains("canonical bars table is empty"),
            "{error}"
        );
    }

    #[test]
    fn bars_validate_rejects_zero_step() {
        let mut table = bars_table();
        table.bar_spec.step = 0;
        let error = table.validate().expect_err("zero step rejected");
        assert!(
            error.to_string().contains("bar step must be positive"),
            "{error}"
        );
    }

    #[test]
    fn bars_validate_rejects_wrong_fidelity_class() {
        let mut table = bars_table();
        table.fidelity_class = SourceProofFidelityClass::TradeReplay;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("TRADE_BAR_REPLAY"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_empty_forbidden_claims() {
        let mut table = bars_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_high_below_open() {
        let mut table = bars_table();
        table.rows[0].high = "0.40".to_string();
        let error = table.validate().expect_err("high below open rejected");
        assert!(error.to_string().contains("high"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_low_above_close() {
        // Construct a bearish bar where open > close so that a low between
        // open and close satisfies low<=open but violates low<=close.  This
        // ensures the low<=close ensure fires — not the preceding low<=open or
        // high>=low ensures — so the test is discriminating for that specific
        // rule.
        //
        // Values: open=0.55, high=0.60, close=0.50, low=0.53.
        //   high>=open  : 0.60 >= 0.55  ✓
        //   high>=low   : 0.60 >= 0.53  ✓
        //   high>=close : 0.60 >= 0.50  ✓
        //   low<=open   : 0.53 <= 0.55  ✓
        //   low<=close  : 0.53 <= 0.50  ✗  ← the only failing rule
        let mut table = bars_table();
        table.rows[0].open = "0.55".to_string();
        table.rows[0].high = "0.60".to_string();
        table.rows[0].close = "0.50".to_string();
        table.rows[0].low = "0.53".to_string();
        let error = table.validate().expect_err("low above close rejected");
        assert!(
            error.to_string().contains("low") && error.to_string().contains("above close"),
            "expected low-above-close rejection; got: {error}"
        );
    }

    #[test]
    fn bars_validate_rejects_non_increasing_open_time() {
        let mut table = bars_table();
        table.rows[1].open_time = table.rows[0].open_time;
        let error = table
            .validate()
            .expect_err("non-increasing open_time rejected");
        assert!(error.to_string().contains("strictly increase"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_close_time_before_open_time() {
        let mut table = bars_table();
        table.rows[0].close_time = table.rows[0].open_time - 1;
        let error = table.validate().expect_err("close before open rejected");
        assert!(error.to_string().contains("precedes open_time"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_close_time_regression() {
        // The catalog write orders bars by ts_init (= close_time); a close
        // time that steps backwards across rows must fail at the canonical
        // boundary, not at the write step.
        let mut table = bars_table();
        table.rows[0].close_time = table.rows[1].close_time + 1;
        let error = table
            .validate()
            .expect_err("close_time regression rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_non_periodic_bar_step() {
        // NautilusTrader requires minute steps to divide the hour; the
        // canonical table owns spec admissibility, so the violation must
        // surface at validate(), not at catalog projection.
        let mut table = bars_table();
        table.bar_spec.step = 7;
        let error = table.validate().expect_err("non-periodic step rejected");
        assert!(error.to_string().contains("not a valid"), "{error}");
    }

    #[test]
    fn bars_validate_rejects_negative_volume() {
        let mut table = bars_table();
        table.rows[0].volume = "-1".to_string();
        let error = table.validate().expect_err("negative volume rejected");
        assert!(error.to_string().contains("negative volume"), "{error}");
    }

    fn quote_row(
        event_time: i64,
        bid: &str,
        ask: &str,
        bid_size: &str,
        ask_size: &str,
    ) -> CanonicalQuoteRow {
        CanonicalQuoteRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "testvenue/prediction-market/YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            event_time,
            capture_time: event_time,
            availability_time: None,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            bid: bid.to_string(),
            ask: ask.to_string(),
            bid_size: bid_size.to_string(),
            ask_size: ask_size.to_string(),
        }
    }

    fn quotes_table() -> CanonicalQuotesTable {
        let base = 1_700_000_000_000_000_000;
        let rows = vec![
            quote_row(base, "0.49", "0.51", "10", "12"),
            quote_row(base + 1, "0.50", "0.52", "8", "0"),
        ];
        CanonicalQuotesTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: partition(),
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::QuoteReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn quotes_validate_accepts_well_formed() {
        quotes_table()
            .validate()
            .expect("well-formed quote table is valid");
    }

    #[test]
    fn quotes_validate_rejects_empty_table() {
        let mut table = quotes_table();
        table.rows.clear();
        let error = table.validate().expect_err("empty quote table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical quotes table is empty"),
            "{error}"
        );
    }

    #[test]
    fn quotes_validate_rejects_wrong_fidelity_class() {
        let mut table = quotes_table();
        table.fidelity_class = SourceProofFidelityClass::L2Replay;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("QUOTE_REPLAY"), "{error}");
    }

    #[test]
    fn quotes_validate_rejects_empty_forbidden_claims() {
        let mut table = quotes_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn quotes_validate_rejects_crossed_book() {
        // ask below bid is a crossed top-of-book and must be rejected.
        let mut table = quotes_table();
        table.rows[0].ask = "0.40".to_string();
        let error = table.validate().expect_err("crossed book rejected");
        assert!(
            error.to_string().contains("ask") && error.to_string().contains("below bid"),
            "expected crossed-book rejection; got: {error}"
        );
    }

    #[test]
    fn quotes_validate_rejects_non_positive_bid() {
        let mut table = quotes_table();
        table.rows[0].bid = "0".to_string();
        let error = table.validate().expect_err("non-positive bid rejected");
        assert!(error.to_string().contains("non-positive bid"), "{error}");
    }

    #[test]
    fn quotes_validate_rejects_decreasing_event_time() {
        let mut table = quotes_table();
        table.rows[1].event_time = table.rows[0].event_time - 1;
        let error = table
            .validate()
            .expect_err("decreasing event_time rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    fn index_row(event_time: i64, value: &str) -> CanonicalIndexPriceRow {
        CanonicalIndexPriceRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "testvenue/prediction-market/YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            event_time,
            capture_time: event_time,
            availability_time: None,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    fn index_prices_table() -> CanonicalIndexPricesTable {
        let base = 1_700_000_000_000_000_000;
        let rows = vec![index_row(base, "0.50"), index_row(base + 1, "0.51")];
        CanonicalIndexPricesTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: partition(),
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::IndexReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn index_prices_validate_accepts_well_formed_table() {
        index_prices_table()
            .validate()
            .expect("well-formed index price table is valid");
    }

    #[test]
    fn index_prices_validate_rejects_empty_table() {
        let mut table = index_prices_table();
        table.rows.clear();
        let error = table
            .validate()
            .expect_err("empty index price table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical index prices table is empty"),
            "{error}"
        );
    }

    #[test]
    fn index_prices_validate_rejects_wrong_fidelity_class() {
        let mut table = index_prices_table();
        table.fidelity_class = SourceProofFidelityClass::SignalOnly;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("INDEX_REPLAY"), "{error}");
    }

    #[test]
    fn index_prices_validate_rejects_empty_forbidden_claims() {
        let mut table = index_prices_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn index_prices_validate_rejects_non_positive_event_time() {
        let mut table = index_prices_table();
        table.rows[0].event_time = 0;
        let error = table
            .validate()
            .expect_err("non-positive event_time rejected");
        assert!(
            error.to_string().contains("non-positive event_time"),
            "{error}"
        );
    }

    #[test]
    fn index_prices_validate_rejects_decreasing_event_time() {
        let mut table = index_prices_table();
        table.rows[1].event_time = table.rows[0].event_time - 1;
        let error = table
            .validate()
            .expect_err("decreasing event_time rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    #[test]
    fn index_prices_validate_rejects_unparseable_value() {
        let mut table = index_prices_table();
        table.rows[0].value = "not-a-decimal".to_string();
        let error = table.validate().expect_err("unparseable value rejected");
        assert!(error.to_string().contains("invalid value"), "{error}");
    }

    #[test]
    fn index_prices_validate_rejects_non_positive_value() {
        let mut table = index_prices_table();
        table.rows[0].value = "0".to_string();
        let error = table.validate().expect_err("non-positive value rejected");
        assert!(error.to_string().contains("non-positive value"), "{error}");
    }

    fn mark_row(event_time: i64, value: &str) -> CanonicalMarkPriceRow {
        CanonicalMarkPriceRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            instrument_id: "YES".to_string(),
            canonical_instrument_key: "testvenue/prediction-market/YES".to_string(),
            venue_symbol: "YES".to_string(),
            nt_instrument_id: Some("YES.TESTVENUE".to_string()),
            event_time,
            capture_time: event_time,
            availability_time: None,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            value: value.to_string(),
        }
    }

    fn mark_prices_table() -> CanonicalMarkPricesTable {
        let base = 1_700_000_000_000_000_000;
        let rows = vec![mark_row(base, "0.50"), mark_row(base + 1, "0.51")];
        CanonicalMarkPricesTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: partition(),
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::MarkReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn mark_prices_validate_accepts_well_formed_table() {
        mark_prices_table()
            .validate()
            .expect("well-formed mark price table is valid");
    }

    #[test]
    fn mark_prices_validate_rejects_empty_table() {
        let mut table = mark_prices_table();
        table.rows.clear();
        let error = table
            .validate()
            .expect_err("empty mark price table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical mark prices table is empty"),
            "{error}"
        );
    }

    #[test]
    fn mark_prices_validate_rejects_wrong_fidelity_class() {
        let mut table = mark_prices_table();
        table.fidelity_class = SourceProofFidelityClass::SignalOnly;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("MARK_REPLAY"), "{error}");
    }

    #[test]
    fn mark_prices_validate_rejects_empty_forbidden_claims() {
        let mut table = mark_prices_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn mark_prices_validate_rejects_non_positive_event_time() {
        let mut table = mark_prices_table();
        table.rows[0].event_time = 0;
        let error = table
            .validate()
            .expect_err("non-positive event_time rejected");
        assert!(
            error.to_string().contains("non-positive event_time"),
            "{error}"
        );
    }

    #[test]
    fn mark_prices_validate_rejects_decreasing_event_time() {
        let mut table = mark_prices_table();
        table.rows[1].event_time = table.rows[0].event_time - 1;
        let error = table
            .validate()
            .expect_err("decreasing event_time rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    #[test]
    fn mark_prices_validate_rejects_unparseable_value() {
        let mut table = mark_prices_table();
        table.rows[0].value = "not-a-decimal".to_string();
        let error = table.validate().expect_err("unparseable value rejected");
        assert!(error.to_string().contains("invalid value"), "{error}");
    }

    #[test]
    fn mark_prices_validate_rejects_non_positive_value() {
        let mut table = mark_prices_table();
        table.rows[0].value = "0".to_string();
        let error = table.validate().expect_err("non-positive value rejected");
        assert!(error.to_string().contains("non-positive value"), "{error}");
    }

    fn funding_rate_row(
        event_time: i64,
        rate: &str,
        interval_minutes: Option<u16>,
        next_funding_time: Option<i64>,
    ) -> CanonicalFundingRateRow {
        CanonicalFundingRateRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: "ingest-run-test".to_string(),
            source_binding: "synthetic-archive".to_string(),
            venue: "testvenue".to_string(),
            product_family: "perpetual".to_string(),
            product_category: "linear-perp".to_string(),
            instrument_id: "BTCUSDT".to_string(),
            canonical_instrument_key: "testvenue/perpetual/BTCUSDT".to_string(),
            venue_symbol: "BTCUSDT".to_string(),
            nt_instrument_id: Some("BTCUSDT.TESTVENUE".to_string()),
            event_time,
            capture_time: event_time,
            availability_time: None,
            source_sequence: Some(event_time.to_string()),
            raw_payload_id: "feedface".to_string(),
            source_proof_id: "source-proof-synthetic".to_string(),
            payload_hash: "feedface".to_string(),
            transform_hash: "0badc0de".to_string(),
            rate: rate.to_string(),
            interval_minutes,
            next_funding_time,
        }
    }

    fn funding_rates_table() -> CanonicalFundingRatesTable {
        let base = 1_700_000_000_000_000_000;
        let rows = vec![
            funding_rate_row(
                base,
                "-0.000100",
                Some(480),
                Some(base + 28_800_000_000_000),
            ),
            funding_rate_row(base + 1, "0", Some(480), Some(base + 28_800_000_000_000)),
        ];
        CanonicalFundingRatesTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: TradesPartition {
                venue: "testvenue".to_string(),
                product_family: "perpetual".to_string(),
                product_category: "linear-perp".to_string(),
                instrument_id: "BTCUSDT".to_string(),
                dt: "2026-05-22".to_string(),
            },
            source_proof_id: "source-proof-synthetic".to_string(),
            source_proof_version: 1,
            fidelity_class: SourceProofFidelityClass::FundingReplay,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            transform_hash: "0badc0de".to_string(),
            payload_hash: "feedface".to_string(),
            rows,
        }
    }

    #[test]
    fn funding_rates_validate_accepts_well_formed_table() {
        funding_rates_table()
            .validate()
            .expect("well-formed funding rate table is valid");
    }

    #[test]
    fn funding_rates_validate_rejects_empty_table() {
        let mut table = funding_rates_table();
        table.rows.clear();
        let error = table
            .validate()
            .expect_err("empty funding rate table rejected");
        assert!(
            error
                .to_string()
                .contains("canonical funding rates table is empty"),
            "{error}"
        );
    }

    #[test]
    fn funding_rates_validate_rejects_wrong_fidelity_class() {
        let mut table = funding_rates_table();
        table.fidelity_class = SourceProofFidelityClass::SignalOnly;
        let error = table.validate().expect_err("wrong fidelity rejected");
        assert!(error.to_string().contains("FUNDING_REPLAY"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_empty_forbidden_claims() {
        let mut table = funding_rates_table();
        table.forbidden_claims.clear();
        let error = table
            .validate()
            .expect_err("empty forbidden claims rejected");
        assert!(error.to_string().contains("forbidden claims"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_non_positive_event_time() {
        let mut table = funding_rates_table();
        table.rows[0].event_time = 0;
        let error = table
            .validate()
            .expect_err("non-positive event_time rejected");
        assert!(
            error.to_string().contains("non-positive event_time"),
            "{error}"
        );
    }

    #[test]
    fn funding_rates_validate_rejects_decreasing_event_time() {
        let mut table = funding_rates_table();
        table.rows[1].event_time = table.rows[0].event_time - 1;
        let error = table
            .validate()
            .expect_err("decreasing event_time rejected");
        assert!(error.to_string().contains("precedes previous"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_partition_instrument_mismatch() {
        let mut table = funding_rates_table();
        table.rows[0].instrument_id = "ETHUSDT".to_string();
        let error = table
            .validate()
            .expect_err("partition instrument mismatch rejected");
        assert!(
            error.to_string().contains("instrument_id does not match"),
            "{error}"
        );
    }

    #[test]
    fn funding_rates_validate_rejects_non_positive_capture_time() {
        let mut table = funding_rates_table();
        table.rows[0].capture_time = 0;
        let error = table
            .validate()
            .expect_err("non-positive capture_time rejected");
        assert!(
            error.to_string().contains("non-positive capture_time"),
            "{error}"
        );
    }

    #[test]
    fn funding_rates_validate_rejects_non_positive_availability_time() {
        let mut table = funding_rates_table();
        table.rows[0].availability_time = Some(0);
        let error = table
            .validate()
            .expect_err("non-positive availability_time rejected");
        assert!(
            error.to_string().contains("non-positive availability_time"),
            "{error}"
        );
    }

    #[test]
    fn funding_rates_validate_rejects_empty_nullable_fields() {
        let mut table = funding_rates_table();
        table.rows[0].nt_instrument_id = Some(String::new());
        let error = table
            .validate()
            .expect_err("empty nt_instrument_id rejected");
        assert!(error.to_string().contains("nt_instrument_id"), "{error}");

        let mut table = funding_rates_table();
        table.rows[0].source_sequence = Some(String::new());
        let error = table
            .validate()
            .expect_err("empty source_sequence rejected");
        assert!(error.to_string().contains("source_sequence"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_unparseable_rate() {
        let mut table = funding_rates_table();
        table.rows[0].rate = "not-a-decimal".to_string();
        let error = table.validate().expect_err("unparseable rate rejected");
        assert!(error.to_string().contains("invalid rate"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_zero_interval() {
        let mut table = funding_rates_table();
        table.rows[0].interval_minutes = Some(0);
        let error = table.validate().expect_err("zero interval rejected");
        assert!(error.to_string().contains("interval"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_non_positive_next_funding_time() {
        let mut table = funding_rates_table();
        table.rows[0].next_funding_time = Some(0);
        let error = table
            .validate()
            .expect_err("non-positive next_funding_time rejected");
        assert!(error.to_string().contains("next_funding_time"), "{error}");
    }

    #[test]
    fn funding_rates_validate_rejects_next_funding_time_not_after_event_time() {
        let mut table = funding_rates_table();
        table.rows[0].next_funding_time = Some(table.rows[0].event_time);
        let error = table
            .validate()
            .expect_err("next_funding_time at event_time rejected");
        assert!(
            error.to_string().contains("not after event_time"),
            "{error}"
        );
    }
}
