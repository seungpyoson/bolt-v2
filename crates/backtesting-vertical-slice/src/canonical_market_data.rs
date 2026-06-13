//! Gate 2 — canonical normalized order-book-delta and bar tables.
//!
//! Extends the canonical normalization layer beyond native `trades`
//! ([`super::canonical_trades`]) to the two additional NautilusTrader data
//! families this slice projects: aggregated L2 order-book deltas and
//! externally-aggregated OHLCV bars. Both tables carry the same identity and
//! provenance header shape as [`super::canonical_trades::CanonicalTradesTable`]
//! and preserve the exact source price/size strings, so the catalog projection
//! in [`super::catalog_projection`] is the single bridge from accepted evidence
//! to the NautilusTrader catalog.
//!
//! These tables are produced from accepted evidence only — raw staged data never
//! reaches this module without first passing source-proof acceptance — and each
//! family binds to its own fidelity class: order-book deltas require
//! [`SourceProofFidelityClass::L2Replay`] and bars require
//! [`SourceProofFidelityClass::TradeBarReplay`].

use anyhow::{Context, Result, ensure};
use nautilus_model::{
    data::BarSpecification,
    enums::{BarAggregation, PriceType, RecordFlag},
};
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
/// (`event_time`, `action`, `side`, `price`, `size`, `order_id`, `flags`,
/// `sequence`) describe a single NautilusTrader `OrderBookDelta`.
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
    /// Dense monotonic venue sequence assigned to the delta.
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
    /// Validate required fields, fidelity class, timestamps, sequence density,
    /// and the snapshot delta-flag contract.
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
            validate_delta_action_payload(index, row, snapshot_flag)?;
        }

        validate_snapshot_f_last(&self.rows, last_flag)?;
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
            // NautilusTrader's own snapshot helpers emit CLEAR rows carrying
            // F_SNAPSHOT only; F_MBP is an informational price-level marker
            // that converters may add but the contract must not mandate.
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

/// Validate that every snapshot expansion closes with `F_LAST` and that each
/// standalone (non-snapshot) delta is self-closing.
///
/// Every book event ends with exactly one `F_LAST` row. An event starts at row
/// 0 or immediately after a row carrying `F_LAST`. A snapshot expansion is one
/// such event whose first row is a `CLEAR` (and whose final row carries
/// `F_LAST`); a single-level delta is a one-row event that carries `F_LAST` on
/// its own row. A `CLEAR` may therefore appear only at an event start, and the
/// final row of the table must close its event with `F_LAST`.
fn validate_snapshot_f_last(rows: &[CanonicalOrderBookDeltaRow], last_flag: u8) -> Result<()> {
    let mut at_event_start = true;
    let mut previous_was_clear = false;
    for (index, row) in rows.iter().enumerate() {
        let is_clear = row.action == DeltaAction::Clear.as_str();
        if is_clear {
            ensure!(
                at_event_start,
                "row {index}: CLEAR may only begin a book event (previous event not closed with F_LAST)"
            );
            // Two CLEARs in a row carry no book information, and a table that
            // OPENS with two CLEARs would make the catalog's Parquet metadata
            // pin file precision from a payload-free row, silently corrupting
            // every later price/size on read-back. Forbid the shape outright.
            ensure!(
                !previous_was_clear,
                "row {index}: consecutive CLEAR rows are not a valid book event sequence"
            );
        }
        previous_was_clear = is_clear;
        let closes_event = row.flags & last_flag != 0;
        at_event_start = closes_event;
    }
    ensure!(
        at_event_start,
        "row {}: final book event is not closed with F_LAST",
        rows.len() - 1
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
            source_sequence: Some(sequence.to_string()),
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
    fn deltas_validate_accepts_snapshot_flag_only_clear() {
        // NautilusTrader's own snapshot helpers emit CLEAR rows carrying
        // F_SNAPSHOT only (no F_MBP); the canonical contract must accept the
        // shape converters faithfully port from that helper.
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
        table
            .validate()
            .expect("snapshot-flag-only CLEAR expansion is valid");
    }

    #[test]
    fn deltas_validate_rejects_consecutive_clear_rows() {
        // A table opening with two CLEAR rows would let the catalog's Parquet
        // metadata pin file precision from a payload-free row, silently
        // corrupting every later price/size on read-back.
        let snapshot = RecordFlag::F_SNAPSHOT as u8;
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
                last,
            ),
        ];
        let table = CanonicalOrderBookDeltasTable {
            rows,
            ..snapshot_table()
        };
        let error = table
            .validate()
            .expect_err("consecutive CLEAR rows rejected");
        assert!(error.to_string().contains("consecutive CLEAR"), "{error}");
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

    // Fix 3 — negative test for the "CLEAR may only begin a book event" rule.
    // The existing deltas_validate_rejects_consecutive_clear_rows test places
    // both CLEARs at event boundaries (each carries F_LAST), so it only trips
    // the consecutive-CLEAR ensure.  This test constructs a mid-event CLEAR
    // (its predecessor did NOT carry F_LAST) to exercise the at_event_start
    // ensure on line ~324 of validate_snapshot_f_last.
    #[test]
    fn deltas_validate_rejects_mid_event_clear() {
        // Row 0: ADD without F_LAST — opens an event but does not close it.
        // Row 1: CLEAR — appears mid-event; predecessor is not event-closing.
        // This shape must be rejected with the "CLEAR may only begin a book
        // event" message, not the consecutive-CLEAR message.
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
}
