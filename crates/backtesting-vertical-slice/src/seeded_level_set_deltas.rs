//! Bounded snapshot-seeded L2 level-set archives compiled into canonical deltas
//! and event-cadence BBO quotes.

use std::{
    collections::BTreeSet,
    io::{self, Write},
    str::FromStr,
};

use anyhow::{Context, Result, bail, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::{OrderBookDelta, OrderBookDeltas},
    enums::{BookAction, BookType, OrderSide, RecordFlag},
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    canonical_market_data::{
        CanonicalOrderBookDeltaRow, CanonicalOrderBookDeltasTable, CanonicalQuoteRow,
        CanonicalQuotesTable, DeltaAction, DeltaSide, NORMALIZED_SCHEMA_VERSION,
    },
    canonical_trades::{
        CanonicalInstrumentIdentity, CsvTimestampUnit, RawPayloadConfig, TradesPartition,
    },
    catalog_projection::canonical_row_to_order_book_delta_at_source_precision,
    jsonl_record_stream::{JsonlScanStats, limits_from_raw_payload, visit_jsonl_records},
    source_proof::{AcceptedDataset, SourceProofFidelityClass},
};

/// Registered transform for snapshot-seeded absolute L2 level replacement.
pub const SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY: &str =
    "snapshot-seeded-level-set-to-canonical-l2.v1";

/// Version of the registered seeded level-set transform.
pub const SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION: &str = "1";

/// Source-event sequence semantics selected by converter TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceSequencePolicy {
    /// Read a venue-native numeric sequence from the configured JSON path.
    Native { path: Vec<String> },
    /// The source does not publish a native event sequence; NT receives zero.
    Unavailable,
}

/// Treatment of an optional per-level order-count field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OrderCountPolicy {
    /// The source tuple carries no order-count field.
    Absent,
    /// Validate a nonnegative integer but drop it because NT full-depth MBP
    /// deltas have no order-count column.
    ValidateNonNegativeAndDrop { index: usize },
}

/// Bounds for retained replay-window state and output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededLevelSetOutputLimits {
    pub max_levels_per_event: usize,
    pub max_active_levels_per_side: usize,
    pub max_selected_events: u64,
    pub max_selected_delta_rows: u64,
    pub max_emitted_bytes: u64,
}

/// Config-driven mapping for one seeded absolute-level L2 wire family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededLevelSetMappingConfig {
    pub record_identity_path: Vec<String>,
    pub action_path: Vec<String>,
    pub event_time_path: Vec<String>,
    pub event_time_unit: CsvTimestampUnit,
    pub bids_path: Vec<String>,
    pub asks_path: Vec<String>,
    pub level_arity: usize,
    pub level_price_index: usize,
    pub level_size_index: usize,
    pub order_count: OrderCountPolicy,
    pub snapshot_action_values: Vec<String>,
    pub update_action_values: Vec<String>,
    pub source_sequence: SourceSequencePolicy,
    pub output: SeededLevelSetOutputLimits,
}

/// Inclusive source-event window retained for replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeededLevelSetWindowBounds {
    pub start_time: Option<i64>,
    pub end_time: Option<i64>,
}

/// Canonical full-depth deltas and derived BBO for one bounded replay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededLevelSetWindow {
    pub deltas: CanonicalOrderBookDeltasTable,
    pub quotes: Option<CanonicalQuotesTable>,
    pub scan: JsonlScanStats,
    pub selected_events: u64,
    pub peak_active_levels_per_side: usize,
    pub emitted_row_bytes: u64,
    pub serialized_output_bytes: u64,
}

/// Accepted source, replay window, and immutable invocation facts for one
/// seeded level-set compilation.
pub struct SeededLevelSetCompileInput<'a> {
    pub accepted: &'a AcceptedDataset,
    pub identity: &'a CanonicalInstrumentIdentity,
    pub instrument: &'a InstrumentAny,
    pub window: SeededLevelSetWindowBounds,
    pub raw_bytes: &'a [u8],
    pub capture_time: i64,
    pub ingest_run_id: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceEventAction {
    Snapshot,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLevel {
    price: String,
    size: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceEvent {
    action: SourceEventAction,
    event_time: i64,
    source_sequence: Option<String>,
    bids: Vec<SourceLevel>,
    asks: Vec<SourceLevel>,
}

#[derive(Debug, Clone)]
struct Provenance {
    ingest_run_id: String,
    source_binding: String,
    venue: String,
    product_family: String,
    product_category: String,
    instrument_id: String,
    canonical_instrument_key: String,
    venue_symbol: String,
    nt_instrument_id: String,
    partition_dt: String,
    source_proof_id: String,
    source_proof_version: u32,
    fidelity_class: SourceProofFidelityClass,
    forbidden_claims: Vec<String>,
    raw_payload_id: String,
    payload_hash: String,
    transform_hash: String,
    capture_time: i64,
}

impl Provenance {
    fn from_accepted(
        accepted: &AcceptedDataset,
        identity: &CanonicalInstrumentIdentity,
        capture_time: i64,
        ingest_run_id: &str,
    ) -> Result<Self> {
        ensure!(capture_time > 0, "capture_time must be positive");
        ensure!(
            !ingest_run_id.trim().is_empty(),
            "ingest_run_id must not be empty"
        );
        Ok(Self {
            ingest_run_id: ingest_run_id.to_string(),
            source_binding: accepted.source_binding.clone(),
            venue: accepted.venue.clone(),
            product_family: accepted.product_family.clone(),
            product_category: accepted.product_category.clone(),
            instrument_id: identity.instrument_id.clone(),
            canonical_instrument_key: format!(
                "{}/{}/{}",
                accepted.venue, accepted.product_family, identity.instrument_id
            ),
            venue_symbol: identity.venue_symbol.clone(),
            nt_instrument_id: identity.nt_instrument_id.clone(),
            partition_dt: accepted.object.archive_date.clone(),
            source_proof_id: accepted.source_proof_id.clone(),
            source_proof_version: accepted.source_proof_version,
            fidelity_class: accepted.fidelity_class,
            forbidden_claims: accepted.forbidden_claims.clone(),
            raw_payload_id: accepted.object.sha256.clone(),
            payload_hash: accepted.object.sha256.clone(),
            transform_hash: seeded_level_set_transform_hash(),
            capture_time,
        })
    }

    fn partition(&self) -> TradesPartition {
        TradesPartition {
            venue: self.venue.clone(),
            product_family: self.product_family.clone(),
            product_category: self.product_category.clone(),
            instrument_id: self.instrument_id.clone(),
            dt: self.partition_dt.clone(),
        }
    }
}

/// Lowercase SHA-256 over the registered transform identity.
#[must_use]
pub fn seeded_level_set_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// Compile a bounded event window while scanning and validating the full object.
///
/// # Errors
///
/// Fails on invalid config, identity mismatch, malformed records, non-monotonic
/// source time, update-before-seed, any configured cap, or invalid canonical/NT
/// output.
pub fn normalize_seeded_level_set_window(
    input: SeededLevelSetCompileInput<'_>,
    raw_payload: &RawPayloadConfig,
    config: &SeededLevelSetMappingConfig,
) -> Result<SeededLevelSetWindow> {
    validate_config(config, raw_payload, input.window)?;
    ensure!(
        input.raw_bytes.len() as u64 <= raw_payload.max_object_bytes,
        "raw object bytes {} exceed max_object_bytes {}",
        input.raw_bytes.len(),
        raw_payload.max_object_bytes
    );
    ensure!(
        input.instrument.id().to_string() == input.identity.nt_instrument_id,
        "NT instrument {:?} does not match accepted identity {:?}",
        input.instrument.id().to_string(),
        input.identity.nt_instrument_id
    );
    let provenance = Provenance::from_accepted(
        input.accepted,
        input.identity,
        input.capture_time,
        input.ingest_run_id,
    )?;
    let stream_limits = limits_from_raw_payload(raw_payload)?;
    let mut compiler = WindowCompiler::new(input.instrument, input.window, config, provenance);
    let scan = visit_jsonl_records(
        raw_payload.container,
        input.raw_bytes,
        &stream_limits,
        |ordinal, record| compiler.consume(ordinal, record),
    )?;
    compiler.finish(scan)
}

fn validate_config(
    config: &SeededLevelSetMappingConfig,
    raw: &RawPayloadConfig,
    window: SeededLevelSetWindowBounds,
) -> Result<()> {
    for (name, path) in [
        ("record_identity_path", &config.record_identity_path),
        ("action_path", &config.action_path),
        ("event_time_path", &config.event_time_path),
        ("bids_path", &config.bids_path),
        ("asks_path", &config.asks_path),
    ] {
        ensure!(
            !path.is_empty() && path.iter().all(|segment| !segment.trim().is_empty()),
            "{name} must contain nonempty path segments"
        );
    }
    ensure!(config.level_arity > 0, "level_arity must be positive");
    ensure!(
        config.level_price_index < config.level_arity,
        "level_price_index is outside level_arity"
    );
    ensure!(
        config.level_size_index < config.level_arity,
        "level_size_index is outside level_arity"
    );
    ensure!(
        config.level_price_index != config.level_size_index,
        "level price and size indices must differ"
    );
    let mut declared_level_indices =
        BTreeSet::from([config.level_price_index, config.level_size_index]);
    if let OrderCountPolicy::ValidateNonNegativeAndDrop { index } = config.order_count {
        ensure!(
            index < config.level_arity,
            "order-count index is outside level_arity"
        );
        ensure!(
            declared_level_indices.insert(index),
            "order-count index must differ from price and size indices"
        );
    }
    ensure!(
        declared_level_indices.len() == config.level_arity
            && (0..config.level_arity).all(|index| declared_level_indices.contains(&index)),
        "every level tuple position must have a declared meaning"
    );
    ensure!(
        !config.snapshot_action_values.is_empty(),
        "snapshot_action_values must not be empty"
    );
    ensure!(
        !config.update_action_values.is_empty(),
        "update_action_values must not be empty"
    );
    for value in config
        .snapshot_action_values
        .iter()
        .chain(&config.update_action_values)
    {
        ensure!(!value.trim().is_empty(), "action values must not be empty");
    }
    ensure!(
        !config.snapshot_action_values.iter().any(|snapshot| {
            config
                .update_action_values
                .iter()
                .any(|update| snapshot.eq_ignore_ascii_case(update))
        }),
        "snapshot and update action values must be disjoint"
    );
    if let SourceSequencePolicy::Native { path } = &config.source_sequence {
        ensure!(
            !path.is_empty() && path.iter().all(|segment| !segment.trim().is_empty()),
            "native source-sequence path must contain nonempty segments"
        );
    }
    for (name, value) in [
        (
            "output.max_selected_events",
            config.output.max_selected_events,
        ),
        (
            "output.max_selected_delta_rows",
            config.output.max_selected_delta_rows,
        ),
        ("output.max_emitted_bytes", config.output.max_emitted_bytes),
    ] {
        ensure!(value > 0, "{name} must be positive");
    }
    ensure!(
        config.output.max_levels_per_event > 0,
        "output.max_levels_per_event must be positive"
    );
    ensure!(
        config.output.max_active_levels_per_side > 0,
        "output.max_active_levels_per_side must be positive"
    );
    ensure!(
        raw.max_decoded_bytes > 0,
        "raw_payload.max_decoded_bytes must be positive"
    );
    if let Some(start) = window.start_time {
        ensure!(start > 0, "window start_time must be positive");
    }
    if let Some(end) = window.end_time {
        ensure!(end > 0, "window end_time must be positive");
    }
    if let (Some(start), Some(end)) = (window.start_time, window.end_time) {
        ensure!(start <= end, "window start_time exceeds end_time");
    }
    limits_from_raw_payload(raw).map(|_| ())
}

struct WindowCompiler<'a> {
    instrument: &'a InstrumentAny,
    window: SeededLevelSetWindowBounds,
    config: &'a SeededLevelSetMappingConfig,
    provenance: Provenance,
    book: OrderBook,
    seeded: bool,
    previous_event_time: Option<i64>,
    selected_events: u64,
    peak_active_levels_per_side: usize,
    emitted_bytes: u64,
    delta_rows: Vec<CanonicalOrderBookDeltaRow>,
    quote_rows: Vec<CanonicalQuoteRow>,
}

impl<'a> WindowCompiler<'a> {
    fn new(
        instrument: &'a InstrumentAny,
        window: SeededLevelSetWindowBounds,
        config: &'a SeededLevelSetMappingConfig,
        provenance: Provenance,
    ) -> Self {
        Self {
            instrument,
            window,
            config,
            book: OrderBook::new(instrument.id(), BookType::L2_MBP),
            provenance,
            seeded: false,
            previous_event_time: None,
            selected_events: 0,
            peak_active_levels_per_side: 0,
            emitted_bytes: 0,
            delta_rows: Vec::new(),
            quote_rows: Vec::new(),
        }
    }

    fn consume(&mut self, ordinal: u64, record: &[u8]) -> Result<()> {
        let event =
            parse_source_event(record, ordinal, self.config, &self.provenance.venue_symbol)?;
        if let Some(previous) = self.previous_event_time {
            ensure!(
                event.event_time >= previous,
                "record {ordinal}: event_time {} precedes previous {previous}",
                event.event_time
            );
        }
        self.previous_event_time = Some(event.event_time);
        if event.action == SourceEventAction::Update {
            ensure!(
                self.seeded,
                "record {ordinal}: L2 update arrived before a seeding snapshot"
            );
        }

        let selected = self.window.contains(event.event_time);
        if selected && self.delta_rows.is_empty() && event.action == SourceEventAction::Update {
            let seed = self.seed_rows(event.event_time)?;
            self.append_delta_event(seed)?;
        }

        let event_rows = source_event_rows(&self.provenance, &event)?;
        let mut candidate = self.book.clone();
        let native = event_rows
            .iter()
            .map(|row| {
                canonical_row_to_order_book_delta_at_source_precision(self.instrument.id(), row)
            })
            .collect::<Result<Vec<OrderBookDelta>>>()?;
        let batch = OrderBookDeltas::new_checked(self.instrument.id(), native)
            .context("construct atomic NT source-event delta batch")?;
        candidate
            .apply_deltas(&batch)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| format!("record {ordinal}: apply NT L2 source event"))?;
        let active_bid_levels = candidate.bids(None).count();
        let active_ask_levels = candidate.asks(None).count();
        ensure!(
            active_bid_levels <= self.config.output.max_active_levels_per_side,
            "record {ordinal}: active bid levels exceed max_active_levels_per_side {}",
            self.config.output.max_active_levels_per_side
        );
        ensure!(
            active_ask_levels <= self.config.output.max_active_levels_per_side,
            "record {ordinal}: active ask levels exceed max_active_levels_per_side {}",
            self.config.output.max_active_levels_per_side
        );
        self.peak_active_levels_per_side = self
            .peak_active_levels_per_side
            .max(active_bid_levels)
            .max(active_ask_levels);
        self.book = candidate;
        if event.action == SourceEventAction::Snapshot {
            self.seeded = true;
        }

        if selected {
            self.selected_events = self
                .selected_events
                .checked_add(1)
                .context("selected event count overflow")?;
            ensure!(
                self.selected_events <= self.config.output.max_selected_events,
                "selected event count exceeds max_selected_events {}",
                self.config.output.max_selected_events
            );
            self.append_delta_event(event_rows)?;
            if let Some(quote) = self.quote_row(&event) {
                self.append_quote(quote)?;
            }
        }
        Ok(())
    }

    fn seed_rows(&self, event_time: i64) -> Result<Vec<CanonicalOrderBookDeltaRow>> {
        ensure!(
            self.seeded,
            "cannot derive a replay seed before source snapshot"
        );
        let timestamp = u64::try_from(event_time).context("negative replay seed event_time")?;
        let snapshot = self
            .book
            .to_deltas(UnixNanos::from(timestamp), UnixNanos::from(timestamp));
        snapshot
            .deltas
            .iter()
            .map(|delta| row_from_nt_snapshot(&self.provenance, event_time, delta))
            .collect()
    }

    fn append_delta_event(&mut self, rows: Vec<CanonicalOrderBookDeltaRow>) -> Result<()> {
        let next_len = self
            .delta_rows
            .len()
            .checked_add(rows.len())
            .context("selected delta row count overflow")?;
        ensure!(
            next_len as u64 <= self.config.output.max_selected_delta_rows,
            "selected delta rows exceed max_selected_delta_rows {}",
            self.config.output.max_selected_delta_rows
        );
        for mut row in rows {
            row.sequence = self.delta_rows.len() as u64;
            self.charge(&row)?;
            self.delta_rows.push(row);
        }
        Ok(())
    }

    fn quote_row(&self, event: &SourceEvent) -> Option<CanonicalQuoteRow> {
        Some(CanonicalQuoteRow {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            ingest_run_id: self.provenance.ingest_run_id.clone(),
            source_binding: self.provenance.source_binding.clone(),
            venue: self.provenance.venue.clone(),
            product_family: self.provenance.product_family.clone(),
            product_category: self.provenance.product_category.clone(),
            instrument_id: self.provenance.instrument_id.clone(),
            canonical_instrument_key: self.provenance.canonical_instrument_key.clone(),
            venue_symbol: self.provenance.venue_symbol.clone(),
            nt_instrument_id: Some(self.provenance.nt_instrument_id.clone()),
            event_time: event.event_time,
            capture_time: self.provenance.capture_time,
            availability_time: Some(event.event_time),
            source_sequence: event.source_sequence.clone(),
            raw_payload_id: self.provenance.raw_payload_id.clone(),
            source_proof_id: self.provenance.source_proof_id.clone(),
            payload_hash: self.provenance.payload_hash.clone(),
            transform_hash: self.provenance.transform_hash.clone(),
            bid: self.book.best_bid_price()?.to_string(),
            ask: self.book.best_ask_price()?.to_string(),
            bid_size: self.book.best_bid_size()?.to_string(),
            ask_size: self.book.best_ask_size()?.to_string(),
        })
    }

    fn append_quote(&mut self, quote: CanonicalQuoteRow) -> Result<()> {
        self.charge(&quote)?;
        self.quote_rows.push(quote);
        Ok(())
    }

    fn charge<T: Serialize>(&mut self, value: &T) -> Result<()> {
        let remaining = self
            .config
            .output
            .max_emitted_bytes
            .checked_sub(self.emitted_bytes)
            .context("emitted byte count exceeds configured maximum")?;
        let mut counter = BoundedCounter::new(remaining);
        crate::reference_artifact::write_canonical_json(&mut counter, value)
            .context("measure emitted canonical row")?;
        self.emitted_bytes = self
            .emitted_bytes
            .checked_add(counter.written)
            .context("emitted byte count overflow")?;
        ensure!(
            self.emitted_bytes <= self.config.output.max_emitted_bytes,
            "canonical output exceeds max_emitted_bytes {}",
            self.config.output.max_emitted_bytes
        );
        Ok(())
    }

    fn finish(self, scan: JsonlScanStats) -> Result<SeededLevelSetWindow> {
        ensure!(self.seeded, "seeded level-set stream contains no snapshot");
        ensure!(
            self.selected_events > 0,
            "seeded level-set stream contains no source event in the selected window"
        );
        ensure!(
            !self.delta_rows.is_empty(),
            "selected L2 delta output is empty"
        );
        let deltas = CanonicalOrderBookDeltasTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: self.provenance.partition(),
            source_proof_id: self.provenance.source_proof_id.clone(),
            source_proof_version: self.provenance.source_proof_version,
            fidelity_class: self.provenance.fidelity_class,
            forbidden_claims: self.provenance.forbidden_claims.clone(),
            transform_hash: self.provenance.transform_hash.clone(),
            payload_hash: self.provenance.payload_hash.clone(),
            rows: self.delta_rows,
        };
        let quotes = if self.quote_rows.is_empty() {
            None
        } else {
            Some(CanonicalQuotesTable {
                schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
                partition: self.provenance.partition(),
                source_proof_id: self.provenance.source_proof_id.clone(),
                source_proof_version: self.provenance.source_proof_version,
                fidelity_class: SourceProofFidelityClass::QuoteReplay,
                forbidden_claims: self.provenance.forbidden_claims.clone(),
                transform_hash: self.provenance.transform_hash,
                payload_hash: self.provenance.payload_hash,
                rows: self.quote_rows,
            })
        };
        let serialized_output_bytes = verify_serialized_output_bound(
            &deltas,
            quotes.as_ref(),
            self.config.output.max_emitted_bytes,
        )?;
        deltas.validate()?;
        if let Some(quotes) = &quotes {
            quotes.validate()?;
        }
        Ok(SeededLevelSetWindow {
            deltas,
            quotes,
            scan,
            selected_events: self.selected_events,
            peak_active_levels_per_side: self.peak_active_levels_per_side,
            emitted_row_bytes: self.emitted_bytes,
            serialized_output_bytes,
        })
    }
}

fn verify_serialized_output_bound(
    deltas: &CanonicalOrderBookDeltasTable,
    quotes: Option<&CanonicalQuotesTable>,
    max_bytes: u64,
) -> Result<u64> {
    let mut counter = BoundedCounter::new(max_bytes);
    crate::reference_artifact::write_canonical_json(&mut counter, deltas)
        .context("measure canonical delta table")?;
    if let Some(quotes) = quotes {
        crate::reference_artifact::write_canonical_json(&mut counter, quotes)
            .context("measure canonical quote table")?;
    }
    Ok(counter.written)
}

struct BoundedCounter {
    max_bytes: u64,
    written: u64,
}

impl BoundedCounter {
    fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            written: 0,
        }
    }
}

impl Write for BoundedCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let next = self
            .written
            .checked_add(buffer.len() as u64)
            .ok_or_else(|| io::Error::other("canonical output byte count overflow"))?;
        if next > self.max_bytes {
            return Err(io::Error::other(format!(
                "canonical output exceeds max_emitted_bytes {}",
                self.max_bytes
            )));
        }
        self.written = next;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl SeededLevelSetWindowBounds {
    fn contains(self, event_time: i64) -> bool {
        self.start_time.is_none_or(|start| event_time >= start)
            && self.end_time.is_none_or(|end| event_time <= end)
    }
}

fn parse_source_event(
    record: &[u8],
    ordinal: u64,
    config: &SeededLevelSetMappingConfig,
    expected_identity: &str,
) -> Result<SourceEvent> {
    let value: Value = serde_json::from_slice(record)
        .with_context(|| format!("record {ordinal}: parse JSON object"))?;
    let identity = required_scalar_at_path(&value, &config.record_identity_path)
        .with_context(|| format!("record {ordinal}: read instrument identity"))?;
    ensure!(
        identity == expected_identity,
        "record {ordinal}: source identity {identity:?} does not match accepted venue_symbol {expected_identity:?}"
    );
    let action_raw = required_scalar_at_path(&value, &config.action_path)
        .with_context(|| format!("record {ordinal}: read action"))?;
    let action = parse_action(config, &action_raw)
        .with_context(|| format!("record {ordinal}: invalid action {action_raw:?}"))?;
    let time_raw = required_scalar_at_path(&value, &config.event_time_path)
        .with_context(|| format!("record {ordinal}: read event time"))?;
    let event_time = config
        .event_time_unit
        .parse_to_nanos(&time_raw)
        .with_context(|| format!("record {ordinal}: invalid event time {time_raw:?}"))?;
    ensure!(event_time > 0, "record {ordinal}: non-positive event time");
    let source_sequence = match &config.source_sequence {
        SourceSequencePolicy::Native { path } => {
            let raw = required_scalar_at_path(&value, path)
                .with_context(|| format!("record {ordinal}: read native sequence"))?;
            raw.parse::<u64>()
                .with_context(|| format!("record {ordinal}: native sequence is not u64 {raw:?}"))?;
            Some(raw)
        }
        SourceSequencePolicy::Unavailable => None,
    };
    let bids = levels_at_path(&value, &config.bids_path, config, action, ordinal, "bids")?;
    let asks = levels_at_path(&value, &config.asks_path, config, action, ordinal, "asks")?;
    let total = bids
        .len()
        .checked_add(asks.len())
        .context("source-event level count overflow")?;
    ensure!(
        total <= config.output.max_levels_per_event,
        "record {ordinal}: source event has {total} levels, exceeding max_levels_per_event {}",
        config.output.max_levels_per_event
    );
    ensure!(
        action == SourceEventAction::Snapshot || total > 0,
        "record {ordinal}: empty incremental update has no NT representation"
    );
    Ok(SourceEvent {
        action,
        event_time,
        source_sequence,
        bids,
        asks,
    })
}

fn parse_action(config: &SeededLevelSetMappingConfig, raw: &str) -> Result<SourceEventAction> {
    if config
        .snapshot_action_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw.trim()))
    {
        return Ok(SourceEventAction::Snapshot);
    }
    if config
        .update_action_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw.trim()))
    {
        return Ok(SourceEventAction::Update);
    }
    bail!("action does not match configured snapshot/update values")
}

fn levels_at_path(
    value: &Value,
    path: &[String],
    config: &SeededLevelSetMappingConfig,
    action: SourceEventAction,
    ordinal: u64,
    side: &str,
) -> Result<Vec<SourceLevel>> {
    let levels = value_at_path(value, path)
        .with_context(|| format!("record {ordinal}: missing {side} path {}", path.join(".")))?
        .as_array()
        .with_context(|| format!("record {ordinal}: {side} path is not an array"))?;
    let mut prices = BTreeSet::new();
    let mut parsed = Vec::with_capacity(levels.len());
    for (index, level) in levels.iter().enumerate() {
        let fields = level
            .as_array()
            .with_context(|| format!("record {ordinal} {side} level {index}: expected an array"))?;
        ensure!(
            fields.len() == config.level_arity,
            "record {ordinal} {side} level {index}: expected tuple arity {}, got {}",
            config.level_arity,
            fields.len()
        );
        let price = scalar_from_index(fields, config.level_price_index)
            .with_context(|| format!("record {ordinal} {side} level {index}: read price"))?;
        let size = scalar_from_index(fields, config.level_size_index)
            .with_context(|| format!("record {ordinal} {side} level {index}: read size"))?;
        let price_decimal = Decimal::from_str(&price).with_context(|| {
            format!("record {ordinal} {side} level {index}: invalid price {price:?}")
        })?;
        let size_decimal = Decimal::from_str(&size).with_context(|| {
            format!("record {ordinal} {side} level {index}: invalid size {size:?}")
        })?;
        ensure!(
            price_decimal > Decimal::ZERO,
            "record {ordinal} {side} level {index}: price must be positive"
        );
        ensure!(
            size_decimal >= Decimal::ZERO,
            "record {ordinal} {side} level {index}: size must be nonnegative"
        );
        if action == SourceEventAction::Snapshot {
            ensure!(
                size_decimal > Decimal::ZERO,
                "record {ordinal} {side} level {index}: snapshot size must be positive"
            );
        }
        ensure!(
            prices.insert(price_decimal.normalize()),
            "record {ordinal} {side}: duplicate price {price:?} in one source event"
        );
        if let OrderCountPolicy::ValidateNonNegativeAndDrop { index: count_index } =
            config.order_count
        {
            let count = scalar_from_index(fields, count_index).with_context(|| {
                format!("record {ordinal} {side} level {index}: read order count")
            })?;
            count.parse::<u64>().with_context(|| {
                format!("record {ordinal} {side} level {index}: order count must be a nonnegative integer, got {count:?}")
            })?;
        }
        parsed.push(SourceLevel { price, size });
    }
    Ok(parsed)
}

fn source_event_rows(
    provenance: &Provenance,
    event: &SourceEvent,
) -> Result<Vec<CanonicalOrderBookDeltaRow>> {
    let mbp = RecordFlag::F_MBP as u8;
    let snapshot = RecordFlag::F_SNAPSHOT as u8;
    let last = RecordFlag::F_LAST as u8;
    let mut rows = Vec::new();
    if event.action == SourceEventAction::Snapshot {
        rows.push(delta_row(
            provenance,
            event,
            DeltaAction::Clear,
            None,
            "",
            "",
            mbp | snapshot,
        ));
    }
    for (side, levels) in [
        (DeltaSide::Buy, &event.bids),
        (DeltaSide::Sell, &event.asks),
    ] {
        for level in levels {
            let action = match event.action {
                SourceEventAction::Snapshot => DeltaAction::Add,
                SourceEventAction::Update if Decimal::from_str(&level.size)? == Decimal::ZERO => {
                    DeltaAction::Delete
                }
                SourceEventAction::Update => DeltaAction::Update,
            };
            rows.push(delta_row(
                provenance,
                event,
                action,
                Some(side),
                &level.price,
                &level.size,
                mbp | if event.action == SourceEventAction::Snapshot {
                    snapshot
                } else {
                    0
                },
            ));
        }
    }
    let final_row = rows
        .last_mut()
        .context("source event produced no delta rows")?;
    final_row.flags |= last;
    for (sequence, row) in rows.iter_mut().enumerate() {
        row.sequence = sequence as u64;
    }
    Ok(rows)
}

fn delta_row(
    provenance: &Provenance,
    event: &SourceEvent,
    action: DeltaAction,
    side: Option<DeltaSide>,
    price: &str,
    size: &str,
    flags: u8,
) -> CanonicalOrderBookDeltaRow {
    CanonicalOrderBookDeltaRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: provenance.ingest_run_id.clone(),
        source_binding: provenance.source_binding.clone(),
        venue: provenance.venue.clone(),
        product_family: provenance.product_family.clone(),
        product_category: provenance.product_category.clone(),
        instrument_id: provenance.instrument_id.clone(),
        canonical_instrument_key: provenance.canonical_instrument_key.clone(),
        venue_symbol: provenance.venue_symbol.clone(),
        nt_instrument_id: Some(provenance.nt_instrument_id.clone()),
        event_time: event.event_time,
        capture_time: provenance.capture_time,
        availability_time: Some(event.event_time),
        source_sequence: event.source_sequence.clone(),
        raw_payload_id: provenance.raw_payload_id.clone(),
        source_proof_id: provenance.source_proof_id.clone(),
        payload_hash: provenance.payload_hash.clone(),
        transform_hash: provenance.transform_hash.clone(),
        action: action.as_str().to_string(),
        side: side.map_or_else(String::new, |side| side.as_str().to_string()),
        price: price.to_string(),
        size: size.to_string(),
        order_id: 0,
        flags,
        sequence: 0,
    }
}

fn row_from_nt_snapshot(
    provenance: &Provenance,
    event_time: i64,
    delta: &OrderBookDelta,
) -> Result<CanonicalOrderBookDeltaRow> {
    let event = SourceEvent {
        action: SourceEventAction::Snapshot,
        event_time,
        source_sequence: None,
        bids: Vec::new(),
        asks: Vec::new(),
    };
    let (action, side, price, size) = match delta.action {
        BookAction::Clear => (DeltaAction::Clear, None, String::new(), String::new()),
        BookAction::Add => (
            DeltaAction::Add,
            Some(match delta.order.side {
                OrderSide::Buy => DeltaSide::Buy,
                OrderSide::Sell => DeltaSide::Sell,
                other => bail!("NT replay seed carries invalid order side {other:?}"),
            }),
            delta.order.price.to_string(),
            delta.order.size.to_string(),
        ),
        other => bail!("NT replay seed carries invalid action {other:?}"),
    };
    Ok(delta_row(
        provenance,
        &event,
        action,
        side,
        &price,
        &size,
        delta.flags | RecordFlag::F_MBP as u8,
    ))
}

fn value_at_path<'a>(value: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut current = value;
    for segment in path {
        current = current.get(segment.as_str())?;
    }
    Some(current)
}

fn required_scalar_at_path(value: &Value, path: &[String]) -> Result<String> {
    let value =
        value_at_path(value, path).with_context(|| format!("missing path {}", path.join(".")))?;
    scalar_to_string(value)
}

fn scalar_from_index(values: &[Value], index: usize) -> Result<String> {
    let value = values
        .get(index)
        .with_context(|| format!("missing array index {index}"))?;
    scalar_to_string(value)
}

fn scalar_to_string(value: &Value) -> Result<String> {
    match value {
        Value::String(value) => Ok(value.clone()),
        Value::Number(value) => Ok(value.to_string()),
        _ => bail!("value is not a string or number"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        canonical_trades::{JsonlStreamConfig, RawPayloadContainer},
        catalog_projection::{
            CatalogInstrumentSpec, SpotInstrumentSpec, build_catalog_instrument,
            canonical_rows_to_order_book_deltas, project_canonical_order_book_deltas_to_catalog,
        },
        hashing::sha256_hex,
        source_proof::synthetic_accepted_dataset_for_tests,
    };

    const BASE_MS: i64 = 1_776_816_000_000;
    const BASE_NS: i64 = BASE_MS * 1_000_000;
    const NT_INSTRUMENT_ID: &str = "BTC-USDT.TESTVENUE";

    #[test]
    fn seeded_level_set_transform_hash_is_stable() {
        assert_eq!(
            seeded_level_set_transform_hash(),
            "f729f684aa764f1e7b70753b4a38f54080ac0a0631705f38052903fe0b122272",
            "seeded level-set transform identity changed; bump its version deliberately"
        );
    }

    fn raw_payload(bytes: &[u8]) -> RawPayloadConfig {
        RawPayloadConfig {
            container: RawPayloadContainer::JsonlText,
            max_object_bytes: bytes.len() as u64,
            max_decoded_bytes: bytes.len() as u64,
            zip_member: None,
            max_member_bytes: None,
            member_suffix: None,
            jsonl_stream: Some(JsonlStreamConfig {
                max_members: 1,
                max_record_bytes: 4096,
                max_records: 8,
            }),
        }
    }

    fn output_limits() -> SeededLevelSetOutputLimits {
        SeededLevelSetOutputLimits {
            max_levels_per_event: 16,
            max_active_levels_per_side: 8,
            max_selected_events: 8,
            max_selected_delta_rows: 64,
            max_emitted_bytes: 1_000_000,
        }
    }

    fn okx_mapping() -> SeededLevelSetMappingConfig {
        SeededLevelSetMappingConfig {
            record_identity_path: vec!["instId".to_string()],
            action_path: vec!["action".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["bids".to_string()],
            asks_path: vec!["asks".to_string()],
            level_arity: 3,
            level_price_index: 0,
            level_size_index: 1,
            order_count: OrderCountPolicy::ValidateNonNegativeAndDrop { index: 2 },
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["update".to_string()],
            source_sequence: SourceSequencePolicy::Unavailable,
            output: output_limits(),
        }
    }

    fn bybit_mapping() -> SeededLevelSetMappingConfig {
        SeededLevelSetMappingConfig {
            record_identity_path: vec!["data".to_string(), "s".to_string()],
            action_path: vec!["type".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["data".to_string(), "b".to_string()],
            asks_path: vec!["data".to_string(), "a".to_string()],
            level_arity: 2,
            level_price_index: 0,
            level_size_index: 1,
            order_count: OrderCountPolicy::Absent,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["delta".to_string()],
            source_sequence: SourceSequencePolicy::Native {
                path: vec!["data".to_string(), "seq".to_string()],
            },
            output: output_limits(),
        }
    }

    fn identity() -> CanonicalInstrumentIdentity {
        CanonicalInstrumentIdentity {
            instrument_id: "BTC-USDT".to_string(),
            venue_symbol: "BTC-USDT".to_string(),
            nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
        }
    }

    fn spot_spec() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            nt_instrument_id: NT_INSTRUMENT_ID.to_string(),
            raw_symbol: "BTC-USDT".to_string(),
            base_currency: "BTC".to_string(),
            quote_currency: "USDT".to_string(),
            price_increment: "0.01".to_string(),
            size_increment: "0.001".to_string(),
            min_quantity: "0.001".to_string(),
            max_quantity: "1000000".to_string(),
            min_notional: "1".to_string(),
            max_notional: "100000000".to_string(),
        }
    }

    fn instrument() -> InstrumentAny {
        build_catalog_instrument(&CatalogInstrumentSpec::Spot(spot_spec()))
            .expect("build NT spot instrument")
    }

    fn accepted(bytes: &[u8]) -> AcceptedDataset {
        let mut accepted = synthetic_accepted_dataset_for_tests();
        let hash = sha256_hex(bytes);
        accepted.fidelity_class = SourceProofFidelityClass::L2Replay;
        accepted.table_family = "order_book_snapshot_deltas".to_string();
        accepted.object.sha256 = hash.clone();
        accepted.object.bytes = bytes.len() as u64;
        accepted.accepted_object_sha256 = hash;
        accepted
    }

    fn compile(
        bytes: &[u8],
        mapping: &SeededLevelSetMappingConfig,
        window: SeededLevelSetWindowBounds,
    ) -> Result<SeededLevelSetWindow> {
        normalize_seeded_level_set_window(
            SeededLevelSetCompileInput {
                accepted: &accepted(bytes),
                identity: &identity(),
                instrument: &instrument(),
                window,
                raw_bytes: bytes,
                capture_time: BASE_NS,
                ingest_run_id: "ingest-run",
            },
            &raw_payload(bytes),
            mapping,
        )
    }

    fn quotes(window: &SeededLevelSetWindow) -> &CanonicalQuotesTable {
        window
            .quotes
            .as_ref()
            .expect("test window should emit derived BBO quotes")
    }

    #[test]
    fn okx_shape_emits_full_depth_event_groups_and_nt_zero_sequence() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"2\"],[\"99\",\"2\",\"1\"]],\"asks\":[[\"101\",\"3\",\"4\"],[\"102\",\"4\",\"1\"]]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[[\"100\",\"0\",\"0\"]],\"asks\":[[\"101\",\"5\",\"3\"]]}}\n",
            BASE_MS + 1
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("compile OKX-shaped level-set events");

        assert_eq!(window.scan.records, 2);
        assert_eq!(window.deltas.rows.len(), 7);
        assert_eq!(window.deltas.rows[0].action, "CLEAR");
        assert_eq!(window.deltas.rows[5].action, "DELETE");
        assert_eq!(window.deltas.rows[6].action, "UPDATE");
        assert_eq!(
            window.deltas.rows[5].flags & RecordFlag::F_LAST as u8,
            0,
            "only the final row closes a multi-level source event"
        );
        assert_ne!(window.deltas.rows[6].flags & RecordFlag::F_LAST as u8, 0);
        assert!(
            window
                .deltas
                .rows
                .iter()
                .all(|row| row.flags & RecordFlag::F_MBP as u8 != 0)
        );
        assert!(
            window
                .deltas
                .rows
                .iter()
                .all(|row| row.source_sequence.is_none())
        );
        let native = canonical_rows_to_order_book_deltas(&window.deltas, &instrument())
            .expect("project canonical rows to NT deltas");
        assert!(native.iter().all(|delta| delta.sequence == 0));
        assert_eq!(quotes(&window).rows.len(), 2);
        assert_eq!(quotes(&window).rows[1].bid, "99");
        assert_eq!(quotes(&window).rows[1].ask_size, "5");
        assert_eq!(
            quotes(&window).rows[1].availability_time,
            Some((BASE_MS + 1) * 1_000_000)
        );
    }

    #[test]
    fn bybit_shape_preserves_one_native_sequence_per_source_event() {
        let input = format!(
            "{{\"type\":\"snapshot\",\"ts\":{BASE_MS},\"data\":{{\"s\":\"BTC-USDT\",\"seq\":10,\"b\":[[\"100\",\"1\"]],\"a\":[[\"101\",\"2\"]]}}}}\n{{\"type\":\"delta\",\"ts\":{},\"data\":{{\"s\":\"BTC-USDT\",\"seq\":11,\"b\":[[\"100\",\"2\"]],\"a\":[[\"101\",\"3\"]]}}}}\n",
            BASE_MS + 1
        );
        let window = compile(
            input.as_bytes(),
            &bybit_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("compile Bybit-shaped level-set events");

        assert!(
            window.deltas.rows[..3]
                .iter()
                .all(|row| row.source_sequence.as_deref() == Some("10"))
        );
        assert!(
            window.deltas.rows[3..]
                .iter()
                .all(|row| row.source_sequence.as_deref() == Some("11"))
        );
        let native = canonical_rows_to_order_book_deltas(&window.deltas, &instrument())
            .expect("project canonical rows to NT deltas");
        assert!(native[..3].iter().all(|delta| delta.sequence == 10));
        assert!(native[3..].iter().all(|delta| delta.sequence == 11));
    }

    #[test]
    fn one_sided_events_emit_no_quote_until_both_sides_exist() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n",
            BASE_MS + 1
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("compile one-sided then two-sided book");

        assert_eq!(quotes(&window).rows.len(), 1);
        assert_eq!(
            quotes(&window).rows[0].event_time,
            (BASE_MS + 1) * 1_000_000
        );
        assert_eq!(quotes(&window).rows[0].bid, "100");
        assert_eq!(quotes(&window).rows[0].ask, "101");
    }

    #[test]
    fn all_one_sided_events_preserve_primary_deltas_without_quotes() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[[\"100\",\"2\",\"1\"]],\"asks\":[]}}\n",
            BASE_MS + 1
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("compile an entirely one-sided L2 window");

        assert!(!window.deltas.rows.is_empty());
        assert!(window.quotes.is_none());
    }

    #[test]
    fn consecutive_empty_snapshots_preserve_distinct_source_events() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[],\"asks\":[]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{}\",\"bids\":[],\"asks\":[]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n",
            BASE_MS + 1,
            BASE_MS + 2
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("consecutive empty snapshots are distinct closed events");

        let clears: Vec<_> = window
            .deltas
            .rows
            .iter()
            .filter(|row| row.action == DeltaAction::Clear.as_str())
            .collect();
        assert_eq!(clears.len(), 2);
        assert_eq!(clears[0].event_time, BASE_NS);
        assert_eq!(clears[1].event_time, (BASE_MS + 1) * 1_000_000);
        assert_ne!(clears[0].flags & RecordFlag::F_LAST as u8, 0);
        assert_ne!(clears[1].flags & RecordFlag::F_LAST as u8, 0);

        let catalog = tempfile::TempDir::new().expect("catalog temp dir");
        let projection = project_canonical_order_book_deltas_to_catalog(
            &window.deltas,
            &spot_spec(),
            catalog.path(),
        )
        .expect("consecutive empty snapshots survive NT catalog projection and read-back");
        assert_eq!(projection.trade_count, window.deltas.rows.len());
    }

    #[test]
    fn selected_window_begins_with_nt_derived_seed_and_scans_through_eof() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[[\"100\",\"3\",\"1\"]],\"asks\":[]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[],\"asks\":[[\"101\",\"4\",\"1\"]]}}\n",
            BASE_MS + 1,
            BASE_MS + 2
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: Some((BASE_MS + 1) * 1_000_000),
                end_time: Some((BASE_MS + 1) * 1_000_000),
            },
        )
        .expect("compile one-event replay window");

        assert_eq!(
            window.scan.records, 3,
            "records after the window are still scanned"
        );
        assert_eq!(quotes(&window).rows.len(), 1, "derived seed emits no quote");
        assert_eq!(quotes(&window).rows[0].bid_size, "3");
        assert_eq!(window.deltas.rows[0].action, "CLEAR");
        assert_ne!(
            window.deltas.rows[0].flags & RecordFlag::F_SNAPSHOT as u8,
            0
        );
        assert_eq!(window.deltas.rows.last().unwrap().action, "UPDATE");

        let malformed_tail = format!("{input}{{not-json}}\n");
        let error = compile(
            malformed_tail.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: Some((BASE_MS + 1) * 1_000_000),
                end_time: Some((BASE_MS + 1) * 1_000_000),
            },
        )
        .expect_err("malformed records after the retained window must fail");
        assert!(error.to_string().contains("parse JSON"), "{error}");
    }

    #[test]
    fn malformed_identity_tuple_count_action_and_time_fail_closed() {
        let cases = [
            (
                format!(
                    "{{\"instId\":\"ETH-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n"
                ),
                "does not match accepted venue_symbol",
            ),
            (
                format!(
                    "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\"]],\"asks\":[]}}\n"
                ),
                "tuple arity",
            ),
            (
                format!(
                    "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"-1\"]],\"asks\":[]}}\n"
                ),
                "order count",
            ),
            (
                format!(
                    "{{\"instId\":\"BTC-USDT\",\"action\":\"unknown\",\"ts\":\"{BASE_MS}\",\"bids\":[],\"asks\":[]}}\n"
                ),
                "invalid action",
            ),
            (
                format!(
                    "{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[]}}\n"
                ),
                "before a seeding snapshot",
            ),
            (
                format!(
                    "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"2\",\"1\"]],\"asks\":[]}}\n",
                    BASE_MS + 1
                ),
                "precedes previous",
            ),
        ];
        for (input, expected) in cases {
            let error = compile(
                input.as_bytes(),
                &okx_mapping(),
                SeededLevelSetWindowBounds {
                    start_time: None,
                    end_time: None,
                },
            )
            .expect_err("malformed source event must fail");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?}: {error}"
            );
        }
    }

    #[test]
    fn active_book_and_retained_output_caps_fail_closed() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"],[\"99\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n"
        );
        let mut active = okx_mapping();
        active.output.max_active_levels_per_side = 1;
        let error = compile(
            input.as_bytes(),
            &active,
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect_err("active-book cap must fail");
        assert!(error.to_string().contains("active bid levels"), "{error}");

        let mut retained = okx_mapping();
        retained.output.max_selected_delta_rows = 1;
        let error = compile(
            input.as_bytes(),
            &retained,
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect_err("retained-row cap must fail");
        assert!(
            error.to_string().contains("max_selected_delta_rows"),
            "{error}"
        );
    }

    #[test]
    fn generated_large_source_keeps_scan_book_window_and_catalog_bounded() {
        let records = 100_000u64;
        let mut input = Vec::with_capacity(records as usize * 128);
        writeln!(
            input,
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}"
        )
        .expect("write generated snapshot");
        for ordinal in 1..records {
            writeln!(
                input,
                "{{\"instId\":\"BTC-USDT\",\"action\":\"update\",\"ts\":\"{}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[]}}",
                BASE_MS + ordinal as i64
            )
            .expect("write generated update");
        }
        let selected_time = (BASE_MS + records as i64 - 1) * 1_000_000;
        let mut mapping = okx_mapping();
        mapping.output.max_levels_per_event = 2;
        mapping.output.max_active_levels_per_side = 2;
        mapping.output.max_selected_events = 1;
        mapping.output.max_selected_delta_rows = 8;
        mapping.output.max_emitted_bytes = 1_000_000;

        let mut payload = raw_payload(&input);
        let stream = payload
            .jsonl_stream
            .as_mut()
            .expect("test JSONL stream bounds");
        stream.max_record_bytes = 512;
        stream.max_records = records;
        let window = normalize_seeded_level_set_window(
            SeededLevelSetCompileInput {
                accepted: &accepted(&input),
                identity: &identity(),
                instrument: &instrument(),
                window: SeededLevelSetWindowBounds {
                    start_time: Some(selected_time),
                    end_time: Some(selected_time),
                },
                raw_bytes: &input,
                capture_time: BASE_NS,
                ingest_run_id: "test-ingest",
            },
            &payload,
            &mapping,
        )
        .expect("compile bounded window from generated large source");

        assert_eq!(window.scan.records, records);
        assert!(window.scan.peak_record_buffer_bytes <= 513);
        assert_eq!(window.selected_events, 1);
        assert_eq!(window.peak_active_levels_per_side, 1);
        assert_eq!(window.deltas.rows.len(), 4);
        assert_eq!(quotes(&window).rows.len(), 1);
        assert!(window.emitted_row_bytes <= mapping.output.max_emitted_bytes);
        assert!(window.serialized_output_bytes <= mapping.output.max_emitted_bytes);

        let catalog = tempfile::TempDir::new().expect("catalog temp dir");
        let projection = project_canonical_order_book_deltas_to_catalog(
            &window.deltas,
            &spot_spec(),
            catalog.path(),
        )
        .expect("project bounded selected window");
        assert_eq!(projection.trade_count, window.deltas.rows.len());
        let mut catalog_bytes = 0u64;
        let mut pending = vec![catalog.path().to_path_buf()];
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(path).expect("read catalog directory") {
                let entry = entry.expect("read catalog entry");
                let metadata = entry.metadata().expect("read catalog metadata");
                if metadata.is_dir() {
                    pending.push(entry.path());
                } else {
                    catalog_bytes = catalog_bytes
                        .checked_add(metadata.len())
                        .expect("catalog byte count overflow");
                }
            }
        }
        assert!(catalog_bytes > 0);
        println!(
            "bounded_l2_evidence input_bytes={} records={} selected_events={} peak_record_buffer_bytes={} peak_active_levels_per_side={} selected_delta_rows={} emitted_row_bytes={} serialized_output_bytes={} catalog_bytes={catalog_bytes}",
            input.len(),
            window.scan.records,
            window.selected_events,
            window.scan.peak_record_buffer_bytes,
            window.peak_active_levels_per_side,
            window.deltas.rows.len(),
            window.emitted_row_bytes,
            window.serialized_output_bytes,
        );
    }

    #[test]
    fn undeclared_tuple_positions_fail_before_decode() {
        let input = format!(
            "{{\"type\":\"snapshot\",\"ts\":{BASE_MS},\"data\":{{\"s\":\"BTC-USDT\",\"seq\":10,\"b\":[[\"100\",\"1\",\"unexpected\"]],\"a\":[[\"101\",\"2\",\"unexpected\"]]}}}}\n"
        );
        let mut mapping = bybit_mapping();
        mapping.level_arity = 3;

        let error = compile(
            input.as_bytes(),
            &mapping,
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect_err("an unnamed tuple field must not be silently dropped");

        assert!(error.to_string().contains("declared meaning"), "{error}");
    }

    #[test]
    fn streaming_nt_book_preserves_source_precision_until_catalog_widening() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100.001\",\"1.0001\",\"1\"]],\"asks\":[[\"101.001\",\"2.0001\",\"1\"]]}}\n"
        );
        let window = compile(
            input.as_bytes(),
            &okx_mapping(),
            SeededLevelSetWindowBounds {
                start_time: None,
                end_time: None,
            },
        )
        .expect("source precision wider than the initial instrument spec remains lossless");

        assert_eq!(quotes(&window).rows[0].bid, "100.001");
        assert_eq!(quotes(&window).rows[0].bid_size, "1.0001");
    }

    #[test]
    fn converter_cannot_promote_a_non_l2_accepted_source() {
        let input = format!(
            "{{\"instId\":\"BTC-USDT\",\"action\":\"snapshot\",\"ts\":\"{BASE_MS}\",\"bids\":[[\"100\",\"1\",\"1\"]],\"asks\":[[\"101\",\"2\",\"1\"]]}}\n"
        );
        let accepted = synthetic_accepted_dataset_for_tests();
        assert_eq!(
            accepted.fidelity_class,
            SourceProofFidelityClass::TradeReplay
        );

        let error = normalize_seeded_level_set_window(
            SeededLevelSetCompileInput {
                accepted: &accepted,
                identity: &identity(),
                instrument: &instrument(),
                window: SeededLevelSetWindowBounds {
                    start_time: None,
                    end_time: None,
                },
                raw_bytes: input.as_bytes(),
                capture_time: BASE_NS,
                ingest_run_id: "ingest-run",
            },
            &raw_payload(input.as_bytes()),
            &okx_mapping(),
        )
        .expect_err("converter must retain and validate accepted source fidelity");

        assert!(error.to_string().contains("L2_REPLAY"), "{error}");
    }
}
