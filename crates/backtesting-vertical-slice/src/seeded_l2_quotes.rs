//! Snapshot-seeded L2 book replay to canonical top-of-book quotes.
//!
//! This adapter is intentionally stricter than a generic level-update reader:
//! it refuses to replay updates before a snapshot has seeded the book. The
//! emitted data family is `QuoteTick`, but the source rows are snapshot+delta L2
//! rows whose best bid/ask are derived by applying absolute level replacement
//! updates to the seeded book.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    canonical_market_data::{CanonicalQuoteRow, CanonicalQuotesTable, NORMALIZED_SCHEMA_VERSION},
    canonical_trades::{CanonicalInstrumentIdentity, CsvTimestampUnit, TradesPartition},
    operator_work_budget::{
        OperatorWorkBudgetGuard, OperatorWorkBudgetStage, deserialize_json_with_budget,
        for_each_nonempty_text_record_with_budget,
    },
    source_proof::{AcceptedDataset, SourceProofFidelityClass},
    tar_reader::TarMember,
};

/// Registered transform identity for snapshot-seeded L2 to top-of-book quotes.
pub const SEEDED_L2_QUOTES_TRANSFORM_IDENTITY: &str = "snapshot-seeded-l2-to-bbo-quotes.v1";

/// Registered transform version for snapshot-seeded L2 to top-of-book quotes.
pub const SEEDED_L2_QUOTES_TRANSFORM_VERSION: &str = "1";

/// Run-spec owned JSON mapping for snapshot-seeded L2 quote replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeededL2QuoteMappingConfig {
    pub action_path: Vec<String>,
    pub event_time_path: Vec<String>,
    pub event_time_unit: CsvTimestampUnit,
    pub bids_path: Vec<String>,
    pub asks_path: Vec<String>,
    pub level_price_index: usize,
    pub level_size_index: usize,
    pub snapshot_action_values: Vec<String>,
    pub update_action_values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_sequence_path: Option<Vec<String>>,
}

/// One source L2 price level in exact source decimal strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededL2QuoteLevel {
    pub price: String,
    pub size: String,
}

/// Source L2 event action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeededL2QuoteAction {
    Snapshot,
    Update,
}

/// One parsed source L2 snapshot or update event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededL2QuoteEvent {
    pub action: SeededL2QuoteAction,
    pub event_time: i64,
    pub capture_time: Option<i64>,
    pub source_sequence: Option<String>,
    pub bids: Vec<SeededL2QuoteLevel>,
    pub asks: Vec<SeededL2QuoteLevel>,
}

/// Provenance shared by every emitted canonical quote row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeededL2QuoteProvenance {
    pub ingest_run_id: String,
    pub source_binding: String,
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    pub canonical_instrument_key: String,
    pub venue_symbol: String,
    pub nt_instrument_id: Option<String>,
    pub partition_dt: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub forbidden_claims: Vec<String>,
    pub raw_payload_id: String,
    pub payload_hash: String,
    pub transform_hash: String,
    pub default_capture_time: i64,
}

impl SeededL2QuoteProvenance {
    fn from_accepted(
        accepted: &AcceptedDataset,
        identity: &CanonicalInstrumentIdentity,
        capture_time_nanos: i64,
        ingest_run_id: &str,
    ) -> Self {
        Self {
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
            nt_instrument_id: Some(identity.nt_instrument_id.clone()),
            partition_dt: accepted.object.archive_date.clone(),
            source_proof_id: accepted.source_proof_id.clone(),
            source_proof_version: accepted.source_proof_version,
            forbidden_claims: accepted.forbidden_claims.clone(),
            raw_payload_id: accepted.object.sha256.clone(),
            payload_hash: accepted.object.sha256.clone(),
            transform_hash: seeded_l2_quote_transform_hash(),
            default_capture_time: capture_time_nanos,
        }
    }
}

/// Lowercase SHA-256 hex of the seeded-L2 quote transform identity.
#[must_use]
pub fn seeded_l2_quote_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(SEEDED_L2_QUOTES_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalize one decoded JSONL source object into a canonical quote table.
///
/// # Errors
///
/// Returns an error if the mapping is invalid, the JSONL is malformed, an
/// update appears before a snapshot, or the seeded replay emits no valid BBO.
pub fn normalize_seeded_l2_jsonl_quotes(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &SeededL2QuoteMappingConfig,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalQuotesTable>> {
    normalize_seeded_l2_jsonl_quotes_with_meter(
        accepted,
        identity,
        mapping,
        jsonl_text,
        capture_time_nanos,
        ingest_run_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn normalize_seeded_l2_jsonl_quotes_with_meter(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &SeededL2QuoteMappingConfig,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<CanonicalQuotesTable>> {
    let events = parse_seeded_l2_jsonl_with_meter(mapping, jsonl_text, work_budget)?;
    let provenance = SeededL2QuoteProvenance::from_accepted(
        accepted,
        identity,
        capture_time_nanos,
        ingest_run_id,
    );
    Ok(vec![normalize_seeded_l2_events_with_meter(
        &provenance,
        &events,
        work_budget,
    )?])
}

/// Normalize tar JSONL members into one canonical quote table.
///
/// The members are replayed in archive order. A snapshot in a later member
/// re-seeds the book just like a snapshot in the same member.
///
/// # Errors
///
/// Returns an error if any member is malformed or the combined seeded replay
/// emits no valid BBO.
pub fn normalize_seeded_l2_tar_jsonl_quotes(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &SeededL2QuoteMappingConfig,
    members: impl IntoIterator<Item = TarMember>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalQuotesTable>> {
    normalize_seeded_l2_tar_jsonl_quotes_with_meter(
        accepted,
        identity,
        mapping,
        members,
        capture_time_nanos,
        ingest_run_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn normalize_seeded_l2_tar_jsonl_quotes_with_meter(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &SeededL2QuoteMappingConfig,
    members: impl IntoIterator<Item = TarMember>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<CanonicalQuotesTable>> {
    let mut events = Vec::new();
    for member in members {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        let mut member_events =
            parse_seeded_l2_jsonl_with_meter(mapping, &member.text, work_budget)
                .with_context(|| format!("parse seeded L2 quote member {:?}", member.name))?;
        events.append(&mut member_events);
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    }
    ensure!(
        !events.is_empty(),
        "seeded L2 quote archive carried no in-scope events"
    );
    let provenance = SeededL2QuoteProvenance::from_accepted(
        accepted,
        identity,
        capture_time_nanos,
        ingest_run_id,
    );
    Ok(vec![normalize_seeded_l2_events_with_meter(
        &provenance,
        &events,
        work_budget,
    )?])
}

/// Parse decoded JSONL snapshot+delta rows into replay events.
///
/// # Errors
///
/// Returns an error if any line is malformed, lacks a configured field, or has
/// invalid price/size/timestamp values.
pub fn parse_seeded_l2_jsonl(
    mapping: &SeededL2QuoteMappingConfig,
    jsonl_text: &str,
) -> Result<Vec<SeededL2QuoteEvent>> {
    parse_seeded_l2_jsonl_with_meter(mapping, jsonl_text, &OperatorWorkBudgetGuard::unbounded())
}

fn parse_seeded_l2_jsonl_with_meter(
    mapping: &SeededL2QuoteMappingConfig,
    jsonl_text: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<SeededL2QuoteEvent>> {
    validate_mapping(mapping)?;
    let mut events = Vec::new();
    for_each_nonempty_text_record_with_budget(
        jsonl_text,
        work_budget,
        OperatorWorkBudgetStage::Normalize,
        |line_index, line| {
            let trimmed = line.trim();
            work_budget.consume_source_row(OperatorWorkBudgetStage::Normalize)?;
            let value: Value = deserialize_json_with_budget(
                trimmed.as_bytes(),
                work_budget,
                OperatorWorkBudgetStage::Normalize,
            )
            .with_context(|| format!("line {}: invalid JSON", line_index + 1))?;
            events.push(parse_seeded_l2_json_value(
                mapping,
                &value,
                line_index + 1,
                work_budget,
            )?);
            Ok(())
        },
    )?;
    ensure!(!events.is_empty(), "seeded L2 quote JSONL is empty");
    Ok(events)
}

/// Convert parsed seeded L2 events into a canonical quote table.
///
/// # Errors
///
/// Returns an error if replay begins with an update, price/size levels are
/// invalid, the output table is empty, or canonical quote validation fails.
pub fn normalize_seeded_l2_events(
    provenance: &SeededL2QuoteProvenance,
    events: &[SeededL2QuoteEvent],
) -> Result<CanonicalQuotesTable> {
    normalize_seeded_l2_events_with_meter(provenance, events, &OperatorWorkBudgetGuard::unbounded())
}

fn normalize_seeded_l2_events_with_meter(
    provenance: &SeededL2QuoteProvenance,
    events: &[SeededL2QuoteEvent],
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<CanonicalQuotesTable> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    ensure!(
        !provenance.ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        provenance.default_capture_time > 0,
        "default capture_time must be positive"
    );
    ensure!(!events.is_empty(), "seeded L2 quote event stream is empty");

    let mut book = SeededBook::default();
    let mut rows = Vec::new();
    for (index, event) in events.iter().enumerate() {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        ensure!(
            event.event_time > 0,
            "event {index}: non-positive event_time"
        );
        if let Some(capture_time) = event.capture_time {
            ensure!(capture_time > 0, "event {index}: non-positive capture_time");
        }
        match event.action {
            SeededL2QuoteAction::Snapshot => book.seed(event, index, work_budget)?,
            SeededL2QuoteAction::Update => book.update(event, index, work_budget)?,
        }
        if let Some((bid, ask)) = book.best_bid_ask() {
            rows.push(make_quote_row(provenance, event, bid, ask));
        }
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    }
    ensure!(!rows.is_empty(), "seeded L2 replay emitted no BBO quotes");
    let table = CanonicalQuotesTable {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        partition: TradesPartition {
            venue: provenance.venue.clone(),
            product_family: provenance.product_family.clone(),
            product_category: provenance.product_category.clone(),
            instrument_id: provenance.instrument_id.clone(),
            dt: provenance.partition_dt.clone(),
        },
        source_proof_id: provenance.source_proof_id.clone(),
        source_proof_version: provenance.source_proof_version,
        fidelity_class: SourceProofFidelityClass::QuoteReplay,
        forbidden_claims: provenance.forbidden_claims.clone(),
        transform_hash: provenance.transform_hash.clone(),
        payload_hash: provenance.payload_hash.clone(),
        rows,
    };
    table.validate_guarded(work_budget, OperatorWorkBudgetStage::Normalize)?;
    Ok(table)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BookLevel {
    price: String,
    size: String,
}

#[derive(Default)]
struct SeededBook {
    seeded: bool,
    bids: BTreeMap<Decimal, BookLevel>,
    asks: BTreeMap<Decimal, BookLevel>,
}

impl SeededBook {
    fn seed(
        &mut self,
        event: &SeededL2QuoteEvent,
        event_index: usize,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<()> {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        self.bids.clear();
        self.asks.clear();
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        apply_levels(&mut self.bids, &event.bids, event_index, "bid", work_budget)?;
        apply_levels(&mut self.asks, &event.asks, event_index, "ask", work_budget)?;
        self.seeded = true;
        Ok(())
    }

    fn update(
        &mut self,
        event: &SeededL2QuoteEvent,
        event_index: usize,
        work_budget: &OperatorWorkBudgetGuard,
    ) -> Result<()> {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        ensure!(
            self.seeded,
            "event {event_index}: L2 update arrived before a seeding snapshot"
        );
        apply_levels(&mut self.bids, &event.bids, event_index, "bid", work_budget)?;
        apply_levels(&mut self.asks, &event.asks, event_index, "ask", work_budget)?;
        Ok(())
    }

    fn best_bid_ask(&self) -> Option<(&BookLevel, &BookLevel)> {
        let bid = self.bids.iter().next_back()?.1;
        let ask = self.asks.iter().next()?.1;
        Some((bid, ask))
    }
}

fn apply_levels(
    side: &mut BTreeMap<Decimal, BookLevel>,
    levels: &[SeededL2QuoteLevel],
    event_index: usize,
    side_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<()> {
    for (level_index, level) in levels.iter().enumerate() {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        let price = parse_decimal(&level.price, event_index, level_index, "price")?;
        let size = parse_decimal(&level.size, event_index, level_index, "size")?;
        ensure!(
            price > Decimal::ZERO,
            "event {event_index} {side_label} level {level_index}: non-positive price {}",
            level.price
        );
        ensure!(
            size >= Decimal::ZERO,
            "event {event_index} {side_label} level {level_index}: negative size {}",
            level.size
        );
        if size == Decimal::ZERO {
            side.remove(&price);
        } else {
            side.insert(
                price,
                BookLevel {
                    price: level.price.clone(),
                    size: level.size.clone(),
                },
            );
        }
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    }
    Ok(())
}

fn parse_decimal(
    raw: &str,
    event_index: usize,
    level_index: usize,
    label: &str,
) -> Result<Decimal> {
    raw.parse::<Decimal>().with_context(|| {
        format!("event {event_index} level {level_index}: invalid {label} {raw:?}")
    })
}

fn make_quote_row(
    provenance: &SeededL2QuoteProvenance,
    event: &SeededL2QuoteEvent,
    bid: &BookLevel,
    ask: &BookLevel,
) -> CanonicalQuoteRow {
    CanonicalQuoteRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: provenance.ingest_run_id.clone(),
        source_binding: provenance.source_binding.clone(),
        venue: provenance.venue.clone(),
        product_family: provenance.product_family.clone(),
        product_category: provenance.product_category.clone(),
        instrument_id: provenance.instrument_id.clone(),
        canonical_instrument_key: provenance.canonical_instrument_key.clone(),
        venue_symbol: provenance.venue_symbol.clone(),
        nt_instrument_id: provenance.nt_instrument_id.clone(),
        event_time: event.event_time,
        capture_time: event
            .capture_time
            .unwrap_or(provenance.default_capture_time),
        // Seeded L2 BBO is a per-event stream, not one batch snapshot: each row's
        // source-availability instant is its own event_time. ts_init prefers
        // availability_time over capture_time, so carrying it per row spreads the
        // quotes across the window instead of collapsing them onto the single batch
        // capture instant — which had frozen the seeded signal/RV feed and aged it
        // out so the strategy never priced (issue #789). capture_time is unchanged.
        availability_time: Some(event.event_time),
        source_sequence: event.source_sequence.clone(),
        raw_payload_id: provenance.raw_payload_id.clone(),
        source_proof_id: provenance.source_proof_id.clone(),
        payload_hash: provenance.payload_hash.clone(),
        transform_hash: provenance.transform_hash.clone(),
        bid: bid.price.clone(),
        ask: ask.price.clone(),
        bid_size: bid.size.clone(),
        ask_size: ask.size.clone(),
    }
}

fn validate_mapping(mapping: &SeededL2QuoteMappingConfig) -> Result<()> {
    for (label, path) in [
        ("action_path", &mapping.action_path),
        ("event_time_path", &mapping.event_time_path),
        ("bids_path", &mapping.bids_path),
        ("asks_path", &mapping.asks_path),
    ] {
        ensure!(
            !path.is_empty(),
            "seeded L2 quote {label} must not be empty"
        );
        ensure!(
            path.iter().all(|segment| !segment.trim().is_empty()),
            "seeded L2 quote {label} must not contain an empty segment"
        );
    }
    if let Some(path) = &mapping.source_sequence_path {
        ensure!(
            !path.is_empty(),
            "seeded L2 quote source_sequence_path must not be empty when set"
        );
        ensure!(
            path.iter().all(|segment| !segment.trim().is_empty()),
            "seeded L2 quote source_sequence_path must not contain an empty segment"
        );
    }
    ensure!(
        !mapping.snapshot_action_values.is_empty(),
        "seeded L2 quote snapshot_action_values must not be empty"
    );
    ensure!(
        !mapping.update_action_values.is_empty(),
        "seeded L2 quote update_action_values must not be empty"
    );
    ensure!(
        mapping
            .snapshot_action_values
            .iter()
            .all(|value| !value.trim().is_empty()),
        "seeded L2 quote snapshot_action_values must not contain an empty value"
    );
    ensure!(
        mapping
            .update_action_values
            .iter()
            .all(|value| !value.trim().is_empty()),
        "seeded L2 quote update_action_values must not contain an empty value"
    );
    ensure!(
        mapping.level_price_index != mapping.level_size_index,
        "seeded L2 quote level price and size indices must differ"
    );
    Ok(())
}

fn parse_seeded_l2_json_value(
    mapping: &SeededL2QuoteMappingConfig,
    value: &Value,
    line_number: usize,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<SeededL2QuoteEvent> {
    work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    let action_raw = required_scalar_at_path(value, &mapping.action_path)
        .with_context(|| format!("line {line_number}: read action"))?;
    let action = parse_action(mapping, &action_raw)
        .with_context(|| format!("line {line_number}: unknown action {action_raw:?}"))?;
    let event_time_raw = required_scalar_at_path(value, &mapping.event_time_path)
        .with_context(|| format!("line {line_number}: read event time"))?;
    let event_time = mapping
        .event_time_unit
        .parse_to_nanos(&event_time_raw)
        .with_context(|| format!("line {line_number}: invalid event time {event_time_raw:?}"))?;
    ensure!(
        event_time > 0,
        "line {line_number}: non-positive event time"
    );
    let source_sequence = match &mapping.source_sequence_path {
        Some(path) => optional_scalar_at_path(value, path)
            .with_context(|| format!("line {line_number}: read source sequence"))?,
        None => None,
    };
    let bids = levels_at_path(
        value,
        &mapping.bids_path,
        mapping,
        line_number,
        "bids",
        work_budget,
    )
    .with_context(|| format!("line {line_number}: read bids"))?;
    let asks = levels_at_path(
        value,
        &mapping.asks_path,
        mapping,
        line_number,
        "asks",
        work_budget,
    )
    .with_context(|| format!("line {line_number}: read asks"))?;
    let event = SeededL2QuoteEvent {
        action,
        event_time,
        capture_time: None,
        source_sequence,
        bids,
        asks,
    };
    work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    Ok(event)
}

fn parse_action(mapping: &SeededL2QuoteMappingConfig, raw: &str) -> Result<SeededL2QuoteAction> {
    let raw = raw.trim();
    if mapping
        .snapshot_action_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw))
    {
        return Ok(SeededL2QuoteAction::Snapshot);
    }
    if mapping
        .update_action_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw))
    {
        return Ok(SeededL2QuoteAction::Update);
    }
    bail!("action does not match configured snapshot/update values")
}

fn levels_at_path(
    value: &Value,
    path: &[String],
    mapping: &SeededL2QuoteMappingConfig,
    line_number: usize,
    side_label: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<SeededL2QuoteLevel>> {
    let levels = value_at_path(value, path)
        .with_context(|| format!("missing {side_label} path {}", path.join(".")))?;
    let levels = levels
        .as_array()
        .with_context(|| format!("{side_label} path {} is not an array", path.join(".")))?;
    let mut parsed = Vec::with_capacity(levels.len());
    for (level_index, level) in levels.iter().enumerate() {
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
        let fields = level.as_array().with_context(|| {
            format!("line {line_number} {side_label} level {level_index} is not an array")
        })?;
        let price = scalar_from_index(fields, mapping.level_price_index).with_context(|| {
            format!("line {line_number} {side_label} level {level_index}: missing price")
        })?;
        let size = scalar_from_index(fields, mapping.level_size_index).with_context(|| {
            format!("line {line_number} {side_label} level {level_index}: missing size")
        })?;
        parsed.push(SeededL2QuoteLevel { price, size });
        work_budget.check_deadline(OperatorWorkBudgetStage::Normalize)?;
    }
    Ok(parsed)
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

fn optional_scalar_at_path(value: &Value, path: &[String]) -> Result<Option<String>> {
    match value_at_path(value, path) {
        Some(value) if !value.is_null() => Ok(Some(scalar_to_string(value)?)),
        _ => Ok(None),
    }
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
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;

    struct NormalizationExpiryClock {
        observations: AtomicUsize,
        expires_after_observation: usize,
    }

    impl crate::operator_work_budget::OperatorWorkBudgetClock for NormalizationExpiryClock {
        fn now(&self) -> Duration {
            if self.observations.fetch_add(1, Ordering::SeqCst) >= self.expires_after_observation {
                Duration::from_secs(1)
            } else {
                Duration::ZERO
            }
        }
    }

    fn expiring_normalization_guard(expires_after_observation: usize) -> OperatorWorkBudgetGuard {
        OperatorWorkBudgetGuard::with_clock(
            crate::operator_work_budget::OperatorWorkBudget::Backfill(
                crate::backfill_execution_plan::BackfillExecutionWorkBudget {
                    max_source_rows: 1,
                    max_decoded_bytes: u64::MAX,
                    max_projected_row_groups: 1,
                    max_wall_seconds: 1,
                    require_object_selection_metadata: false,
                },
            ),
            Arc::new(NormalizationExpiryClock {
                observations: AtomicUsize::new(0),
                expires_after_observation,
            }),
        )
        .expect("expiring normalization guard")
    }

    fn test_mapping() -> SeededL2QuoteMappingConfig {
        SeededL2QuoteMappingConfig {
            action_path: vec!["action".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["bids".to_string()],
            asks_path: vec!["asks".to_string()],
            level_price_index: 0,
            level_size_index: 1,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["update".to_string()],
            source_sequence_path: None,
        }
    }

    fn provenance() -> SeededL2QuoteProvenance {
        SeededL2QuoteProvenance {
            ingest_run_id: "ingest-run".to_string(),
            source_binding: "spot-l2".to_string(),
            venue: "testvenue".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            instrument_id: "BTC-USDT".to_string(),
            canonical_instrument_key: "testvenue/spot/BTC-USDT".to_string(),
            venue_symbol: "BTC-USDT".to_string(),
            nt_instrument_id: Some("BTC-USDT.TESTVENUE".to_string()),
            partition_dt: "2026-04-22".to_string(),
            source_proof_id: "source-proof".to_string(),
            source_proof_version: 1,
            forbidden_claims: vec!["No execution-quality claims.".to_string()],
            raw_payload_id: "payload".to_string(),
            payload_hash: "payload".to_string(),
            transform_hash: seeded_l2_quote_transform_hash(),
            default_capture_time: 1_776_816_000_000_000_000,
        }
    }

    fn level(price: &str, size: &str) -> SeededL2QuoteLevel {
        SeededL2QuoteLevel {
            price: price.to_string(),
            size: size.to_string(),
        }
    }

    #[test]
    fn update_before_snapshot_is_rejected() {
        let events = vec![SeededL2QuoteEvent {
            action: SeededL2QuoteAction::Update,
            event_time: 1_776_816_000_000_000_000,
            capture_time: None,
            source_sequence: None,
            bids: vec![level("100", "1")],
            asks: vec![],
        }];

        let error = normalize_seeded_l2_events(&provenance(), &events)
            .expect_err("update-only stream must fail");
        assert!(
            error.to_string().contains("before a seeding snapshot"),
            "{error}"
        );
    }

    #[test]
    fn snapshot_seed_then_absolute_replace_updates_emit_bbo_quotes() {
        let base = 1_776_816_000_000_000_000;
        let events = vec![
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Snapshot,
                event_time: base,
                capture_time: None,
                source_sequence: Some("1".to_string()),
                bids: vec![level("100", "1"), level("99", "2")],
                asks: vec![level("101", "3"), level("102", "4")],
            },
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Update,
                event_time: base + 1,
                capture_time: None,
                source_sequence: Some("2".to_string()),
                bids: vec![level("100", "0")],
                asks: vec![level("101", "5")],
            },
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Update,
                event_time: base + 2,
                capture_time: None,
                source_sequence: Some("3".to_string()),
                bids: vec![level("100.5", "8")],
                asks: vec![],
            },
        ];

        let table =
            normalize_seeded_l2_events(&provenance(), &events).expect("seeded replay emits quotes");
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0].bid, "100");
        assert_eq!(table.rows[0].ask, "101");
        assert_eq!(table.rows[0].bid_size, "1");
        assert_eq!(table.rows[0].ask_size, "3");
        assert_eq!(table.rows[1].bid, "99");
        assert_eq!(table.rows[1].ask, "101");
        assert_eq!(table.rows[1].ask_size, "5");
        assert_eq!(table.rows[2].bid, "100.5");
        assert_eq!(table.rows[2].ask, "101");
        assert_eq!(table.fidelity_class, SourceProofFidelityClass::QuoteReplay);
    }

    #[test]
    fn one_seeded_l2_row_with_many_levels_stops_during_nested_parse() {
        let bids = (0..128)
            .map(|index| serde_json::json!([format!("{index}.1"), "1"]))
            .collect::<Vec<_>>();
        let jsonl = serde_json::json!({
            "action": "snapshot",
            "ts": 1_776_816_000_000_i64,
            "bids": bids,
            "asks": [["999.1", "1"]],
        })
        .to_string();
        let guard = expiring_normalization_guard(8);

        let error = parse_seeded_l2_jsonl_with_meter(&test_mapping(), &jsonl, &guard)
            .expect_err("one source row must not bypass the deadline through level parsing");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("normalize"), "{error:#}");
        assert_eq!(guard.source_rows_consumed(), 1);
    }

    #[test]
    fn seeded_book_application_stops_inside_one_high_level_snapshot() {
        let bids = (1..=128)
            .map(|index| level(&index.to_string(), "1"))
            .collect();
        let events = vec![SeededL2QuoteEvent {
            action: SeededL2QuoteAction::Snapshot,
            event_time: 1_776_816_000_000_000_000,
            capture_time: None,
            source_sequence: Some("1".to_string()),
            bids,
            asks: vec![level("999", "1")],
        }];
        let guard = expiring_normalization_guard(6);

        let error = normalize_seeded_l2_events_with_meter(&provenance(), &events, &guard)
            .expect_err("seeded-book level application must observe the deadline");

        assert!(error.to_string().contains("max_wall_seconds"), "{error:#}");
        assert!(error.to_string().contains("normalize"), "{error:#}");
    }

    #[test]
    fn seeded_l2_quote_rows_carry_per_event_availability_time() {
        // Differential guard for the issue #789 signal-feed freeze: each seeded-L2
        // BBO row must carry its OWN event_time as availability_time so ts_init
        // advances per row. With availability_time = None the rows collapse onto the
        // single batch capture instant, the backtest delivers every quote at t0, and
        // the strategy's spot observation ages out and never prices.
        let base = 1_776_816_000_000_000_000;
        let events = vec![
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Snapshot,
                event_time: base,
                capture_time: None,
                source_sequence: Some("1".to_string()),
                bids: vec![level("100", "1")],
                asks: vec![level("101", "3")],
            },
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Update,
                event_time: base + 1_000,
                capture_time: None,
                source_sequence: Some("2".to_string()),
                bids: vec![level("100.5", "2")],
                asks: vec![level("101", "4")],
            },
            SeededL2QuoteEvent {
                action: SeededL2QuoteAction::Update,
                event_time: base + 2_000,
                capture_time: None,
                source_sequence: Some("3".to_string()),
                bids: vec![level("100.75", "2")],
                asks: vec![level("101.5", "4")],
            },
        ];

        let table =
            normalize_seeded_l2_events(&provenance(), &events).expect("seeded replay emits quotes");
        assert_eq!(table.rows.len(), 3);
        for row in &table.rows {
            assert_eq!(
                row.availability_time,
                Some(row.event_time),
                "each seeded-L2 quote row's availability_time must equal its own event_time"
            );
        }
        let distinct: std::collections::BTreeSet<_> =
            table.rows.iter().map(|row| row.availability_time).collect();
        assert_eq!(
            distinct.len(),
            3,
            "availability_time must advance per event, not collapse onto one batch instant"
        );
    }

    #[test]
    fn mapping_driven_parser_handles_okx_and_bybit_shapes() {
        let okx_mapping = SeededL2QuoteMappingConfig {
            action_path: vec!["action".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["bids".to_string()],
            asks_path: vec!["asks".to_string()],
            level_price_index: 0,
            level_size_index: 1,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["update".to_string()],
            source_sequence_path: None,
        };
        let okx = r#"{"action":"snapshot","ts":"1776816000000","bids":[["100","1","0"]],"asks":[["101","2","0"]]}"#;
        let okx_events = parse_seeded_l2_jsonl(&okx_mapping, okx).expect("parse OKX shape");
        assert_eq!(okx_events[0].action, SeededL2QuoteAction::Snapshot);
        assert_eq!(okx_events[0].event_time, 1_776_816_000_000_000_000);
        assert_eq!(okx_events[0].bids[0], level("100", "1"));

        let bybit_mapping = SeededL2QuoteMappingConfig {
            action_path: vec!["type".to_string()],
            event_time_path: vec!["ts".to_string()],
            event_time_unit: CsvTimestampUnit::Milliseconds,
            bids_path: vec!["data".to_string(), "b".to_string()],
            asks_path: vec!["data".to_string(), "a".to_string()],
            level_price_index: 0,
            level_size_index: 1,
            snapshot_action_values: vec!["snapshot".to_string()],
            update_action_values: vec!["delta".to_string()],
            source_sequence_path: Some(vec!["data".to_string(), "seq".to_string()]),
        };
        let bybit = r#"{"topic":"orderbook.50.BTCUSDT","type":"delta","ts":1776816000001,"data":{"s":"BTCUSDT","seq":7,"b":[["100","0"]],"a":[["101","3"]]}}"#;
        let bybit_events = parse_seeded_l2_jsonl(&bybit_mapping, bybit).expect("parse Bybit shape");
        assert_eq!(bybit_events[0].action, SeededL2QuoteAction::Update);
        assert_eq!(bybit_events[0].source_sequence.as_deref(), Some("7"));
        assert_eq!(bybit_events[0].asks[0], level("101", "3"));
    }
}
