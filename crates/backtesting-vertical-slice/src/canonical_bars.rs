//! Gate 2 — config-driven CSV bar source adapter (format family F1).
//!
//! Normalizes an accepted CSV object of externally-aggregated OHLCV bars into
//! the `bars` table family of the `backfill-table-contract.v1` contract,
//! emitting one [`CanonicalBarsTable`] per instrument carried in the object.
//!
//! This is the bar-family sibling of [`super::canonical_trades`]: it reuses the
//! same column-mapping discipline (header reconcile against the accepted
//! object schema, headerless objects carry their schema in `schema_columns`,
//! `column_index` resolution, [`super::canonical_trades::CsvTimestampUnit`]
//! parsing) and the same identity/provenance header shape, and it preserves the
//! exact source OHLCV strings so the catalog projection in
//! [`super::catalog_projection`] is the single bridge from accepted evidence to
//! the NautilusTrader catalog.
//!
//! The bar period is a per-object property of the source granularity. Objects
//! that stage no interval in the row or filename (format family F1) recover the
//! period from the data: the interval is the smallest positive gap between
//! consecutive distinct bar-open timestamps across every instrument in the
//! object, and every gap must be an exact positive multiple of it. A single-bar
//! instrument cannot prove a period on its own but inherits the object's.
//!
//! Input is only ever an [`AcceptedDataset`] from gate 1 — raw staged data never
//! reaches this module without first passing source-proof acceptance.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use nautilus_model::enums::BarAggregation;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_market_data::{
        CanonicalBarRow, CanonicalBarSpec, CanonicalBarsTable, NORMALIZED_SCHEMA_VERSION,
    },
    canonical_trades::{
        BAR_TRANSFORM_IDENTITY, CanonicalInstrumentIdentity, CsvTimestampUnit, TradesPartition,
    },
    source_proof::AcceptedDataset,
};

const NANOS_PER_MILLISECOND: u64 = 1_000_000;

/// Fixed-duration NautilusTrader bar units, longest first, paired with their
/// millisecond length. Used both to express an observed candle interval as the
/// NautilusTrader `(step, unit)` pair and to recover the interval duration when
/// an object carries no close-time column. Sub-second and calendar-variable
/// units (month/year) are deliberately excluded: a uniform millisecond gap
/// cannot honestly prove one, so such an interval fails loud instead.
const BAR_UNITS_MS: [(BarAggregation, u64); 5] = [
    (BarAggregation::Week, 7 * 24 * 60 * 60 * 1_000),
    (BarAggregation::Day, 24 * 60 * 60 * 1_000),
    (BarAggregation::Hour, 60 * 60 * 1_000),
    (BarAggregation::Minute, 60 * 1_000),
    (BarAggregation::Second, 1_000),
];

/// Run-spec owned CSV-bar column mapping for the F1 source adapter.
///
/// A new source that emits the same headerless-or-headed OHLCV CSV shape selects
/// the bar converter from TOML and supplies its column mapping here. Mirrors
/// [`super::canonical_trades::CsvTradeMappingConfig`]: the timestamp unit is the
/// shared [`CsvTimestampUnit`], and column names are resolved against the
/// accepted object schema rather than positionally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarMappingConfig {
    pub has_headers: bool,
    pub open_time_column: String,
    pub close_time_column: String,
    pub timestamp_unit: CsvTimestampUnit,
    pub open_column: String,
    pub high_column: String,
    pub low_column: String,
    pub close_column: String,
    pub volume_column: String,
    /// Column whose value keys the per-row instrument in a multi-instrument
    /// object. `None` selects the single-instrument object shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_column: Option<String>,
    pub interval_source: BarIntervalSource,
    pub price_sign_policy: BarPriceSignPolicy,
}

/// How the bar period (step + aggregation) is determined for an object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BarIntervalSource {
    /// The run-spec declares the period; the data-derived period must equal it.
    Declared {
        step: usize,
        aggregation: BarAggregation,
    },
    /// The period is recovered from the spacing of distinct bar-open times.
    DerivedFromOpenTimes,
    /// The period is carried in a per-row column. Rejected by this adapter:
    /// per-row interval columns are a different format family (F3, JSONL
    /// multi-interval), not the single-interval F1 CSV shape.
    FromColumn { interval_column: String },
}

/// Sign policy for bar OHLC prices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarPriceSignPolicy {
    /// Every open/high/low/close must be strictly positive.
    StrictlyPositive,
}

/// Instrument-identity resolution for a bar object.
///
/// A single-instrument object binds one identity to every row; a
/// multi-instrument object keys identities by the value of the configured
/// `instrument_column`. Built by the caller from accepted instrument-universe
/// data, so no instrument identity is hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarInstrumentIdentities {
    Single(CanonicalInstrumentIdentity),
    Keyed(BTreeMap<String, CanonicalInstrumentIdentity>),
}

impl BarInstrumentIdentities {
    /// Resolve the identity for one row, given the configured instrument-column
    /// value (`None` for the single-instrument shape).
    fn resolve(&self, instrument_key: Option<&str>) -> Result<&CanonicalInstrumentIdentity> {
        match self {
            Self::Single(identity) => {
                ensure!(
                    instrument_key.is_none(),
                    "single-instrument identities cannot resolve instrument-column key {:?}",
                    instrument_key
                );
                Ok(identity)
            }
            Self::Keyed(identities) => {
                let key = instrument_key.context(
                    "keyed instrument identities require a configured instrument_column",
                )?;
                identities.get(key).with_context(|| {
                    format!("no instrument identity registered for instrument key {key:?}")
                })
            }
        }
    }
}

/// Lowercase SHA-256 hex of the bar transform identity.
#[must_use]
pub fn bar_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(BAR_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// Express a fixed millisecond interval as the NautilusTrader `(step, unit)`
/// pair using the largest fixed-duration unit that divides it evenly (60_000 ms
/// -> step 1 [`BarAggregation::Minute`]; 300_000 ms -> step 5 minute;
/// 3_600_000 ms -> step 1 [`BarAggregation::Hour`]).
///
/// # Errors
///
/// Returns an error if `interval_ms` is zero or is not an exact multiple of any
/// fixed-duration unit down to one second (sub-second or calendar-variable
/// intervals are not honestly representable from a uniform millisecond gap).
fn bar_spec_from_interval_ms(interval_ms: u64) -> Result<CanonicalBarSpec> {
    ensure!(interval_ms > 0, "bar interval is zero");
    for (aggregation, unit_ms) in BAR_UNITS_MS {
        if interval_ms.is_multiple_of(unit_ms) {
            let step = usize::try_from(interval_ms / unit_ms).context("bar step overflow")?;
            return Ok(CanonicalBarSpec { step, aggregation });
        }
    }
    bail!(
        "bar interval {interval_ms} ms is not a whole number of seconds; \
         cannot derive a bar unit"
    )
}

/// Millisecond length of one `(step, aggregation)` bar period.
///
/// # Errors
///
/// Returns an error for a non-fixed-duration aggregation or on overflow.
fn bar_interval_ms(spec: CanonicalBarSpec) -> Result<u64> {
    let unit_ms = BAR_UNITS_MS
        .iter()
        .find_map(|(aggregation, unit_ms)| (*aggregation == spec.aggregation).then_some(*unit_ms))
        .with_context(|| {
            format!(
                "bar aggregation {:?} is not a fixed-duration unit",
                spec.aggregation
            )
        })?;
    let step = u64::try_from(spec.step).context("bar step overflow")?;
    step.checked_mul(unit_ms)
        .context("bar interval overflows milliseconds")
}

/// Derive the [`CanonicalBarSpec`] (step + aggregation) from a set of bar
/// `open_time` values (Unix nanoseconds).
///
/// The interval is the smallest positive gap between consecutive distinct
/// bar-open timestamps, and every gap must be an exact positive multiple of it
/// (a larger gap is a missing bar, never a different period). Fewer than two
/// distinct opens cannot prove a period and fails loud.
///
/// The caller controls the SCOPE of `open_times`. The per-object derivation
/// passes the union of every instrument's opens, because the period is a
/// per-object property of the source granularity (an illiquid instrument that
/// traded in a single bar cannot prove a period on its own but inherits the
/// object's). The input need not be sorted.
///
/// # Errors
///
/// Returns an error if fewer than two distinct bar-open times are present, a gap
/// is not a multiple of the base interval, or the interval is not representable
/// as a fixed-duration NautilusTrader bar unit.
pub fn bar_spec_from_open_times(open_times: &[i64]) -> Result<CanonicalBarSpec> {
    let mut times: Vec<i64> = open_times.to_vec();
    times.sort_unstable();
    times.dedup();
    ensure!(
        times.len() >= 2,
        "cannot derive bar interval from fewer than two distinct bar-open times"
    );

    let mut gaps: Vec<u64> = Vec::with_capacity(times.len() - 1);
    for window in times.windows(2) {
        let delta = window[1]
            .checked_sub(window[0])
            .context("bar-open time underflow")?;
        let delta = u64::try_from(delta).context("negative bar-open gap")?;
        ensure!(delta > 0, "duplicate bar-open time survived dedup");
        gaps.push(delta);
    }

    let base = *gaps.iter().min().expect("at least one gap");
    // The base nanosecond interval scaled to milliseconds for unit selection.
    let base_ms = base
        .checked_div(NANOS_PER_MILLISECOND)
        .filter(|_| base.is_multiple_of(NANOS_PER_MILLISECOND))
        .context("bar interval is not a whole number of milliseconds")?;
    for gap in &gaps {
        ensure!(
            gap.is_multiple_of(base),
            "bar gaps are not multiples of the base interval \
             ({gap} ns is not a multiple of {base} ns)"
        );
    }
    bar_spec_from_interval_ms(base_ms)
}

/// One parsed CSV bar, before identity/provenance assembly.
struct ParsedBarRow {
    instrument_key: Option<String>,
    open_time: i64,
    close_time: Option<i64>,
    open: String,
    high: String,
    low: String,
    close: String,
    volume: String,
}

/// Normalize an accepted CSV bar object into one [`CanonicalBarsTable`] per
/// instrument.
///
/// `csv_text` must be the decoded text of the accepted object whose hash already
/// matched the manifest (the caller verified it via gate 1).
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run that produced this
/// normalization, recorded for lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if the header does not match the accepted schema, a row is
/// malformed, a field fails to parse, the period cannot be derived or disagrees
/// with a declared period, an OHLC price is non-positive, or a produced table
/// fails its contract.
pub fn normalize_csv_native_bars(
    accepted: &AcceptedDataset,
    identities: &BarInstrumentIdentities,
    mapping: &BarMappingConfig,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalBarsTable>> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        !mapping.open_time_column.trim().is_empty(),
        "converter bars.open_time_column must not be empty"
    );
    ensure!(
        !mapping.close_time_column.trim().is_empty(),
        "converter bars.close_time_column must not be empty"
    );
    ensure!(
        !mapping.open_column.trim().is_empty(),
        "converter bars.open_column must not be empty"
    );
    ensure!(
        !mapping.high_column.trim().is_empty(),
        "converter bars.high_column must not be empty"
    );
    ensure!(
        !mapping.low_column.trim().is_empty(),
        "converter bars.low_column must not be empty"
    );
    ensure!(
        !mapping.close_column.trim().is_empty(),
        "converter bars.close_column must not be empty"
    );
    ensure!(
        !mapping.volume_column.trim().is_empty(),
        "converter bars.volume_column must not be empty"
    );
    if let Some(instrument_column) = &mapping.instrument_column {
        ensure!(
            !instrument_column.trim().is_empty(),
            "converter bars.instrument_column must not be empty when set"
        );
    }
    if let BarIntervalSource::FromColumn { interval_column } = &mapping.interval_source {
        bail!(
            "converter bars.interval_source kind \"from_column\" (column {interval_column:?}) is a \
             different format family (per-row multi-interval); the F1 CSV bar adapter requires \
             \"declared\" or \"derived_from_open_times\""
        );
    }

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(mapping.has_headers)
        .trim(csv::Trim::All)
        .from_reader(csv_text.as_bytes());
    let header_columns: Vec<String> = if mapping.has_headers {
        let header_columns = reader
            .headers()
            .context("empty csv: missing header")?
            .iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        ensure!(
            accepted.object.schema_columns == header_columns,
            "csv header {header_columns:?} does not match accepted object schema {:?}",
            accepted.object.schema_columns
        );
        header_columns
    } else {
        ensure!(
            !accepted.object.schema_columns.is_empty(),
            "accepted object schema columns must not be empty for headerless csv"
        );
        accepted.object.schema_columns.clone()
    };

    let open_time_index = column_index(&header_columns, &mapping.open_time_column)?;
    let open_index = column_index(&header_columns, &mapping.open_column)?;
    let high_index = column_index(&header_columns, &mapping.high_column)?;
    let low_index = column_index(&header_columns, &mapping.low_column)?;
    let close_index = column_index(&header_columns, &mapping.close_column)?;
    let volume_index = column_index(&header_columns, &mapping.volume_column)?;
    // close_time and instrument columns are optional: a close_time column that
    // resolves is used directly, otherwise close_time is derived from the
    // period; the instrument column is present only for multi-instrument
    // objects.
    let close_time_index = header_columns
        .iter()
        .position(|column| column == &mapping.close_time_column);
    let instrument_index = match &mapping.instrument_column {
        Some(instrument_column) => Some(column_index(&header_columns, instrument_column)?),
        None => None,
    };

    // Group parsed rows by instrument key (single-instrument objects use one
    // group keyed by `None`), preserving first-seen group order.
    let mut group_order: Vec<Option<String>> = Vec::new();
    let mut groups: BTreeMap<Option<String>, Vec<ParsedBarRow>> = BTreeMap::new();
    let mut object_open_times: Vec<i64> = Vec::new();

    for (index, record) in reader.records().enumerate() {
        let fields = record.with_context(|| format!("row {index}: malformed csv record"))?;
        if fields.iter().all(str::is_empty) {
            continue;
        }
        ensure!(
            fields.len() == header_columns.len(),
            "row {index} has {} fields, expected {}",
            fields.len(),
            header_columns.len()
        );

        let open_time_raw = fields.get(open_time_index).context("missing open_time")?;
        let open_time = mapping
            .timestamp_unit
            .parse_to_nanos(open_time_raw)
            .with_context(|| format!("row {index}: invalid open_time {open_time_raw:?}"))?;
        ensure!(open_time > 0, "row {index}: non-positive open_time");

        let close_time = match close_time_index {
            Some(close_time_index) => {
                let close_time_raw = fields.get(close_time_index).context("missing close_time")?;
                let close_time = mapping
                    .timestamp_unit
                    .parse_to_nanos(close_time_raw)
                    .with_context(|| {
                        format!("row {index}: invalid close_time {close_time_raw:?}")
                    })?;
                Some(close_time)
            }
            None => None,
        };

        let open = fields.get(open_index).context("missing open")?.to_string();
        let high = fields.get(high_index).context("missing high")?.to_string();
        let low = fields.get(low_index).context("missing low")?.to_string();
        let close = fields
            .get(close_index)
            .context("missing close")?
            .to_string();
        let volume = fields
            .get(volume_index)
            .context("missing volume")?
            .to_string();

        for (label, value) in [
            ("open", &open),
            ("high", &high),
            ("low", &low),
            ("close", &close),
            ("volume", &volume),
        ] {
            ensure!(!value.trim().is_empty(), "row {index}: empty {label}");
        }
        apply_price_sign_policy(index, mapping.price_sign_policy, &open, &high, &low, &close)?;

        let instrument_key = match instrument_index {
            Some(instrument_index) => {
                let raw = fields
                    .get(instrument_index)
                    .context("missing instrument column")?;
                ensure!(!raw.is_empty(), "row {index}: empty instrument column");
                Some(raw.to_string())
            }
            None => None,
        };

        object_open_times.push(open_time);
        let group = groups.entry(instrument_key.clone()).or_insert_with(|| {
            group_order.push(instrument_key.clone());
            Vec::new()
        });
        group.push(ParsedBarRow {
            instrument_key,
            open_time,
            close_time,
            open,
            high,
            low,
            close,
            volume,
        });
    }

    ensure!(!group_order.is_empty(), "bar object yielded no rows");

    // Derive the period ONCE for the whole object from the union of every
    // instrument's open times, so a single-bar instrument inherits the object's
    // proven period instead of aborting. A declared period must equal it.
    let bar_spec = bar_spec_from_open_times(&object_open_times)?;
    if let BarIntervalSource::Declared { step, aggregation } = &mapping.interval_source {
        let declared = CanonicalBarSpec {
            step: *step,
            aggregation: *aggregation,
        };
        ensure!(
            declared == bar_spec,
            "declared bar interval {declared:?} does not match interval derived from open times {bar_spec:?}"
        );
    }
    let interval_ms = bar_interval_ms(bar_spec)?;
    let interval_nanos = i64::try_from(
        interval_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .context("bar interval overflows nanoseconds")?,
    )
    .context("bar interval overflows i64")?;

    let canonical_instrument_key_prefix = format!("{}/{}", accepted.venue, accepted.product_family);
    let transform_hash = bar_transform_hash();

    let mut tables = Vec::with_capacity(group_order.len());
    for instrument_key in &group_order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let canonical_instrument_key = format!(
            "{canonical_instrument_key_prefix}/{}",
            identity.instrument_id
        );
        let parsed_rows = groups
            .remove(instrument_key)
            .expect("group order entry has a populated group");
        let parsed_rows = dedup_sorted_bar_rows(parsed_rows)?;

        let mut rows = Vec::with_capacity(parsed_rows.len());
        for parsed in parsed_rows {
            let close_time = match parsed.close_time {
                Some(close_time) => close_time,
                None => parsed
                    .open_time
                    .checked_add(interval_nanos)
                    .context("bar close_time overflows nanoseconds")?,
            };
            rows.push(CanonicalBarRow {
                schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
                ingest_run_id: ingest_run_id.to_string(),
                source_binding: accepted.source_binding.clone(),
                venue: accepted.venue.clone(),
                product_family: accepted.product_family.clone(),
                product_category: accepted.product_category.clone(),
                instrument_id: identity.instrument_id.clone(),
                canonical_instrument_key: canonical_instrument_key.clone(),
                venue_symbol: identity.venue_symbol.clone(),
                nt_instrument_id: Some(identity.nt_instrument_id.clone()),
                open_time: parsed.open_time,
                close_time,
                capture_time: capture_time_nanos,
                availability_time: None,
                source_sequence: Some(parsed.open_time.to_string()),
                raw_payload_id: accepted.object.sha256.clone(),
                source_proof_id: accepted.source_proof_id.clone(),
                payload_hash: accepted.object.sha256.clone(),
                transform_hash: transform_hash.clone(),
                open: parsed.open,
                high: parsed.high,
                low: parsed.low,
                close: parsed.close,
                volume: parsed.volume,
            });
        }

        let table = CanonicalBarsTable {
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
            bar_spec,
            rows,
        };
        table.validate()?;
        tables.push(table);
    }

    Ok(tables)
}

/// Collapse exact-duplicate `open_time` rows (always on) and sort ascending by
/// `open_time`.
///
/// A duplicate `open_time` is collapsed only when its OHLCV strings are
/// byte-identical to the row already kept; a disagreeing duplicate is a corrupt
/// object and fails loud.
fn dedup_sorted_bar_rows(mut rows: Vec<ParsedBarRow>) -> Result<Vec<ParsedBarRow>> {
    rows.sort_by_key(|row| row.open_time);
    let mut deduped: Vec<ParsedBarRow> = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some(last) = deduped.last() {
            if last.open_time == row.open_time {
                ensure!(
                    last.open == row.open
                        && last.high == row.high
                        && last.low == row.low
                        && last.close == row.close
                        && last.volume == row.volume
                        && last.close_time == row.close_time,
                    "duplicate open_time {} for instrument {:?} carries disagreeing OHLCV",
                    row.open_time,
                    row.instrument_key
                );
                continue;
            }
        }
        deduped.push(row);
    }
    Ok(deduped)
}

/// Enforce the OHLC sign policy for one parsed row.
///
/// `StrictlyPositive` rejects any non-positive open/high/low/close; non-negative
/// volume is left to [`CanonicalBarsTable::validate`].
fn apply_price_sign_policy(
    index: usize,
    policy: BarPriceSignPolicy,
    open: &str,
    high: &str,
    low: &str,
    close: &str,
) -> Result<()> {
    match policy {
        BarPriceSignPolicy::StrictlyPositive => {
            for (label, value) in [
                ("open", open),
                ("high", high),
                ("low", low),
                ("close", close),
            ] {
                let parsed: Decimal = value
                    .parse()
                    .with_context(|| format!("row {index}: invalid {label} {value:?}"))?;
                ensure!(
                    parsed > Decimal::ZERO,
                    "row {index}: non-positive {label} {value:?}"
                );
            }
            Ok(())
        }
    }
}

fn column_index(header_columns: &[String], column_name: &str) -> Result<usize> {
    header_columns
        .iter()
        .position(|column| column == column_name)
        .with_context(|| format!("configured converter column {column_name:?} missing from csv"))
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
key = "testvenue-bars"
venue = "testvenue"
product_family = "prediction-market"
market_structure_fixture = "binary-option"
source_uri = "https://synthetic.invalid/data"
evidence_state = "owner_archive_backfillable"
table_families = ["bars"]
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

    fn accepted_dataset(schema_columns: &[&str]) -> AcceptedDataset {
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://synthetic-artifacts/source-proofs/raw/object.csv".to_string(),
            source_url: SOURCE_URL.to_string(),
            sha256: OBJECT_SHA256.to_string(),
            bytes: 4096,
            archive_date: "2026-05-22".to_string(),
            schema_columns: schema_columns.iter().map(ToString::to_string).collect(),
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
            granularity: RequiredCheck::passed("aggregated_bars"),
            completeness: RequiredCheck::passed(evidence),
            nt_mapping: RequiredCheck::passed("Bar"),
            cost: RequiredCheck::passed("free"),
            storage: RequiredCheck::passed("artifact_root"),
        };
        let proof = SourceProofReport {
            source_proof_id: "source-proof-synthetic-bars".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "testvenue-bars".to_string(),
            venue: "testvenue".to_string(),
            product_family: "prediction-market".to_string(),
            product_category: "binary".to_string(),
            table_family: "bars".to_string(),
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
            instrument_universe_id: "testvenue-bars-instruments-2026-05-22".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s3://synthetic-artifacts/source-proofs/schema.json".to_string(),
            schema_sample_hash: "bf26db".to_string(),
            license_ref: "https://synthetic.invalid/ (attestation)".to_string(),
            license_scope: LicenseScope::Public,
            retention_ref: "https://synthetic.invalid/".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeBarReplay,
            l2_replay_evidence: L2ReplayEvidence {
                order_book_delta_ref: None,
                sufficient_snapshot_cadence_ref: None,
                no_tick_size_change_universe_ref: None,
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
        .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
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

    fn single_identity() -> BarInstrumentIdentities {
        BarInstrumentIdentities::Single(identity("BASEQUOTE"))
    }

    fn declared_minute_mapping() -> BarMappingConfig {
        BarMappingConfig {
            has_headers: true,
            open_time_column: "open_time".to_string(),
            close_time_column: "close_time".to_string(),
            timestamp_unit: CsvTimestampUnit::Milliseconds,
            open_column: "open".to_string(),
            high_column: "high".to_string(),
            low_column: "low".to_string(),
            close_column: "close".to_string(),
            volume_column: "volume".to_string(),
            instrument_column: None,
            interval_source: BarIntervalSource::Declared {
                step: 1,
                aggregation: BarAggregation::Minute,
            },
            price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
        }
    }

    // open_time/close_time are unix-ms; close_time is one minute after open.
    const SINGLE_CSV_WITH_CLOSE: &str = "open_time,close_time,open,high,low,close,volume\n\
        1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
        1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";

    fn schema_with_close() -> Vec<&'static str> {
        vec![
            "open_time",
            "close_time",
            "open",
            "high",
            "low",
            "close",
            "volume",
        ]
    }

    #[test]
    fn normalizes_single_instrument_bars_with_close_time_column() {
        let accepted = accepted_dataset(&schema_with_close());
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            SINGLE_CSV_WITH_CLOSE,
            42,
            "ingest-run-test",
        )
        .expect("normalize single-instrument bars");
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.bar_spec.step, 1);
        assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
        assert_eq!(table.partition.dt, "2026-05-22");
        let first = &table.rows[0];
        assert_eq!(first.open_time, 1_700_000_000_000_000_000);
        assert_eq!(first.close_time, 1_700_000_060_000_000_000);
        assert_eq!(first.capture_time, 42);
        assert_eq!(first.ingest_run_id, "ingest-run-test");
        assert_eq!(first.open, "0.50");
        assert_eq!(first.close, "0.52");
        assert_eq!(first.volume, "100");
        assert_eq!(
            first.canonical_instrument_key,
            "testvenue/prediction-market/BASEQUOTE"
        );
        assert_eq!(first.payload_hash, OBJECT_SHA256);
        assert_eq!(first.transform_hash, bar_transform_hash());
        assert_eq!(
            first.nt_instrument_id.as_deref(),
            Some("BASEQUOTE.TESTVENUE")
        );
    }

    #[test]
    fn derives_close_time_from_period_when_column_absent() {
        // Schema carries no close_time column; close_time is derived as
        // open_time + one derived minute.
        let accepted = accepted_dataset(&["open_time", "open", "high", "low", "close", "volume"]);
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::DerivedFromOpenTimes,
            ..declared_minute_mapping()
        };
        let csv = "open_time,open,high,low,close,volume\n\
            1700000000000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,0.52,0.58,0.51,0.57,120\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize bars deriving close_time");
        let table = &tables[0];
        assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
        assert_eq!(table.rows[0].open_time, 1_700_000_000_000_000_000);
        assert_eq!(table.rows[0].close_time, 1_700_000_060_000_000_000);
        assert_eq!(table.rows[1].close_time, 1_700_000_120_000_000_000);
    }

    #[test]
    fn normalizes_headerless_bars_using_configured_schema_columns() {
        // Headerless: the schema vector IS the column layout. The configured
        // close_time column name resolves to nothing here, so close time is
        // derived from the period.
        let accepted = accepted_dataset(&["open_time", "open", "high", "low", "close", "volume"]);
        let mapping = BarMappingConfig {
            has_headers: false,
            interval_source: BarIntervalSource::DerivedFromOpenTimes,
            ..declared_minute_mapping()
        };
        let csv = "1700000000000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,0.52,0.58,0.51,0.57,120\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize headerless bars");
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[0].rows[0].open_time, 1_700_000_000_000_000_000);
        assert_eq!(tables[0].rows[0].close_time, 1_700_000_060_000_000_000);
    }

    #[test]
    fn groups_multi_instrument_object_into_one_table_per_instrument() {
        let accepted = accepted_dataset(&[
            "instrument",
            "open_time",
            "close_time",
            "open",
            "high",
            "low",
            "close",
            "volume",
        ]);
        let mapping = BarMappingConfig {
            instrument_column: Some("instrument".to_string()),
            ..declared_minute_mapping()
        };
        let identities = BarInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("BASEONE")),
            ("BBB".to_string(), identity("BASETWO")),
        ]));
        // AAA carries two bars (proves the minute period); BBB carries one bar
        // and inherits the object-level period.
        let csv = "instrument,open_time,close_time,open,high,low,close,volume\n\
            AAA,1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            AAA,1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            BBB,1700000000000,1700000060000,0.30,0.33,0.29,0.31,40\n";
        let mut tables =
            normalize_csv_native_bars(&accepted, &identities, &mapping, csv, 42, "ingest-run-test")
                .expect("normalize multi-instrument bars");
        tables.sort_by(|left, right| {
            left.partition
                .instrument_id
                .cmp(&right.partition.instrument_id)
        });
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].partition.instrument_id, "BASEONE");
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[1].partition.instrument_id, "BASETWO");
        assert_eq!(tables[1].rows.len(), 1);
        // Single-bar instrument inherited the object's minute period.
        assert_eq!(tables[1].bar_spec.aggregation, BarAggregation::Minute);
    }

    #[test]
    fn rejects_from_column_interval_source_as_different_format_family() {
        let accepted = accepted_dataset(&schema_with_close());
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::FromColumn {
                interval_column: "interval".to_string(),
            },
            ..declared_minute_mapping()
        };
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            SINGLE_CSV_WITH_CLOSE,
            42,
            "ingest-run-test",
        )
        .expect_err("from_column interval source must be rejected");
        assert!(err.to_string().contains("from_column"), "{err}");
        assert!(err.to_string().contains("format family"), "{err}");
    }

    #[test]
    fn rejects_declared_interval_disagreeing_with_derived() {
        let accepted = accepted_dataset(&schema_with_close());
        // The data is a one-minute period but the run-spec declares one hour.
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::Declared {
                step: 1,
                aggregation: BarAggregation::Hour,
            },
            ..declared_minute_mapping()
        };
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            SINGLE_CSV_WITH_CLOSE,
            42,
            "ingest-run-test",
        )
        .expect_err("declared/derived interval mismatch must be rejected");
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    #[test]
    fn collapses_byte_identical_duplicate_open_time() {
        let accepted = accepted_dataset(&schema_with_close());
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize collapses byte-identical duplicate");
        assert_eq!(tables[0].rows.len(), 2);
    }

    #[test]
    fn rejects_disagreeing_duplicate_open_time() {
        let accepted = accepted_dataset(&schema_with_close());
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.99,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect_err("disagreeing duplicate open_time must be rejected");
        assert!(err.to_string().contains("disagreeing"), "{err}");
    }

    #[test]
    fn rejects_non_positive_price_under_strictly_positive_policy() {
        let accepted = accepted_dataset(&schema_with_close());
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0,0.52,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect_err("non-positive low must be rejected");
        assert!(err.to_string().contains("non-positive"), "{err}");
    }

    #[test]
    fn sorts_unsorted_rows_before_table_assembly() {
        // Rows arrive out of order; the adapter sorts by open_time before table
        // assembly, so the strictly-increasing-open-time contract still holds.
        let accepted = accepted_dataset(&schema_with_close());
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize sorts unsorted input");
        assert!(tables[0].rows[0].open_time < tables[0].rows[1].open_time);
    }

    #[test]
    fn rejects_unregistered_instrument_key() {
        let accepted = accepted_dataset(&[
            "instrument",
            "open_time",
            "close_time",
            "open",
            "high",
            "low",
            "close",
            "volume",
        ]);
        let mapping = BarMappingConfig {
            instrument_column: Some("instrument".to_string()),
            ..declared_minute_mapping()
        };
        let identities = BarInstrumentIdentities::Keyed(BTreeMap::from([(
            "AAA".to_string(),
            identity("BASEONE"),
        )]));
        let csv = "instrument,open_time,close_time,open,high,low,close,volume\n\
            AAA,1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            AAA,1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            ZZZ,1700000000000,1700000060000,0.30,0.33,0.29,0.31,40\n";
        let err =
            normalize_csv_native_bars(&accepted, &identities, &mapping, csv, 42, "ingest-run-test")
                .expect_err("unregistered instrument key must be rejected");
        assert!(err.to_string().contains("no instrument identity"), "{err}");
    }
}
