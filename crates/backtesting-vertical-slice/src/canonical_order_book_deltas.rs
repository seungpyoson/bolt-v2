//! Gate 2 — config-driven JSONL periodic-full-snapshot L2 delta source adapter
//! (format family S3).
//!
//! Normalizes an accepted JSONL object of periodic full order-book photos into
//! the `order_book_snapshot_deltas` table family of the
//! `backfill-table-contract.v1` contract, emitting one
//! [`CanonicalOrderBookDeltasTable`] per instrument carried in the object.
//!
//! This is the order-book-delta sibling of [`super::canonical_bars`]: it reuses
//! the same config-schema / identity-resolution / provenance discipline (a
//! run-spec owned mapping, [`DeltaInstrumentIdentities`] built by the caller
//! from accepted instrument-universe data, and the shared
//! [`super::canonical_trades::CsvTimestampUnit`] for event-time parsing) and
//! preserves the exact source price/size strings so the catalog projection in
//! [`super::catalog_projection`] is the single bridge from accepted evidence to
//! the NautilusTrader catalog.
//!
//! Each JSONL line is one full L2 photo. A photo expands to a `CLEAR`
//! (`F_SNAPSHOT`) followed by one `ADD` per level (bids then asks, each
//! `F_SNAPSHOT`); the final row of the photo additionally carries `F_LAST` to
//! close the book event. An empty photo (no levels on either side) collapses to
//! a lone `CLEAR` carrying `F_SNAPSHOT | F_LAST`, and runs of consecutive empty
//! photos collapse onto that single `CLEAR` so the table never carries two
//! `CLEAR` rows back to back (a shape the contract forbids).
//!
//! This slice implements only the periodic-full-snapshot wire shape
//! ([`DeltaSourceFormat::Snapshot`]); event-stream and tar-bundled L2 families
//! are separate slices and are deliberately absent from the format enum.
//!
//! Input is only ever an [`AcceptedDataset`] from gate 1 — raw staged data never
//! reaches this module without first passing source-proof acceptance.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use nautilus_model::enums::RecordFlag;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    canonical_market_data::{
        CanonicalOrderBookDeltaRow, CanonicalOrderBookDeltasTable, DeltaAction, DeltaSide,
        NORMALIZED_SCHEMA_VERSION,
    },
    canonical_trades::{
        CanonicalInstrumentIdentity, CsvTimestampUnit, DELTAS_TRANSFORM_IDENTITY, TradesPartition,
    },
    source_proof::AcceptedDataset,
};

/// Run-spec owned JSONL order-book-delta column mapping for the S3 source
/// adapter.
///
/// A new source that emits the same periodic-full-snapshot JSONL shape selects
/// the delta converter from TOML and supplies its field mapping here. Mirrors
/// [`super::canonical_bars::BarMappingConfig`]: the timestamp unit is the shared
/// [`CsvTimestampUnit`], and field names are resolved against each JSON object
/// rather than positionally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaMappingConfig {
    pub format: DeltaSourceFormat,
    pub instrument_key: InstrumentKeySpec,
    pub ordering: OrderingAuthority,
    pub price_sign_policy: DeltaPriceSignPolicy,
    pub empty_book_policy: EmptyBookPolicy,
}

/// Wire shape of the accepted L2 archive object.
///
/// This slice implements only [`DeltaSourceFormat::Snapshot`] — one full L2
/// photo per JSONL line. Event-stream and tar-bundled L2 families are separate
/// slices, so no speculative variants are declared here; a new variant fails to
/// deserialize until its slice registers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeltaSourceFormat {
    /// Each JSONL line is one full order-book photo: `bids_field` and
    /// `asks_field` each hold an array of level objects keyed by
    /// `level_price_field` / `level_size_field`, and `event_time_field` holds
    /// the exchange book time in `event_time_unit` units.
    Snapshot {
        bids_field: String,
        asks_field: String,
        level_price_field: String,
        level_size_field: String,
        event_time_field: String,
        event_time_unit: CsvTimestampUnit,
    },
}

/// How a row's instrument is keyed in a multi-instrument object.
///
/// A `key_field` of `None` selects the single-instrument object shape (one
/// identity bound to every photo). When set, the field value keys the per-row
/// instrument, and `exclusion_filter` declaratively fences out keys that must
/// never reach normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstrumentKeySpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusion_filter: Option<InstrumentExclusionFilter>,
}

/// Declarative instrument-key fence.
///
/// Expresses the venue classifier as config rather than code: a key is fenced
/// out (dropped silently at grouping time) when it contains any
/// `exclude_if_contains` substring or starts with any `exclude_if_prefix`
/// prefix. Both lists default empty (no fencing).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct InstrumentExclusionFilter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_if_contains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_if_prefix: Vec<String>,
}

impl InstrumentExclusionFilter {
    /// Whether `key` is fenced out by this filter.
    fn excludes(&self, key: &str) -> bool {
        self.exclude_if_contains
            .iter()
            .any(|needle| key.contains(needle.as_str()))
            || self
                .exclude_if_prefix
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
    }
}

/// Authority that orders rows within an instrument's table.
///
/// This slice supports only [`OrderingAuthority::EventTime`]: rows keep their
/// photo order and the per-instrument event time must be non-decreasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingAuthority {
    EventTime,
}

/// Sign policy for order-book level price and size.
///
/// Mirrors [`super::canonical_bars::BarPriceSignPolicy`] for the delta family:
/// every level in a FULL snapshot must carry a strictly-positive price AND
/// size. A zero-size level in a full photo is meaningless (unlike an
/// event-stream delete, where size `0` removes a level — that is the deferred
/// event-stream family), so it fails loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaPriceSignPolicy {
    /// Every level price and size must be strictly positive.
    StrictlyPositive,
}

/// How empty (no-level) photos are represented.
///
/// This slice supports only [`EmptyBookPolicy::LoneClearLast`]: an empty photo
/// becomes a single `CLEAR` carrying `F_SNAPSHOT | F_LAST`, and runs of
/// consecutive empty photos collapse onto that one `CLEAR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmptyBookPolicy {
    LoneClearLast,
}

/// Instrument-identity resolution for an order-book-delta object.
///
/// A single-instrument object binds one identity to every photo; a
/// multi-instrument object keys identities by the configured `key_field` value.
/// Built by the caller from accepted instrument-universe data, so no instrument
/// identity is hardcoded in this module. Separate from
/// [`super::canonical_bars::BarInstrumentIdentities`] so the two families evolve
/// independently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaInstrumentIdentities {
    Single(CanonicalInstrumentIdentity),
    Keyed(BTreeMap<String, CanonicalInstrumentIdentity>),
}

impl DeltaInstrumentIdentities {
    /// Resolve the identity for one photo, given the configured instrument-key
    /// value (`None` for the single-instrument shape).
    pub(crate) fn resolve(
        &self,
        instrument_key: Option<&str>,
    ) -> Result<&CanonicalInstrumentIdentity> {
        match self {
            Self::Single(identity) => {
                ensure!(
                    instrument_key.is_none(),
                    "single-instrument identities cannot resolve instrument key {:?}",
                    instrument_key
                );
                Ok(identity)
            }
            Self::Keyed(identities) => {
                let key = instrument_key
                    .context("keyed instrument identities require a configured key_field")?;
                identities.get(key).with_context(|| {
                    format!("no instrument identity registered for instrument key {key:?}")
                })
            }
        }
    }
}

/// Lowercase SHA-256 hex of the given transform identity string.
///
/// The caller selects the identity for the adapter being used (e.g.
/// [`DELTAS_TRANSFORM_IDENTITY`] for the JSONL snapshot adapter). Later
/// adapters (tar, parquet) pass their own identity constants so every
/// adapter family stamps a distinct, correct `transform_hash`.
#[must_use]
pub fn delta_transform_hash(identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    hex::encode(hasher.finalize())
}

/// One parsed level of a photo (exact source decimal strings).
struct ParsedLevel {
    price: String,
    size: String,
}

/// One parsed photo, before identity/provenance assembly.
struct ParsedPhoto {
    event_time: i64,
    bids: Vec<ParsedLevel>,
    asks: Vec<ParsedLevel>,
}

/// Normalize an accepted JSONL periodic-full-snapshot object into one
/// [`CanonicalOrderBookDeltasTable`] per instrument.
///
/// `jsonl_text` must be the decoded text of the accepted object whose hash
/// already matched the manifest (the caller verified it via gate 1), one JSON
/// photo per line. `capture_time_nanos` is the ingest capture timestamp recorded
/// for the run. `ingest_run_id` is the stable identifier of the ingest/run that
/// produced this normalization, recorded for lineage; it is not the source
/// object URL.
///
/// # Errors
///
/// Returns an error if a line is malformed JSON, a configured field is missing,
/// a level price/size is non-positive, an instrument's event time regresses, the
/// keyed identity is unknown, or a produced table fails its contract.
pub fn normalize_jsonl_snapshot_deltas(
    accepted: &AcceptedDataset,
    identities: &DeltaInstrumentIdentities,
    mapping: &DeltaMappingConfig,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalOrderBookDeltasTable>> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );

    let DeltaSourceFormat::Snapshot {
        bids_field,
        asks_field,
        level_price_field,
        level_size_field,
        event_time_field,
        event_time_unit,
    } = &mapping.format;
    for (label, field) in [
        ("bids_field", bids_field),
        ("asks_field", asks_field),
        ("level_price_field", level_price_field),
        ("level_size_field", level_size_field),
        ("event_time_field", event_time_field),
    ] {
        ensure!(
            !field.trim().is_empty(),
            "converter deltas.{label} must not be empty"
        );
    }
    if let Some(key_field) = &mapping.instrument_key.key_field {
        ensure!(
            !key_field.trim().is_empty(),
            "converter deltas.instrument_key.key_field must not be empty when set"
        );
    }
    if let Some(filter) = &mapping.instrument_key.exclusion_filter {
        for needle in &filter.exclude_if_contains {
            ensure!(
                !needle.trim().is_empty(),
                "converter deltas.instrument_key.exclusion_filter.exclude_if_contains must not contain empty needles"
            );
        }
        for prefix in &filter.exclude_if_prefix {
            ensure!(
                !prefix.trim().is_empty(),
                "converter deltas.instrument_key.exclusion_filter.exclude_if_prefix must not contain empty prefixes"
            );
        }
    }

    // Group parsed photos by instrument key (single-instrument objects use one
    // group keyed by `None`), preserving first-seen group order.
    let mut group_order: Vec<Option<String>> = Vec::new();
    let mut groups: BTreeMap<Option<String>, Vec<ParsedPhoto>> = BTreeMap::new();

    for (index, line) in jsonl_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("line {index}: malformed snapshot JSON"))?;

        let instrument_key = match &mapping.instrument_key.key_field {
            Some(key_field) => {
                let raw = value
                    .get(key_field)
                    .and_then(Value::as_str)
                    .with_context(|| {
                        format!("line {index}: missing string instrument key field {key_field:?}")
                    })?;
                ensure!(!raw.trim().is_empty(), "line {index}: empty instrument key");
                if let Some(filter) = &mapping.instrument_key.exclusion_filter
                    && filter.excludes(raw)
                {
                    continue;
                }
                Some(raw.to_string())
            }
            None => None,
        };

        let event_time_raw = value.get(event_time_field).with_context(|| {
            format!("line {index}: missing event time field {event_time_field:?}")
        })?;
        let event_time = parse_event_time(*event_time_unit, event_time_raw)
            .with_context(|| format!("line {index}: invalid event time {event_time_raw}"))?;
        ensure!(event_time > 0, "line {index}: non-positive event time");

        let bids = parse_levels(
            index,
            &value,
            bids_field,
            level_price_field,
            level_size_field,
            mapping.price_sign_policy,
        )?;
        let asks = parse_levels(
            index,
            &value,
            asks_field,
            level_price_field,
            level_size_field,
            mapping.price_sign_policy,
        )?;

        let group = groups.entry(instrument_key.clone()).or_insert_with(|| {
            group_order.push(instrument_key.clone());
            Vec::new()
        });
        group.push(ParsedPhoto {
            event_time,
            bids,
            asks,
        });
    }

    ensure!(
        !group_order.is_empty(),
        "snapshot object yielded no in-scope photos"
    );

    let canonical_instrument_key_prefix = format!("{}/{}", accepted.venue, accepted.product_family);
    let transform_hash = delta_transform_hash(DELTAS_TRANSFORM_IDENTITY);

    let mut tables = Vec::with_capacity(group_order.len());
    for instrument_key in &group_order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let canonical_instrument_key = format!(
            "{canonical_instrument_key_prefix}/{}",
            identity.instrument_id
        );
        let photos = groups
            .remove(instrument_key)
            .context("internal: group_order entry missing from groups map")?;

        let provenance = RowProvenance {
            accepted,
            identity,
            canonical_instrument_key: &canonical_instrument_key,
            transform_hash: &transform_hash,
            capture_time_nanos,
            ingest_run_id,
        };
        let rows = expand_photos(
            &provenance,
            mapping.ordering,
            mapping.empty_book_policy,
            &photos,
        )?;

        let table = CanonicalOrderBookDeltasTable {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: TradesPartition {
                venue: accepted.venue.clone(),
                product_family: accepted.product_family.clone(),
                product_category: accepted.product_category.clone(),
                instrument_id: identity.instrument_id.clone(),
                dt: accepted.object.archive_date.clone(),
            },
            source_proof_id: accepted.source_proof_id.clone(),
            source_proof_version: accepted.source_proof_version,
            fidelity_class: accepted.fidelity_class,
            forbidden_claims: accepted.forbidden_claims.clone(),
            transform_hash: transform_hash.clone(),
            payload_hash: accepted.object.sha256.clone(),
            rows,
        };
        table.validate()?;
        tables.push(table);
    }

    Ok(tables)
}

/// Parse one photo's event time from a JSON value through the configured unit.
///
/// Numeric event times serialize as JSON integers; the shared
/// [`CsvTimestampUnit::parse_to_nanos`] parser is string-based, so a numeric
/// value is rendered without quotes before parsing and a string value is parsed
/// directly. Non-integer JSON (objects, arrays, booleans, null) fails loud.
fn parse_event_time(unit: CsvTimestampUnit, value: &Value) -> Result<i64> {
    match value {
        Value::Number(number) => unit.parse_to_nanos(&number.to_string()),
        Value::String(text) => unit.parse_to_nanos(text),
        other => bail!("event time must be a JSON number or string, got {other}"),
    }
}

/// Parse one side's level array from a photo, enforcing the sign policy.
fn parse_levels(
    line_index: usize,
    photo: &Value,
    side_field: &str,
    price_field: &str,
    size_field: &str,
    policy: DeltaPriceSignPolicy,
) -> Result<Vec<ParsedLevel>> {
    let array = photo
        .get(side_field)
        .with_context(|| format!("line {line_index}: missing side field {side_field:?}"))?
        .as_array()
        .with_context(|| format!("line {line_index}: side field {side_field:?} is not an array"))?;
    let mut levels = Vec::with_capacity(array.len());
    for (level_index, level) in array.iter().enumerate() {
        let price = level
            .get(price_field)
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "line {line_index} level {level_index}: missing string price field {price_field:?}"
                )
            })?;
        let size = level
            .get(size_field)
            .and_then(Value::as_str)
            .with_context(|| {
                format!(
                    "line {line_index} level {level_index}: missing string size field {size_field:?}"
                )
            })?;
        apply_level_sign_policy(line_index, level_index, policy, price, size)?;
        levels.push(ParsedLevel {
            price: price.to_string(),
            size: size.to_string(),
        });
    }
    Ok(levels)
}

/// Enforce the level sign policy for one parsed level.
///
/// `StrictlyPositive` rejects any non-positive price or size: a full snapshot
/// level must carry both.
fn apply_level_sign_policy(
    line_index: usize,
    level_index: usize,
    policy: DeltaPriceSignPolicy,
    price: &str,
    size: &str,
) -> Result<()> {
    match policy {
        DeltaPriceSignPolicy::StrictlyPositive => {
            for (label, value) in [("price", price), ("size", size)] {
                let parsed: Decimal = value.parse().with_context(|| {
                    format!("line {line_index} level {level_index}: invalid {label} {value:?}")
                })?;
                ensure!(
                    parsed > Decimal::ZERO,
                    "line {line_index} level {level_index}: non-positive {label} {value:?}"
                );
            }
            Ok(())
        }
    }
}

/// Per-table provenance constants shared by every row of one instrument's
/// table.
///
/// Bundles the values that are invariant across a table's rows (the accepted
/// dataset, the resolved identity, the canonical instrument key, the transform
/// hash, the capture time, and the ingest run id) so row construction and photo
/// expansion take one context reference rather than a long argument list, and so
/// the per-table constants have a single owner.
struct RowProvenance<'a> {
    accepted: &'a AcceptedDataset,
    identity: &'a CanonicalInstrumentIdentity,
    canonical_instrument_key: &'a str,
    transform_hash: &'a str,
    capture_time_nanos: i64,
    ingest_run_id: &'a str,
}

/// One row's payload fields, distinct from the per-table [`RowProvenance`].
struct RowPayload<'a> {
    event_time: i64,
    action: DeltaAction,
    side: &'a str,
    price: &'a str,
    size: &'a str,
    flags: u8,
}

/// Expand an instrument's ordered photos into canonical delta rows.
///
/// Each non-empty photo becomes `CLEAR` + one `ADD` per level (bids then asks),
/// all carrying `F_SNAPSHOT`, with the photo's final row also carrying `F_LAST`.
/// An empty photo becomes a lone `CLEAR` carrying `F_SNAPSHOT | F_LAST`; a run of
/// consecutive empty photos collapses onto that one `CLEAR`, and the first
/// populated photo after a lone `CLEAR` expands as snapshot ADDs only (its own
/// `CLEAR` would be adjacent to the previous one, which the contract forbids,
/// and the book is already provably empty). The table therefore never carries
/// two `CLEAR` rows in a row. Dense `sequence` is assigned after the collapse.
fn expand_photos(
    provenance: &RowProvenance<'_>,
    ordering: OrderingAuthority,
    empty_book_policy: EmptyBookPolicy,
    photos: &[ParsedPhoto],
) -> Result<Vec<CanonicalOrderBookDeltaRow>> {
    let OrderingAuthority::EventTime = ordering;
    let EmptyBookPolicy::LoneClearLast = empty_book_policy;

    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let last_flag = RecordFlag::F_LAST as u8;

    let mut rows: Vec<CanonicalOrderBookDeltaRow> = Vec::new();
    let mut previous_event_time = i64::MIN;
    let mut previous_was_lone_clear = false;

    for photo in photos {
        ensure!(
            photo.event_time >= previous_event_time,
            "instrument {:?}: event time {} precedes previous {}",
            provenance.identity.instrument_id,
            photo.event_time,
            previous_event_time
        );
        previous_event_time = photo.event_time;

        let is_empty = photo.bids.is_empty() && photo.asks.is_empty();
        if is_empty {
            // Collapse a run of consecutive empty photos onto the single CLEAR
            // already emitted: a second back-to-back CLEAR carries no book
            // information and the contract forbids it.
            if previous_was_lone_clear {
                continue;
            }
            rows.push(make_row(
                provenance,
                &RowPayload {
                    event_time: photo.event_time,
                    action: DeltaAction::Clear,
                    side: "",
                    price: "",
                    size: "",
                    flags: snapshot_flags | last_flag,
                },
            ));
            previous_was_lone_clear = true;
            continue;
        }
        let book_established_empty = previous_was_lone_clear;
        previous_was_lone_clear = false;

        // A populated photo normally opens with its own CLEAR. When the
        // previous event was a lone CLEAR the book is already provably empty,
        // so a second CLEAR would be adjacent to the first (which the
        // contract forbids) and carries no information: the photo expands as
        // snapshot ADDs over the established-empty book instead.
        if !book_established_empty {
            rows.push(make_row(
                provenance,
                &RowPayload {
                    event_time: photo.event_time,
                    action: DeltaAction::Clear,
                    side: "",
                    price: "",
                    size: "",
                    flags: snapshot_flags,
                },
            ));
        }
        for (side, levels) in [
            (DeltaSide::Buy, &photo.bids),
            (DeltaSide::Sell, &photo.asks),
        ] {
            for level in levels {
                rows.push(make_row(
                    provenance,
                    &RowPayload {
                        event_time: photo.event_time,
                        action: DeltaAction::Add,
                        side: side.as_str(),
                        price: &level.price,
                        size: &level.size,
                        flags: snapshot_flags,
                    },
                ));
            }
        }
        // Close the photo's book event on its final row. A non-empty photo
        // always pushes at least one row above, so an empty `rows` here is an
        // internal invariant breach: fail loud instead of panicking.
        let last = rows
            .last_mut()
            .context("internal: non-empty photo emitted no rows")?;
        last.flags |= last_flag;
    }

    ensure!(
        !rows.is_empty(),
        "instrument {:?} yielded no delta rows",
        provenance.identity.instrument_id
    );
    // `sequence` and `source_sequence` are both set to the converter-assigned
    // dense per-instrument ordinal (0, 1, 2, …). `source_sequence` does NOT
    // carry a venue-native sequence number — periodic-full-snapshot objects
    // carry no per-row native identity. The field is populated so downstream
    // readers can reference a stable within-table row index without treating
    // it as a wire-derived identifier.
    for (sequence, row) in rows.iter_mut().enumerate() {
        let sequence = sequence as u64;
        row.sequence = sequence;
        row.source_sequence = Some(sequence.to_string());
    }
    Ok(rows)
}

/// Build one canonical delta row from the per-table [`RowProvenance`] and the
/// row's [`RowPayload`].
///
/// `sequence` and `source_sequence` are assigned by [`expand_photos`] after
/// expansion and collapse. Both are set to the converter-assigned dense
/// per-instrument ordinal; `source_sequence` is NOT a venue-native sequence
/// number (periodic-full-snapshot objects carry no per-row wire identity).
/// This constructor leaves `sequence` at `0` and `source_sequence` at `None`.
fn make_row(
    provenance: &RowProvenance<'_>,
    payload: &RowPayload<'_>,
) -> CanonicalOrderBookDeltaRow {
    let accepted = provenance.accepted;
    let identity = provenance.identity;
    CanonicalOrderBookDeltaRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: provenance.ingest_run_id.to_string(),
        source_binding: accepted.source_binding.clone(),
        venue: accepted.venue.clone(),
        product_family: accepted.product_family.clone(),
        product_category: accepted.product_category.clone(),
        instrument_id: identity.instrument_id.clone(),
        canonical_instrument_key: provenance.canonical_instrument_key.to_string(),
        venue_symbol: identity.venue_symbol.clone(),
        nt_instrument_id: Some(identity.nt_instrument_id.clone()),
        event_time: payload.event_time,
        capture_time: provenance.capture_time_nanos,
        availability_time: None,
        source_sequence: None,
        raw_payload_id: accepted.object.sha256.clone(),
        source_proof_id: accepted.source_proof_id.clone(),
        payload_hash: accepted.object.sha256.clone(),
        transform_hash: provenance.transform_hash.to_string(),
        action: payload.action.as_str().to_string(),
        side: payload.side.to_string(),
        price: payload.price.to_string(),
        size: payload.size.to_string(),
        order_id: 0,
        flags: payload.flags,
        sequence: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source_proof::{
        AcceptanceMode, AcceptanceScope, EvidenceState, FixtureType, IngestManifestObjectRecord,
        L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks,
        SourceBindingRegistry, SourceCandidateClass, SourceProofClaimLimit,
        SourceProofFidelityClass, SourceProofReport, SourceProofStatus, SourceProofUsageScope,
        SourceSelectionStatus, TimeRange, select_accepted_dataset_with_registry,
    };

    const OBJECT_SHA256: &str = "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598";
    const SOURCE_URL: &str = "https://synthetic.invalid/data";

    fn source_binding_registry() -> SourceBindingRegistry {
        SourceBindingRegistry::from_toml_str(
            r#"[[source_binding]]
key = "testvenue-deltas"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["order_book_snapshot_deltas"]
"#,
        )
        .expect("synthetic source binding registry parses")
    }

    fn claim_limits_for(claims: &[String]) -> Vec<SourceProofClaimLimit> {
        claims
            .iter()
            .enumerate()
            .map(|(index, claim)| SourceProofClaimLimit {
                id: format!("claim-limit-{}", index + 1),
                severity: "blocking".to_string(),
                claim: claim.clone(),
                reason: "source fidelity does not prove this claim".to_string(),
                evidence_ref: "source-proof://fidelity-class".to_string(),
            })
            .collect()
    }

    fn accepted_dataset() -> AcceptedDataset {
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.jsonl".to_string(),
            source_url: SOURCE_URL.to_string(),
            sha256: OBJECT_SHA256.to_string(),
            bytes: 4096,
            archive_date: "2026-05-22".to_string(),
            schema_columns: vec!["l2_snapshot_jsonl".to_string()],
        };
        let forbidden_claims = vec!["No execution-quality claims.".to_string()];
        let checks = |evidence: &str| RequiredChecks {
            source_access: RequiredCheck::passed(evidence),
            license: RequiredCheck::passed("attestation"),
            schema: RequiredCheck::passed("schema"),
            time_semantics: RequiredCheck::passed("ms_to_nanos"),
            instrument_universe: RequiredCheck::passed("universe"),
            coverage: RequiredCheck::passed(evidence),
            retention_freshness: RequiredCheck::passed("retention"),
            granularity: RequiredCheck::passed("l2_snapshot"),
            completeness: RequiredCheck::passed(evidence),
            nt_mapping: RequiredCheck::passed("OrderBookDelta"),
            cost: RequiredCheck::passed("free"),
            storage: RequiredCheck::passed("artifact_root"),
        };
        let proof = SourceProofReport {
            source_proof_id: "source-proof-synthetic-deltas".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "testvenue-deltas".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            table_family: "order_book_snapshot_deltas".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            source_candidate_class: SourceCandidateClass::OfficialFree,
            source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
            fixture_type: FixtureType::BinaryOption,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-05-22T00:00:00Z".to_string(),
                end_utc: "2026-05-23T00:00:00Z".to_string(),
            },
            instrument_universe_id: "testvenue-deltas-instruments-2026-05-22".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
            schema_sample_hash: "bf26db".to_string(),
            license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
            license_scope: LicenseScope::Public,
            retention_ref: "https://synthetic.invalid/".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::L2Replay,
            l2_replay_evidence: L2ReplayEvidence {
                order_book_delta_ref: Some("source-proof://order-book-deltas".to_string()),
                sufficient_snapshot_cadence_ref: None,
                no_tick_size_change_universe_ref: Some(
                    "source-proof://no-tick-size-change-universe".to_string(),
                ),
                timed_instrument_epoch_replay_ref: None,
            },
            forbidden_claims: forbidden_claims.clone(),
            claim_limits: claim_limits_for(&forbidden_claims),
            cross_market_components: Vec::new(),
            acceptance_scope: Some(AcceptanceScope {
                planned_objects: 1,
                completed_objects: 1,
                failed_objects: 0,
                skipped_objects: 0,
                accepted_bytes: object.bytes,
                selector_scope_violations: 0,
            }),
            gap_policy_id: String::new(),
            required_checks: checks("manifest://synthetic"),
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
        .accept_with_registry(
            &source_binding_registry(),
            AcceptanceMode::Manual,
            "operator",
            "2026-06-02T00:00:00Z",
        )
        .expect("accept source proof");
        select_accepted_dataset_with_registry(
            &proof,
            &object,
            &object.sha256,
            &source_binding_registry(),
        )
        .expect("select accepted dataset")
    }

    fn identity(instrument: &str) -> CanonicalInstrumentIdentity {
        CanonicalInstrumentIdentity {
            instrument_id: instrument.to_string(),
            venue_symbol: instrument.to_string(),
            nt_instrument_id: format!("{instrument}.TESTVENUE"),
        }
    }

    fn single_identity() -> DeltaInstrumentIdentities {
        DeltaInstrumentIdentities::Single(identity("BASEQUOTE"))
    }

    fn single_mapping() -> DeltaMappingConfig {
        DeltaMappingConfig {
            format: DeltaSourceFormat::Snapshot {
                bids_field: "bids".to_string(),
                asks_field: "asks".to_string(),
                level_price_field: "px".to_string(),
                level_size_field: "sz".to_string(),
                event_time_field: "time".to_string(),
                event_time_unit: CsvTimestampUnit::Milliseconds,
            },
            instrument_key: InstrumentKeySpec {
                key_field: None,
                exclusion_filter: None,
            },
            ordering: OrderingAuthority::EventTime,
            price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
            empty_book_policy: EmptyBookPolicy::LoneClearLast,
        }
    }

    fn keyed_mapping(exclusion: Option<InstrumentExclusionFilter>) -> DeltaMappingConfig {
        DeltaMappingConfig {
            instrument_key: InstrumentKeySpec {
                key_field: Some("coin".to_string()),
                exclusion_filter: exclusion,
            },
            ..single_mapping()
        }
    }

    // Two full photos one minute apart: bid at 0.49/10, ask at 0.51/12.
    const SINGLE_JSONL: &str = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
        {\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";

    #[test]
    fn normalizes_single_instrument_snapshots() {
        let accepted = accepted_dataset();
        let tables = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            SINGLE_JSONL,
            42,
            "ingest-run-test",
        )
        .expect("normalize single-instrument snapshots");
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        // Two photos, each CLEAR + 1 bid ADD + 1 ask ADD = 3 rows => 6 rows.
        assert_eq!(table.rows.len(), 6);
        assert_eq!(table.partition.dt, "2026-05-22");

        let snapshot = RecordFlag::F_SNAPSHOT as u8;
        let last = RecordFlag::F_LAST as u8;
        assert_eq!(table.rows[0].action, DeltaAction::Clear.as_str());
        assert_eq!(table.rows[0].flags & snapshot, snapshot);
        assert_eq!(table.rows[0].flags & last, 0);
        assert_eq!(table.rows[1].action, DeltaAction::Add.as_str());
        assert_eq!(table.rows[1].side, DeltaSide::Buy.as_str());
        assert_eq!(table.rows[1].price, "0.49");
        assert_eq!(table.rows[1].size, "10");
        assert_eq!(table.rows[2].side, DeltaSide::Sell.as_str());
        assert_ne!(table.rows[2].flags & last, 0);
        for (index, row) in table.rows.iter().enumerate() {
            assert_eq!(row.sequence, index as u64);
        }
        assert_eq!(table.rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(table.rows[3].event_time, 1_700_000_060_000_000_000);
        assert_eq!(table.rows[0].capture_time, 42);
        assert_eq!(table.rows[0].ingest_run_id, "ingest-run-test");
        assert_eq!(
            table.rows[1].canonical_instrument_key,
            "testvenue/prediction-market/BASEQUOTE"
        );
        assert_eq!(table.rows[0].payload_hash, OBJECT_SHA256);
        assert_eq!(
            table.rows[0].transform_hash,
            delta_transform_hash(DELTAS_TRANSFORM_IDENTITY)
        );
        assert_eq!(
            table.rows[0].nt_instrument_id.as_deref(),
            Some("BASEQUOTE.TESTVENUE")
        );
    }

    #[test]
    fn empty_snapshot_emits_lone_clear_with_f_last() {
        let accepted = accepted_dataset();
        let jsonl = "{\"time\":1700000000000,\"bids\":[],\"asks\":[]}\n";
        let tables = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize empty snapshot");
        let table = &tables[0];
        assert_eq!(table.rows.len(), 1);
        let row = &table.rows[0];
        assert_eq!(row.action, DeltaAction::Clear.as_str());
        assert_ne!(row.flags & RecordFlag::F_SNAPSHOT as u8, 0);
        assert_ne!(row.flags & RecordFlag::F_LAST as u8, 0);
        assert!(row.side.is_empty());
        assert!(row.price.is_empty());
        assert!(row.size.is_empty());
    }

    #[test]
    fn collapses_consecutive_empty_snapshots() {
        let accepted = accepted_dataset();
        // Empty, empty, then a populated photo: the two empties collapse to one
        // lone CLEAR, then the populated photo expands normally.
        let jsonl = "{\"time\":1700000000000,\"bids\":[],\"asks\":[]}\n\
            {\"time\":1700000060000,\"bids\":[],\"asks\":[]}\n\
            {\"time\":1700000120000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let tables = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize collapses consecutive empties");
        let table = &tables[0];
        // One lone CLEAR (both empties collapsed) + bid/ask ADDs from the
        // populated photo, which skips its own CLEAR because the book is
        // already provably empty = 3 rows with no adjacent CLEARs.
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.rows[0].action, DeltaAction::Clear.as_str());
        // The lone CLEAR closes its own event.
        assert_ne!(table.rows[0].flags & RecordFlag::F_LAST as u8, 0);
        // The populated photo expands as ADDs over the established-empty book.
        assert_eq!(table.rows[1].action, DeltaAction::Add.as_str());
        assert_eq!(table.rows[1].flags & RecordFlag::F_LAST as u8, 0);
        assert_eq!(table.rows[2].action, DeltaAction::Add.as_str());
        // The ADDs-only event still closes with F_LAST.
        assert_ne!(table.rows[2].flags & RecordFlag::F_LAST as u8, 0);
        table
            .validate()
            .expect("collapsed table satisfies the contract");
    }

    #[test]
    fn splits_multi_instrument_object_per_key() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("BASEONE")),
            ("BBB".to_string(), identity("BASETWO")),
        ]));
        let jsonl = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
            {\"coin\":\"BBB\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.30\",\"sz\":\"5\"}],\"asks\":[{\"px\":\"0.33\",\"sz\":\"7\"}]}\n";
        let mut tables = normalize_jsonl_snapshot_deltas(
            &accepted,
            &identities,
            &keyed_mapping(None),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize multi-instrument object");
        tables.sort_by(|left, right| {
            left.partition
                .instrument_id
                .cmp(&right.partition.instrument_id)
        });
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].partition.instrument_id, "BASEONE");
        assert_eq!(tables[1].partition.instrument_id, "BASETWO");
        // Sequence restarts at 0 per table.
        assert_eq!(tables[0].rows[0].sequence, 0);
        assert_eq!(tables[1].rows[0].sequence, 0);
    }

    #[test]
    fn exclusion_filter_fences_matching_keys() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
            "AAA".to_string(),
            identity("BASE"),
        )]));
        let exclusion = InstrumentExclusionFilter {
            exclude_if_contains: vec![":".to_string(), "/".to_string()],
            exclude_if_prefix: vec!["@".to_string()],
        };
        // "AAA" survives; "dex:AAA", "@7", "AAA/BBB" are all fenced out.
        let jsonl = "{\"coin\":\"dex:AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
            {\"coin\":\"@7\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
            {\"coin\":\"AAA/BBB\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
            {\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let tables = normalize_jsonl_snapshot_deltas(
            &accepted,
            &identities,
            &keyed_mapping(Some(exclusion)),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize fences excluded keys");
        // Only the un-fenced "AAA" key survives to a table.
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].partition.instrument_id, "BASE");
    }

    #[test]
    fn rejects_non_positive_level() {
        let accepted = accepted_dataset();
        let price_zero = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            price_zero,
            42,
            "ingest-run-test",
        )
        .expect_err("non-positive price must be rejected");
        assert!(err.to_string().contains("non-positive price"), "{err}");

        let size_zero = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"0\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            size_zero,
            42,
            "ingest-run-test",
        )
        .expect_err("non-positive size must be rejected");
        assert!(err.to_string().contains("non-positive size"), "{err}");
    }

    #[test]
    fn rejects_regressing_event_time() {
        let accepted = accepted_dataset();
        let jsonl = "{\"time\":1700000060000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n\
            {\"time\":1700000000000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("regressing event time must be rejected");
        assert!(err.to_string().contains("precedes previous"), "{err}");
    }

    #[test]
    fn rejects_unknown_instrument_key_with_keyed_identities() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
            "AAA".to_string(),
            identity("BASE"),
        )]));
        let jsonl = "{\"coin\":\"ZZZ\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &identities,
            &keyed_mapping(None),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("unknown instrument key must be rejected");
        assert!(err.to_string().contains("no instrument identity"), "{err}");
    }

    // ── Fix 1: exclusion-filter empty-needle validation ───────────────────────

    #[test]
    fn rejects_empty_exclude_if_contains_needle() {
        let accepted = accepted_dataset();
        let filter = InstrumentExclusionFilter {
            exclude_if_contains: vec!["".to_string()],
            exclude_if_prefix: vec![],
        };
        let jsonl = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
                "AAA".to_string(),
                identity("BASE"),
            )])),
            &keyed_mapping(Some(filter)),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("empty exclude_if_contains needle must be rejected");
        assert!(
            err.to_string()
                .contains("exclusion_filter.exclude_if_contains must not contain empty needles"),
            "{err}"
        );
    }

    #[test]
    fn rejects_empty_exclude_if_prefix_needle() {
        let accepted = accepted_dataset();
        let filter = InstrumentExclusionFilter {
            exclude_if_contains: vec![],
            exclude_if_prefix: vec!["".to_string()],
        };
        let jsonl = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
                "AAA".to_string(),
                identity("BASE"),
            )])),
            &keyed_mapping(Some(filter)),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("empty exclude_if_prefix needle must be rejected");
        assert!(
            err.to_string()
                .contains("exclusion_filter.exclude_if_prefix must not contain empty prefixes"),
            "{err}"
        );
    }

    // ── Fix 2: missing / non-array side-field negative tests ─────────────────

    #[test]
    fn rejects_missing_bids_side_field() {
        let accepted = accepted_dataset();
        // Photo has no "bids" key at all.
        let jsonl = "{\"time\":1700000000000,\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("missing bids field must be rejected");
        assert!(
            err.to_string().contains("missing side field"),
            "expected 'missing side field' in: {err}"
        );
    }

    #[test]
    fn rejects_non_array_bids_side_field() {
        let accepted = accepted_dataset();
        // "bids" is a string, not an array.
        let jsonl = "{\"time\":1700000000000,\"bids\":\"not-an-array\",\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("non-array bids field must be rejected");
        assert!(
            err.to_string().contains("is not an array"),
            "expected 'is not an array' in: {err}"
        );
    }

    // ── Fix 3: JSON-parse surface negative tests ──────────────────────────────

    #[test]
    fn rejects_malformed_json_line() {
        let accepted = accepted_dataset();
        let jsonl = "not valid json\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("malformed JSON must be rejected");
        assert!(
            err.to_string().contains("malformed snapshot JSON"),
            "expected 'malformed snapshot JSON' in: {err}"
        );
    }

    #[test]
    fn rejects_missing_event_time_field() {
        let accepted = accepted_dataset();
        // Photo is valid JSON but has no "time" key.
        let jsonl = "{\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("missing event time field must be rejected");
        assert!(
            err.to_string().contains("missing event time field"),
            "expected 'missing event time field' in: {err}"
        );
    }

    #[test]
    fn rejects_missing_instrument_key_field_in_keyed_mode() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
            "AAA".to_string(),
            identity("BASE"),
        )]));
        // Photo has no "coin" key; keyed mode requires it.
        let jsonl = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &identities,
            &keyed_mapping(None),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("missing instrument key field must be rejected in keyed mode");
        assert!(
            err.to_string()
                .contains("missing string instrument key field"),
            "expected 'missing string instrument key field' in: {err}"
        );
    }

    // ── Fix 4: DeltaInstrumentIdentities::resolve mismatch negative tests ────

    #[test]
    fn rejects_non_none_key_with_single_identity() {
        let identity_single = single_identity();
        let err = identity_single
            .resolve(Some("AAA"))
            .expect_err("Single identity given non-None key must fail");
        assert!(
            err.to_string()
                .contains("single-instrument identities cannot resolve instrument key"),
            "expected mismatch message in: {err}"
        );
    }

    #[test]
    fn rejects_none_key_with_keyed_identity() {
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([(
            "AAA".to_string(),
            identity("BASE"),
        )]));
        // Passing None simulates "no key_field configured" for a Keyed shape.
        let err = identities
            .resolve(None)
            .expect_err("Keyed identity given None key must fail");
        assert!(
            err.to_string()
                .contains("keyed instrument identities require a configured key_field"),
            "expected key_field message in: {err}"
        );
    }

    // ── Fix 6: transform_hash pinning test ────────────────────────────────────

    /// Pin the JSONL snapshot adapter's `transform_hash` to its exact SHA-256
    /// hex. Any change to [`DELTAS_TRANSFORM_IDENTITY`] will break this test,
    /// which is intentional: the identity is part of the provenance contract and
    /// must be changed deliberately.
    #[test]
    fn jsonl_snapshot_delta_transform_hash_is_stable() {
        // SHA-256("jsonl-snapshot-deltas-to-canonical-order-book-deltas.v1")
        let expected = "3a06800b8fb1971b991255cde14c031dedb02de2fe16daf3d08af9cc6b0882f7";
        assert_eq!(
            delta_transform_hash(DELTAS_TRANSFORM_IDENTITY),
            expected,
            "DELTAS_TRANSFORM_IDENTITY hash changed — update the expected value \
             and bump the transform version if this is intentional"
        );
    }
}
