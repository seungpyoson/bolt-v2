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
        column_index,
    },
    source_proof::AcceptedDataset,
};

/// Re-exported so callers that import from this module's namespace can pass the
/// CSV bar transform identity to [`normalize_csv_native_bars`] without a
/// separate import of [`canonical_trades`].
pub use super::canonical_trades::BAR_TRANSFORM_IDENTITY;

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

/// Lowercase SHA-256 hex of an arbitrary transform identity string.
///
/// This is the parameterized core used by every bar adapter to derive its
/// per-row `transform_hash` from the adapter's own registry identity.  Each
/// adapter entry point passes its own identity constant so that the shared
/// assembly path never hardcodes a single adapter's identity.
#[must_use]
pub fn compute_bar_transform_hash(identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    hex::encode(hasher.finalize())
}

/// Lowercase SHA-256 hex of the CSV bar transform identity.
///
/// Convenience wrapper for the CSV native-bar adapter.  The hash value is
/// pinned by `csv_adapter_transform_hash_is_stable` so that any inadvertent
/// identity-string change is caught before it reaches the catalog.
#[must_use]
pub fn bar_transform_hash() -> String {
    compute_bar_transform_hash(BAR_TRANSFORM_IDENTITY)
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
    transform_identity: &str,
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

    // Determine the bar period from the interval source.
    //
    // Declared: the operator-specified period is authoritative. Each
    // instrument's adjacent open-time gaps must each be a positive integer
    // multiple of the declared period (gaps represent missing bars). A
    // single-bar instrument is valid — one bar cannot prove a gap, but the
    // declared period makes the period unambiguous.
    //
    // DerivedFromOpenTimes: the period is derived per instrument from its own
    // open times. Each instrument must have at least two rows so the minimum
    // adjacent gap can be found; if any instrument has only one row the
    // operator must declare the interval explicitly. Every instrument's gaps
    // must be integer multiples of that instrument's minimum gap. All
    // per-instrument derived specs must agree (a single canonical spec for the
    // whole object). GCD across instruments is not used; only the per-instrument
    // minimum gap drives the spec.
    let (bar_spec, interval_nanos) = match &mapping.interval_source {
        BarIntervalSource::Declared { step, aggregation } => {
            let declared = CanonicalBarSpec {
                step: *step,
                aggregation: *aggregation,
            };
            let interval_ms = bar_interval_ms(declared)?;
            let interval_nanos = i64::try_from(
                interval_ms
                    .checked_mul(NANOS_PER_MILLISECOND)
                    .context("declared bar interval overflows nanoseconds")?,
            )
            .context("declared bar interval overflows i64")?;
            let period =
                u64::try_from(interval_nanos).context("declared bar interval is non-positive")?;
            // Validate every instrument's gaps are positive integer multiples
            // of the declared period. Single-bar instruments are valid.
            for instrument_key in &group_order {
                let rows = groups
                    .get(instrument_key)
                    .context("internal: group_order key absent from groups")?;
                let mut opens: Vec<i64> = rows.iter().map(|row| row.open_time).collect();
                opens.sort_unstable();
                opens.dedup();
                for window in opens.windows(2) {
                    let gap = u64::try_from(
                        window[1]
                            .checked_sub(window[0])
                            .context("open_time underflow in declared-interval gap check")?,
                    )
                    .context("negative open_time gap in declared-interval gap check")?;
                    ensure!(
                        gap > 0,
                        "duplicate open_time survived dedup for instrument {instrument_key:?}"
                    );
                    ensure!(
                        gap.is_multiple_of(period),
                        "instrument {instrument_key:?}: open_time gap {gap} ns is not a \
                         multiple of the declared interval {period} ns — row is misaligned, \
                         not a data hole"
                    );
                }
            }
            (declared, interval_nanos)
        }
        BarIntervalSource::DerivedFromOpenTimes => {
            // Derive the spec per instrument. All must agree.
            let mut object_spec: Option<CanonicalBarSpec> = None;
            for instrument_key in &group_order {
                let rows = groups
                    .get(instrument_key)
                    .context("internal: group_order key absent from groups")?;
                let mut opens: Vec<i64> = rows.iter().map(|row| row.open_time).collect();
                opens.sort_unstable();
                opens.dedup();
                ensure!(
                    opens.len() >= 2,
                    "instrument {instrument_key:?} has only {} bar row(s) — cannot derive the \
                     period from a single open time; declare the interval explicitly via \
                     interval_source = \"declared\"",
                    opens.len()
                );
                let mut gaps: Vec<u64> = Vec::with_capacity(opens.len() - 1);
                for window in opens.windows(2) {
                    let gap = u64::try_from(
                        window[1]
                            .checked_sub(window[0])
                            .context("open_time underflow in derived-interval gap check")?,
                    )
                    .context("negative open_time gap in derived-interval gap check")?;
                    ensure!(gap > 0, "duplicate open_time survived dedup");
                    gaps.push(gap);
                }
                let min_gap = gaps
                    .iter()
                    .copied()
                    .min()
                    .context("internal: multi-row instrument yielded no open-time gaps")?;
                for gap in &gaps {
                    ensure!(
                        gap.is_multiple_of(min_gap),
                        "instrument {instrument_key:?}: open_time gap {gap} ns is not a \
                         multiple of the minimum gap {min_gap} ns — gaps must be integer \
                         multiples of the base interval (missing bars are allowed; \
                         non-multiples indicate mixed bar sizes)"
                    );
                }
                let min_gap_ms = min_gap
                    .checked_div(NANOS_PER_MILLISECOND)
                    .filter(|_| min_gap.is_multiple_of(NANOS_PER_MILLISECOND))
                    .context("derived bar interval is not a whole number of milliseconds")?;
                let instrument_spec = bar_spec_from_interval_ms(min_gap_ms)?;
                match &object_spec {
                    None => object_spec = Some(instrument_spec),
                    Some(existing) => {
                        ensure!(
                            *existing == instrument_spec,
                            "instrument {instrument_key:?} derived bar spec {instrument_spec:?} \
                             disagrees with object spec {existing:?} — all instruments must \
                             have the same bar period; declare the interval explicitly if the \
                             instruments have different granularities"
                        );
                    }
                }
            }
            let bar_spec =
                object_spec.context("internal: no instrument groups to derive a bar spec from")?;
            let interval_ms = bar_interval_ms(bar_spec)?;
            let interval_nanos = i64::try_from(
                interval_ms
                    .checked_mul(NANOS_PER_MILLISECOND)
                    .context("derived bar interval overflows nanoseconds")?,
            )
            .context("derived bar interval overflows i64")?;
            (bar_spec, interval_nanos)
        }
        BarIntervalSource::FromColumn { .. } => {
            bail!("internal: from_column interval source reached after pre-parse rejection")
        }
    };

    let canonical_instrument_key_prefix = format!("{}/{}", accepted.venue, accepted.product_family);
    let transform_hash = compute_bar_transform_hash(transform_identity);

    let mut tables = Vec::with_capacity(group_order.len());
    for instrument_key in &group_order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let canonical_instrument_key = format!(
            "{canonical_instrument_key_prefix}/{}",
            identity.instrument_id
        );
        let parsed_rows = groups
            .remove(instrument_key)
            .context("internal: group_order key absent from groups")?;
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
        if let Some(last) = deduped.last()
            && last.open_time == row.open_time
        {
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
            BAR_TRANSFORM_IDENTITY,
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
            BAR_TRANSFORM_IDENTITY,
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
            BAR_TRANSFORM_IDENTITY,
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
        // AAA carries two bars with 1-minute gaps; BBB carries one bar (no gap
        // to validate). With a declared 1-minute interval, single-bar BBB is valid.
        let csv = "instrument,open_time,close_time,open,high,low,close,volume\n\
            AAA,1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            AAA,1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            BBB,1700000000000,1700000060000,0.30,0.33,0.29,0.31,40\n";
        let mut tables = normalize_csv_native_bars(
            &accepted,
            &identities,
            &mapping,
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
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
        // Single-bar instrument receives the declared minute period.
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
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("from_column interval source must be rejected");
        assert!(err.to_string().contains("from_column"), "{err}");
        assert!(err.to_string().contains("format family"), "{err}");
    }

    #[test]
    fn rejects_declared_interval_misaligned_with_data_gaps() {
        let accepted = accepted_dataset(&schema_with_close());
        // Data has one-minute gaps; declaring one hour means 60s is not a
        // multiple of 3600s — the adapter must reject the row as misaligned.
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
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("misaligned gap must be rejected against declared interval");
        assert!(err.to_string().contains("not a multiple"), "{err}");
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
            BAR_TRANSFORM_IDENTITY,
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
            BAR_TRANSFORM_IDENTITY,
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
            BAR_TRANSFORM_IDENTITY,
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
            BAR_TRANSFORM_IDENTITY,
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
        let err = normalize_csv_native_bars(
            &accepted,
            &identities,
            &mapping,
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("unregistered instrument key must be rejected");
        assert!(err.to_string().contains("no instrument identity"), "{err}");
    }

    // ── Fix 3: close_time equality clause in dedup ────────────────────────────

    /// Two rows with the same open_time but different close_time values must be
    /// rejected even when all OHLCV fields are identical.  The dedup clause
    /// requires byte-identical rows; differing close_time alone must fail.
    #[test]
    fn rejects_disagreeing_close_time_with_identical_ohlcv() {
        let accepted = accepted_dataset(&schema_with_close());
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000000000,1700000999000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("disagreeing close_time with identical OHLCV must be rejected");
        assert!(
            err.to_string().contains("disagreeing"),
            "expected 'disagreeing' in error: {err}"
        );
    }

    // ── Fix 4: gap-multiple integrity negative tests ──────────────────────────

    /// A derived-interval object whose single instrument has gaps of 60 s and
    /// 90 s must fail: 90 s is not a multiple of 60 s.  The error must cite
    /// "not a multiple".
    #[test]
    fn rejects_derived_interval_misaligned_gaps() {
        let accepted = accepted_dataset(&schema_with_close());
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::DerivedFromOpenTimes,
            ..declared_minute_mapping()
        };
        // Gaps: 60 s then 90 s — 90 s is not a multiple of 60 s.
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            1700000150000,1700000210000,0.57,0.60,0.55,0.59,80\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("misaligned gaps must be rejected");
        assert!(
            err.to_string().contains("not a multiple"),
            "expected 'not a multiple' in error: {err}"
        );
    }

    /// A derived-interval object with a single bar row cannot derive its period.
    /// The adapter must fail loud and tell the operator to declare the interval.
    #[test]
    fn rejects_derived_interval_single_bar_without_declaration() {
        let accepted = accepted_dataset(&schema_with_close());
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::DerivedFromOpenTimes,
            ..declared_minute_mapping()
        };
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("single-bar derived-interval must be rejected");
        assert!(
            err.to_string().contains("declare the interval"),
            "expected 'declare the interval' in error: {err}"
        );
    }

    // ── Fix 5: header-vs-schema reconciliation negative test ─────────────────

    /// A CSV header that diverges from the accepted object's schema_columns must
    /// be rejected.  The error must reference "header" or "does not match".
    #[test]
    fn rejects_header_mismatch() {
        // Accepted schema expects the standard columns in schema_with_close order.
        let accepted = accepted_dataset(&schema_with_close());
        // CSV header has an extra column that is not in the accepted schema.
        let csv = "open_time,close_time,open,high,low,close,volume,unexpected_column\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100,extra\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("header mismatch must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("header") || msg.contains("does not match"),
            "expected 'header' or 'does not match' in error: {err}"
        );
    }

    // ── Fix 6: empty-string validation negative tests ─────────────────────────

    /// An empty ingest_run_id must be rejected.
    #[test]
    fn rejects_empty_ingest_run_id() {
        let accepted = accepted_dataset(&schema_with_close());
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            SINGLE_CSV_WITH_CLOSE,
            42,
            "",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("empty ingest_run_id must be rejected");
        assert!(
            err.to_string().contains("ingest_run_id"),
            "expected 'ingest_run_id' in error: {err}"
        );
    }

    /// An empty open_time_column name in the mapping must be rejected.
    #[test]
    fn rejects_empty_column_name_in_mapping() {
        let accepted = accepted_dataset(&schema_with_close());
        let mapping = BarMappingConfig {
            open_time_column: String::new(),
            ..declared_minute_mapping()
        };
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            SINGLE_CSV_WITH_CLOSE,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("empty open_time_column must be rejected");
        assert!(
            err.to_string().contains("open_time_column"),
            "expected 'open_time_column' in error: {err}"
        );
    }

    /// An empty instrument column value in a data row must be rejected.
    #[test]
    fn rejects_empty_instrument_key_value() {
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
        // Row has an empty string for the instrument column.
        let csv = "instrument,open_time,close_time,open,high,low,close,volume\n\
            ,1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &identities,
            &mapping,
            csv,
            42,
            "ingest-run-test",
            BAR_TRANSFORM_IDENTITY,
        )
        .expect_err("empty instrument column value must be rejected");
        assert!(
            err.to_string().contains("empty instrument"),
            "expected 'empty instrument' in error: {err}"
        );
    }

    /// Pin the CSV bar adapter's transform_hash to its current byte value.
    ///
    /// `normalize_csv_native_bars` passes `BAR_TRANSFORM_IDENTITY` to
    /// `compute_bar_transform_hash`.  The convenience wrapper `bar_transform_hash()`
    /// must produce the identical digest so that any inadvertent change to the
    /// identity string is caught before it reaches the catalog.
    #[test]
    fn csv_adapter_transform_hash_is_stable() {
        let via_param = compute_bar_transform_hash(BAR_TRANSFORM_IDENTITY);
        let via_wrapper = bar_transform_hash();
        assert_eq!(
            via_param, via_wrapper,
            "CSV adapter transform_hash diverged: param={via_param:?} wrapper={via_wrapper:?}"
        );
        // Pin the current byte value so identity-string drift is caught immediately.
        // Computed from: sha256("csv-native-bars-to-canonical-bars.v1")
        assert_eq!(
            via_wrapper, "03abb9c288a4f54881aab0e60d6ca8e28c6872023dbbbc54cc2577fb5c5cdd75",
            "BAR_TRANSFORM_IDENTITY hash changed — update this pin or revert the identity change"
        );
    }
}
