//! Gate 2 — config-driven bar source adapters (format families F1, F2, F3).
//!
//! Normalizes an accepted object of externally-aggregated OHLCV bars into the
//! `bars` table family of the `backfill-table-contract.v1` contract, emitting one
//! [`CanonicalBarsTable`] per `(instrument, interval)` series carried in the
//! object. Three wire shapes share this module:
//!
//! - **F1** — headerless-or-headed OHLCV CSV ([`normalize_csv_native_bars`]). The
//!   period is a per-object property recovered from the data (or declared and
//!   reconciled against it); one table per instrument.
//! - **F2** — paged REST JSON klines ([`normalize_paged_json_bars`]). Rows live
//!   inside a JSON envelope at a configured `rows_path`, arrive newest-first, and
//!   adjacent pages overlap in time; the declared period is reconciled against
//!   the deduped data; one table per (single) instrument.
//! - **F3** — line-delimited multi-interval klines
//!   ([`normalize_jsonl_multi_interval_bars`]). Each line carries its own interval
//!   token mapped through a config-side `interval_token_map`; rows group by
//!   `(instrument_key, interval token)` into one table per group, each with its
//!   own bar spec.
//!
//! All three reuse the same column/identity discipline, preserve the exact source
//! OHLCV strings, and assemble + validate through the shared
//! [`assemble_bar_table`] helper so the catalog projection in
//! [`super::catalog_projection`] is the single bridge from accepted evidence to
//! the NautilusTrader catalog.
//!
//! For F1, the bar period is recovered from the data: the interval is the
//! smallest positive gap between consecutive distinct bar-open timestamps across
//! every instrument in the object, and every gap must be an exact positive
//! multiple of it. A single-bar instrument cannot prove a period on its own but
//! inherits the object's.
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
        BAR_TRANSFORM_IDENTITY, CanonicalInstrumentIdentity, CsvTimestampUnit,
        JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY, PAGED_JSON_BARS_TRANSFORM_IDENTITY,
        TradesPartition, column_index,
    },
    operator_work_budget::{OperatorWorkBudgetGuard, OperatorWorkBudgetStage},
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

/// Fixed nanosecond length of one `(step, aggregation)` bar period, or `None`
/// when the period is calendar-variable (month/year) and has no fixed nanosecond
/// length.
///
/// Used by [`assemble_bar_table`] to derive a missing `close_time`: a
/// fixed-duration period yields `Some(nanos)`, a calendar-variable period yields
/// `None`, which forces every row of such a series to carry its own `close_time`
/// rather than inventing a length.
///
/// # Errors
///
/// Returns an error only on overflow of a fixed-duration period; a
/// calendar-variable aggregation is `Ok(None)`, not an error.
fn bar_interval_nanos_for_spec(spec: CanonicalBarSpec) -> Result<Option<i64>> {
    let is_fixed_duration = BAR_UNITS_MS
        .iter()
        .any(|(aggregation, _)| *aggregation == spec.aggregation);
    if !is_fixed_duration {
        return Ok(None);
    }
    let interval_ms = bar_interval_ms(spec)?;
    let interval_nanos = i64::try_from(
        interval_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .context("bar interval overflows nanoseconds")?,
    )
    .context("bar interval overflows i64")?;
    Ok(Some(interval_nanos))
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
    normalize_csv_native_bars_with_meter(
        accepted,
        identities,
        mapping,
        csv_text,
        capture_time_nanos,
        ingest_run_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn normalize_csv_native_bars_with_meter(
    accepted: &AcceptedDataset,
    identities: &BarInstrumentIdentities,
    mapping: &BarMappingConfig,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
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
        let fields = match record {
            Ok(fields) => fields,
            Err(error) => {
                work_budget.consume_source_row(OperatorWorkBudgetStage::Normalize)?;
                return Err(error).with_context(|| format!("row {index}: malformed csv record"));
            }
        };
        if fields.iter().all(str::is_empty) {
            continue;
        }
        work_budget.consume_source_row(OperatorWorkBudgetStage::Normalize)?;
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
        apply_price_sign_policy_at(
            &format!("row {index}"),
            mapping.price_sign_policy,
            &open,
            &high,
            &low,
            &close,
        )?;

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
    // Declared: the operator-specified period is reconciled against the period
    // derived from each instrument's own deduped open times via the one shared
    // [`bar_spec_from_open_times`] helper — the SAME reconciliation the paged-
    // JSON and derive paths use. The helper computes the fundamental (minimum-
    // gap) spec and enforces that every gap is a multiple of it, so a declared
    // period that under-states the true spacing (e.g. 60 s bars declared as 1 s)
    // fails loud instead of mislabelling the bar_spec and deriving a wrong
    // close_time. A single-bar instrument is valid — one bar cannot prove a
    // period, but the declared period makes it unambiguous.
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
            let interval_nanos = bar_interval_nanos_for_spec(declared)?
                .context("declared fixed-duration bar spec yielded no fixed nanosecond length")?;
            // Reconcile the declared period against the period derived from each
            // instrument's own deduped open times. The shared helper enforces
            // all-gaps-are-multiples-of-the-minimum internally (so misaligned
            // rows still fail loud) AND returns the fundamental spec, so a
            // declared period that mis-states the true spacing is rejected
            // instead of silently mislabelling the table. A single-bar
            // instrument has no derivable gap; the declaration stands.
            for instrument_key in &group_order {
                let rows = groups
                    .get(instrument_key)
                    .context("internal: group_order key absent from groups")?;
                let mut opens: Vec<i64> = rows.iter().map(|row| row.open_time).collect();
                opens.sort_unstable();
                opens.dedup();
                if opens.len() < 2 {
                    continue;
                }
                // Prefix the instrument key while preserving the specific
                // derivation failure (e.g. "not a multiple") in the Display, as
                // the derive path does — a `with_context` wrapper would bury it.
                let derived = bar_spec_from_open_times(&opens)
                    .map_err(|error| anyhow::anyhow!("instrument {instrument_key:?}: {error:#}"))?;
                ensure!(
                    declared == derived,
                    "instrument {instrument_key:?}: declared bar interval {declared:?} does not \
                     match interval derived from open times {derived:?} — the declared period \
                     mis-states the true bar spacing"
                );
            }
            (declared, interval_nanos)
        }
        BarIntervalSource::DerivedFromOpenTimes => {
            // Derive the spec per instrument through the one shared
            // open-times helper, then require every instrument to agree on a
            // single canonical spec for the whole object.
            let mut object_spec: Option<CanonicalBarSpec> = None;
            for instrument_key in &group_order {
                let rows = groups
                    .get(instrument_key)
                    .context("internal: group_order key absent from groups")?;
                let mut opens: Vec<i64> = rows.iter().map(|row| row.open_time).collect();
                opens.sort_unstable();
                opens.dedup();
                // A single-bar instrument cannot prove a gap; the derive path
                // demands an explicit declaration with an operator-actionable
                // message before delegating to the shared min-gap derivation.
                ensure!(
                    opens.len() >= 2,
                    "instrument {instrument_key:?} has only {} bar row(s) — cannot derive the \
                     period from a single open time; declare the interval explicitly via \
                     interval_source = \"declared\"",
                    opens.len()
                );
                // Prefix the instrument key while preserving the specific
                // derivation failure (e.g. "not a multiple") in the Display.
                // A `with_context` wrapper would bury that reason behind a
                // generic message in `to_string()`, masking the operator-
                // actionable cause and diverging from the declared-interval
                // path, which surfaces "not a multiple" at the top level.
                let instrument_spec = bar_spec_from_open_times(&opens)
                    .map_err(|error| anyhow::anyhow!("instrument {instrument_key:?}: {error:#}"))?;
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
            let interval_nanos = bar_interval_nanos_for_spec(bar_spec)?.context(
                "internal: derived fixed-duration bar spec yielded no fixed nanosecond length",
            )?;
            (bar_spec, interval_nanos)
        }
        BarIntervalSource::FromColumn { .. } => {
            bail!("internal: from_column interval source reached after pre-parse rejection")
        }
    };

    let mut tables = Vec::with_capacity(group_order.len());
    for instrument_key in &group_order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let parsed_rows = groups
            .remove(instrument_key)
            .expect("group order entry has a populated group");
        let table = assemble_bar_table(
            BAR_TRANSFORM_IDENTITY,
            accepted,
            identity,
            bar_spec,
            Some(interval_nanos),
            parsed_rows,
            capture_time_nanos,
            ingest_run_id,
        )?;
        tables.push(table);
    }

    Ok(tables)
}

/// Shape of one bar row inside a paged JSON envelope (F2).
///
/// A row is either a positional JSON array (field order fixed by index) or a JSON
/// object keyed by field name. `close_time` is optional in both: absent, it is
/// derived from the declared interval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PagedJsonRowShape {
    /// Each row is a JSON array; fields are read by zero-based index.
    PositionalArray {
        open_time_index: usize,
        open_index: usize,
        high_index: usize,
        low_index: usize,
        close_index: usize,
        volume_index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        close_time_index: Option<usize>,
    },
    /// Each row is a JSON object; fields are read by key.
    FieldKeyed {
        open_time_field: String,
        open_field: String,
        high_field: String,
        low_field: String,
        close_field: String,
        volume_field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        close_time_field: Option<String>,
    },
}

/// Declared bar period for the paged-JSON adapter (F2).
///
/// Paged REST kline pages carry no interval of their own, so the period is always
/// run-spec declared and then reconciled against the period derived from the
/// deduped open times — exactly as [`BarIntervalSource::Declared`] is reconciled
/// for the CSV path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredBarInterval {
    pub step: usize,
    pub aggregation: BarAggregation,
}

/// Run-spec owned paged-JSON bar column mapping for the F2 source adapter.
///
/// A source that serves OHLCV klines inside a JSON REST envelope (rows nested at
/// `rows_path`, pages arriving newest-first and overlapping in time) selects the
/// paged-JSON converter from TOML and supplies this mapping. Paged REST is
/// per-instrument, so there is no instrument column: the caller binds the single
/// identity. The period is always declared (pages carry none) and reconciled
/// against the data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PagedJsonBarMappingConfig {
    /// Dot-separated path to the row array inside each envelope object, for
    /// example `result.list`. Generic config string; never a venue literal.
    pub rows_path: String,
    pub row_shape: PagedJsonRowShape,
    pub timestamp_unit: CsvTimestampUnit,
    /// Declared period; pages carry no interval. Reconciled against the period
    /// derived from the deduped open times.
    pub interval: DeclaredBarInterval,
    pub price_sign_policy: BarPriceSignPolicy,
}

/// Normalize an accepted paged-JSON bar object into one [`CanonicalBarsTable`]
/// for the single bound instrument (format family F2).
///
/// `json_text` is the decoded text of the accepted object whose hash already
/// matched the manifest. The payload is EITHER one JSON envelope object OR several
/// newline-separated envelope objects (one per fetched page concatenated by the
/// backfill); both forms are accepted, and every page's rows at `rows_path` are
/// collected. Adjacent pages overlap in time, so rows are sorted ascending by
/// open time and deduped via [`dedup_sorted_bar_rows`] (byte-identical overlaps
/// collapse, disagreeing overlaps fail loud). The declared period is reconciled
/// against the period derived from the deduped open times, and each missing
/// `close_time` is derived from that period.
///
/// Each scalar (timestamp + OHLCV) is read as a JSON string or integer; a JSON
/// float is rejected ([`json_scalar_to_string`]) because it cannot preserve the
/// source precision through `f64`. Sources that publish fractional prices serve
/// them as strings.
///
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run, recorded for
/// lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if the envelope is not valid JSON, `rows_path` does not
/// resolve to an array, a row is malformed, a field fails to parse, an OHLC price
/// is non-positive, the declared period disagrees with the data-derived period,
/// or the produced table fails its contract.
pub fn normalize_paged_json_bars(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &PagedJsonBarMappingConfig,
    json_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalBarsTable>> {
    normalize_paged_json_bars_with_meter(
        accepted,
        identity,
        mapping,
        json_text,
        capture_time_nanos,
        ingest_run_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn normalize_paged_json_bars_with_meter(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &PagedJsonBarMappingConfig,
    json_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<CanonicalBarsTable>> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        !mapping.rows_path.trim().is_empty(),
        "converter paged_json_bars.rows_path must not be empty"
    );
    let path_segments: Vec<&str> = mapping.rows_path.split('.').collect();
    ensure!(
        path_segments.iter().all(|segment| !segment.is_empty()),
        "converter paged_json_bars.rows_path {:?} has an empty path segment",
        mapping.rows_path
    );
    let declared_spec = CanonicalBarSpec {
        step: mapping.interval.step,
        aggregation: mapping.interval.aggregation,
    };

    let mut parsed_rows: Vec<ParsedBarRow> = Vec::new();
    let mut object_open_times: Vec<i64> = Vec::new();
    // Accept either one envelope object (which may span multiple lines, e.g. a
    // pretty-printed body) or newline-separated compact envelope objects (the
    // backfill may concatenate page bodies). The whole text is tried as one
    // envelope FIRST; only when that parse fails is the text split per line,
    // because a multi-page concatenation is never itself valid JSON while a
    // single envelope may legitimately contain newlines.
    let pages: Vec<String> = match serde_json::from_str::<serde_json::Value>(json_text) {
        Ok(_) => vec![json_text.to_string()],
        Err(_) => json_text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_string)
            .collect(),
    };
    let mut saw_envelope = false;
    for (page_index, line) in pages.iter().enumerate() {
        saw_envelope = true;
        let envelope: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("page {page_index}: invalid JSON envelope"))?;
        let rows_value = walk_json_path(&envelope, &path_segments).with_context(|| {
            format!(
                "page {page_index}: rows_path {:?} does not resolve in envelope",
                mapping.rows_path
            )
        })?;
        let rows = rows_value.as_array().with_context(|| {
            format!(
                "page {page_index}: rows_path {:?} is not a JSON array",
                mapping.rows_path
            )
        })?;
        for (row_index, row_value) in rows.iter().enumerate() {
            work_budget.consume_source_row(OperatorWorkBudgetStage::Normalize)?;
            let parsed = parse_paged_json_row(
                page_index,
                row_index,
                row_value,
                &mapping.row_shape,
                mapping.timestamp_unit,
                mapping.price_sign_policy,
            )?;
            object_open_times.push(parsed.open_time);
            parsed_rows.push(parsed);
        }
    }
    ensure!(
        saw_envelope,
        "paged-JSON bar object carried no envelope content"
    );
    ensure!(
        !parsed_rows.is_empty(),
        "paged-JSON bar object yielded no rows"
    );

    // Reconcile the declared period against the data, exactly as the CSV path
    // reconciles a declared interval, so a mis-declared step fails loud.
    let derived_spec = bar_spec_from_open_times(&object_open_times)?;
    ensure!(
        declared_spec == derived_spec,
        "declared bar interval {declared_spec:?} does not match interval derived from open times {derived_spec:?}"
    );
    let interval_nanos = bar_interval_nanos_for_spec(declared_spec)?;

    let table = assemble_bar_table(
        PAGED_JSON_BARS_TRANSFORM_IDENTITY,
        accepted,
        identity,
        declared_spec,
        interval_nanos,
        parsed_rows,
        capture_time_nanos,
        ingest_run_id,
    )?;
    Ok(vec![table])
}

/// Resolve a dot-separated path of object keys to a value inside `root`.
///
/// Every segment must address an object key; an array index segment or a missing
/// key is an error (the caller validates the final value is an array).
fn walk_json_path<'value>(
    root: &'value serde_json::Value,
    segments: &[&str],
) -> Result<&'value serde_json::Value> {
    let mut current = root;
    for segment in segments {
        current = current
            .get(*segment)
            .with_context(|| format!("path segment {segment:?} not found"))?;
    }
    Ok(current)
}

/// Parse one paged-JSON row (array or object shape) into a [`ParsedBarRow`].
fn parse_paged_json_row(
    page_index: usize,
    row_index: usize,
    row_value: &serde_json::Value,
    row_shape: &PagedJsonRowShape,
    timestamp_unit: CsvTimestampUnit,
    price_sign_policy: BarPriceSignPolicy,
) -> Result<ParsedBarRow> {
    let location = format!("page {page_index} row {row_index}");
    let (open_time_raw, open, high, low, close, volume, close_time_raw) = match row_shape {
        PagedJsonRowShape::PositionalArray {
            open_time_index,
            open_index,
            high_index,
            low_index,
            close_index,
            volume_index,
            close_time_index,
        } => {
            let array = row_value
                .as_array()
                .with_context(|| format!("{location}: positional row is not a JSON array"))?;
            let at = |index: usize, label: &str| -> Result<String> {
                json_scalar_to_string(
                    array
                        .get(index)
                        .with_context(|| format!("{location}: missing {label} at index {index}"))?,
                    &location,
                    label,
                )
            };
            let open_time_raw = at(*open_time_index, "open_time")?;
            let open = at(*open_index, "open")?;
            let high = at(*high_index, "high")?;
            let low = at(*low_index, "low")?;
            let close = at(*close_index, "close")?;
            let volume = at(*volume_index, "volume")?;
            let close_time_raw = match close_time_index {
                Some(close_time_index) => Some(at(*close_time_index, "close_time")?),
                None => None,
            };
            (
                open_time_raw,
                open,
                high,
                low,
                close,
                volume,
                close_time_raw,
            )
        }
        PagedJsonRowShape::FieldKeyed {
            open_time_field,
            open_field,
            high_field,
            low_field,
            close_field,
            volume_field,
            close_time_field,
        } => {
            let at = |field: &str, label: &str| -> Result<String> {
                json_scalar_to_string(
                    row_value
                        .get(field)
                        .with_context(|| format!("{location}: missing {label} field {field:?}"))?,
                    &location,
                    label,
                )
            };
            let open_time_raw = at(open_time_field, "open_time")?;
            let open = at(open_field, "open")?;
            let high = at(high_field, "high")?;
            let low = at(low_field, "low")?;
            let close = at(close_field, "close")?;
            let volume = at(volume_field, "volume")?;
            let close_time_raw = match close_time_field {
                Some(close_time_field) => Some(at(close_time_field, "close_time")?),
                None => None,
            };
            (
                open_time_raw,
                open,
                high,
                low,
                close,
                volume,
                close_time_raw,
            )
        }
    };

    let open_time = timestamp_unit
        .parse_to_nanos(&open_time_raw)
        .with_context(|| format!("{location}: invalid open_time {open_time_raw:?}"))?;
    ensure!(open_time > 0, "{location}: non-positive open_time");
    let close_time = match close_time_raw {
        Some(close_time_raw) => Some(
            timestamp_unit
                .parse_to_nanos(&close_time_raw)
                .with_context(|| format!("{location}: invalid close_time {close_time_raw:?}"))?,
        ),
        None => None,
    };
    for (label, value) in [
        ("open", &open),
        ("high", &high),
        ("low", &low),
        ("close", &close),
        ("volume", &volume),
    ] {
        ensure!(!value.trim().is_empty(), "{location}: empty {label}");
    }
    apply_price_sign_policy_at(&location, price_sign_policy, &open, &high, &low, &close)?;

    Ok(ParsedBarRow {
        instrument_key: None,
        open_time,
        close_time,
        open,
        high,
        low,
        close,
        volume,
    })
}

/// Render a JSON scalar as its exact source token, preserving precision.
///
/// Klines carry OHLCV and timestamps as JSON strings or integers; both preserve
/// the source token exactly (a string is verbatim, an integer round-trips
/// losslessly). A JSON FLOAT is rejected: without serde_json's
/// `arbitrary_precision` feature a float is parsed through `f64`, which drops
/// trailing zeros and other significant digits (`0.50` -> `0.5`), so accepting it
/// would silently corrupt the source precision. Sources that publish fractional
/// prices serve them as strings; a float here is a config/source mismatch and
/// fails loud rather than rounding. Booleans, nulls, arrays, and objects are not
/// scalar values and fail loud.
fn json_scalar_to_string(value: &serde_json::Value, location: &str, label: &str) -> Result<String> {
    match value {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Number(number) if number.is_f64() => bail!(
            "{location}: {label} {number} is a JSON float; serve fractional values as JSON \
             strings to preserve exact precision (a float loses trailing zeros through f64)"
        ),
        serde_json::Value::Number(number) => Ok(number.to_string()),
        other => bail!("{location}: {label} {other} is not a string or integer scalar"),
    }
}

/// One bar period mapped from a source interval token (F3).
///
/// The serde-friendly value of the [`JsonlBarMappingConfig::interval_token_map`]:
/// a config-side token (for example `1m`, `1h`, `1w`, `1M`) maps to a
/// NautilusTrader `(step, aggregation)` pair. Tokens are matched case-SENSITIVELY
/// so a lowercase week token (`1w` -> [`BarAggregation::Week`]) and an uppercase
/// month token (`1M` -> [`BarAggregation::Month`]) map to different periods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarIntervalToken {
    pub step: usize,
    pub aggregation: BarAggregation,
}

/// Run-spec owned line-delimited multi-interval bar mapping for the F3 source
/// adapter.
///
/// A source that serves OHLCV klines as line-delimited JSON objects, where each
/// line carries its own interval token (the family's defining trait), selects the
/// JSONL multi-interval converter from TOML and supplies this mapping. The
/// instrument-key field is optional (a single-instrument object omits it); the
/// interval-token field is REQUIRED. `interval_token_map` maps every source token
/// to a `(step, aggregation)` pair and an unmapped token fails loud — tokens are
/// matched case-sensitively (`w` vs `M`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonlBarMappingConfig {
    /// Field keying the per-line instrument in a multi-instrument object. `None`
    /// selects the single-instrument object shape (the caller binds one identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instrument_field: Option<String>,
    /// Field carrying the per-line interval token. Required: the interval is this
    /// family's defining per-row trait.
    pub interval_field: String,
    pub timestamp_unit: CsvTimestampUnit,
    pub open_time_field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_time_field: Option<String>,
    pub open_field: String,
    pub high_field: String,
    pub low_field: String,
    pub close_field: String,
    pub volume_field: String,
    /// Source interval token -> `(step, aggregation)`. An unmapped token fails
    /// loud; tokens match case-sensitively.
    pub interval_token_map: BTreeMap<String, BarIntervalToken>,
    pub price_sign_policy: BarPriceSignPolicy,
}

/// Normalize an accepted line-delimited multi-interval bar object into one
/// [`CanonicalBarsTable`] per `(instrument_key, interval token)` group (format
/// family F3).
///
/// `jsonl_text` is the decoded text of the accepted object whose hash already
/// matched the manifest. Each non-blank line is one JSON object carrying its own
/// interval token (mapped through `interval_token_map`, fail-loud on an unmapped
/// token). Rows group by `(instrument_key, interval token)` so a staged object
/// that interleaves several intervals for one or several instruments emits one
/// table per group, each with its own `bar_spec`. Per group, rows are sorted
/// ascending and deduped via [`dedup_sorted_bar_rows`], and each missing
/// `close_time` is derived from the group's mapped period (a calendar-variable
/// period such as a month has no fixed length, so a row of such a group missing
/// `close_time` fails loud).
///
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run, recorded for
/// lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if `ingest_run_id`/`interval_field` is empty, a line is not
/// valid JSON, a required field is missing or fails to parse, an interval token
/// is not in `interval_token_map`, an OHLC price is non-positive, an instrument
/// key resolves to no identity, or a produced table fails its contract.
pub fn normalize_jsonl_multi_interval_bars(
    accepted: &AcceptedDataset,
    identities: &BarInstrumentIdentities,
    mapping: &JsonlBarMappingConfig,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalBarsTable>> {
    normalize_jsonl_multi_interval_bars_with_meter(
        accepted,
        identities,
        mapping,
        jsonl_text,
        capture_time_nanos,
        ingest_run_id,
        &OperatorWorkBudgetGuard::unbounded(),
    )
}

pub(crate) fn normalize_jsonl_multi_interval_bars_with_meter(
    accepted: &AcceptedDataset,
    identities: &BarInstrumentIdentities,
    mapping: &JsonlBarMappingConfig,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<CanonicalBarsTable>> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        !mapping.interval_field.trim().is_empty(),
        "converter jsonl_bars.interval_field must not be empty"
    );
    for (label, field) in [
        ("open_time_field", &mapping.open_time_field),
        ("open_field", &mapping.open_field),
        ("high_field", &mapping.high_field),
        ("low_field", &mapping.low_field),
        ("close_field", &mapping.close_field),
        ("volume_field", &mapping.volume_field),
    ] {
        ensure!(
            !field.trim().is_empty(),
            "converter jsonl_bars.{label} must not be empty"
        );
    }
    if let Some(instrument_field) = &mapping.instrument_field {
        ensure!(
            !instrument_field.trim().is_empty(),
            "converter jsonl_bars.instrument_field must not be empty when set"
        );
    }
    ensure!(
        !mapping.interval_token_map.is_empty(),
        "converter jsonl_bars.interval_token_map must not be empty"
    );
    for (token, interval) in &mapping.interval_token_map {
        ensure!(
            !token.trim().is_empty(),
            "converter jsonl_bars.interval_token_map carries an empty token"
        );
        ensure!(
            interval.step > 0,
            "converter jsonl_bars.interval_token_map token {token:?} has a non-positive step"
        );
    }

    // Group rows by (instrument_key, interval token), preserving first-seen group
    // order so the produced tables are deterministically ordered by first
    // appearance. A BTreeMap keys the groups for grouping; group_order keeps the
    // emission order stable.
    type GroupKey = (Option<String>, String);
    let mut group_order: Vec<GroupKey> = Vec::new();
    let mut groups: BTreeMap<GroupKey, Vec<ParsedBarRow>> = BTreeMap::new();

    for (line_index, line) in jsonl_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        work_budget.consume_source_row(OperatorWorkBudgetStage::Normalize)?;
        let location = format!("line {}", line_index + 1);
        let record: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("{location}: invalid JSON object"))?;

        let interval_token = json_scalar_to_string(
            record.get(&mapping.interval_field).with_context(|| {
                format!(
                    "{location}: missing interval field {:?}",
                    mapping.interval_field
                )
            })?,
            &location,
            "interval",
        )?;
        ensure!(
            mapping.interval_token_map.contains_key(&interval_token),
            "{location}: interval token {interval_token:?} is not in interval_token_map"
        );

        let instrument_key = match &mapping.instrument_field {
            Some(instrument_field) => {
                let raw = json_scalar_to_string(
                    record.get(instrument_field).with_context(|| {
                        format!("{location}: missing instrument field {instrument_field:?}")
                    })?,
                    &location,
                    "instrument",
                )?;
                ensure!(!raw.is_empty(), "{location}: empty instrument field");
                Some(raw)
            }
            None => None,
        };

        let at = |field: &str, label: &str| -> Result<String> {
            json_scalar_to_string(
                record
                    .get(field)
                    .with_context(|| format!("{location}: missing {label} field {field:?}"))?,
                &location,
                label,
            )
        };
        let open_time_raw = at(&mapping.open_time_field, "open_time")?;
        let open = at(&mapping.open_field, "open")?;
        let high = at(&mapping.high_field, "high")?;
        let low = at(&mapping.low_field, "low")?;
        let close = at(&mapping.close_field, "close")?;
        let volume = at(&mapping.volume_field, "volume")?;
        let close_time_raw = match &mapping.close_time_field {
            Some(close_time_field) => Some(at(close_time_field, "close_time")?),
            None => None,
        };

        let open_time = mapping
            .timestamp_unit
            .parse_to_nanos(&open_time_raw)
            .with_context(|| format!("{location}: invalid open_time {open_time_raw:?}"))?;
        ensure!(open_time > 0, "{location}: non-positive open_time");
        let close_time = match close_time_raw {
            Some(close_time_raw) => Some(
                mapping
                    .timestamp_unit
                    .parse_to_nanos(&close_time_raw)
                    .with_context(|| {
                        format!("{location}: invalid close_time {close_time_raw:?}")
                    })?,
            ),
            None => None,
        };
        for (label, value) in [
            ("open", &open),
            ("high", &high),
            ("low", &low),
            ("close", &close),
            ("volume", &volume),
        ] {
            ensure!(!value.trim().is_empty(), "{location}: empty {label}");
        }
        apply_price_sign_policy_at(
            &location,
            mapping.price_sign_policy,
            &open,
            &high,
            &low,
            &close,
        )?;

        let key: GroupKey = (instrument_key.clone(), interval_token);
        let group = groups.entry(key.clone()).or_insert_with(|| {
            group_order.push(key.clone());
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

    ensure!(
        !group_order.is_empty(),
        "JSONL multi-interval bar object yielded no rows"
    );

    let mut tables = Vec::with_capacity(group_order.len());
    for key in &group_order {
        let (instrument_key, interval_token) = key;
        let identity = identities.resolve(instrument_key.as_deref())?;
        let interval = mapping
            .interval_token_map
            .get(interval_token)
            .expect("grouped interval token is in the map");
        let bar_spec = CanonicalBarSpec {
            step: interval.step,
            aggregation: interval.aggregation,
        };
        let interval_nanos = bar_interval_nanos_for_spec(bar_spec)?;
        let parsed_rows = groups
            .remove(key)
            .expect("group order entry has a populated group");
        let table = assemble_bar_table(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            accepted,
            identity,
            bar_spec,
            interval_nanos,
            parsed_rows,
            capture_time_nanos,
            ingest_run_id,
        )?;
        tables.push(table);
    }
    Ok(tables)
}

/// Assemble one validated [`CanonicalBarsTable`] from parsed rows of a single
/// `(instrument, interval)` series.
///
/// The single source of truth for bar-table assembly across every format family:
/// it sorts + dedups the rows ([`dedup_sorted_bar_rows`]), derives each missing
/// `close_time` from `interval_nanos`, stamps the identity/provenance header from
/// `accepted`, stamps each row's `transform_hash` from the caller's
/// `transform_identity` (so each format family — CSV, paged-JSON, JSONL
/// multi-interval — records its OWN provenance rather than a single hardcoded
/// one), builds the [`CanonicalBarsTable`] with the supplied `bar_spec`, and
/// validates it through the table contract.
///
/// `interval_nanos` is the fixed nanosecond length of one bar period when known.
/// It is `None` for a calendar-variable period (for example a month) whose length
/// is not a fixed multiple of nanoseconds; in that case every row must already
/// carry its own `close_time`, and a row missing one fails loud rather than
/// inventing a period length.
///
/// # Errors
///
/// Returns an error if a duplicate row disagrees, a derived `close_time`
/// overflows, a row needs a derived `close_time` but `interval_nanos` is `None`,
/// or the assembled table fails its contract.
#[allow(clippy::too_many_arguments)]
fn assemble_bar_table(
    transform_identity: &str,
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    bar_spec: CanonicalBarSpec,
    interval_nanos: Option<i64>,
    parsed_rows: Vec<ParsedBarRow>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<CanonicalBarsTable> {
    let canonical_instrument_key = format!(
        "{}/{}/{}",
        accepted.venue, accepted.product_family, identity.instrument_id
    );
    let transform_hash = compute_bar_transform_hash(transform_identity);
    let parsed_rows = dedup_sorted_bar_rows(parsed_rows)?;

    let mut rows = Vec::with_capacity(parsed_rows.len());
    for parsed in parsed_rows {
        let close_time = match parsed.close_time {
            Some(close_time) => close_time,
            None => {
                let interval_nanos = interval_nanos.with_context(|| {
                    format!(
                        "bar at open_time {} for instrument {:?} carries no close_time and the \
                         {:?} period has no fixed nanosecond length to derive one",
                        parsed.open_time, identity.instrument_id, bar_spec.aggregation
                    )
                })?;
                parsed
                    .open_time
                    .checked_add(interval_nanos)
                    .context("bar close_time overflows nanoseconds")?
            }
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
        transform_hash,
        payload_hash: accepted.object.sha256.clone(),
        bar_spec,
        rows,
    };
    table.validate()?;
    Ok(table)
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
/// volume is left to [`CanonicalBarsTable::validate`]. `location` is a
/// human-readable position label (for example `row 3` or `page 1 row 0`) so every
/// format family reports the same sign violation through one helper.
fn apply_price_sign_policy_at(
    location: &str,
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
                    .with_context(|| format!("{location}: invalid {label} {value:?}"))?;
                ensure!(
                    parsed > Decimal::ZERO,
                    "{location}: non-positive {label} {value:?}"
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
        // AAA carries two bars with 1-minute gaps; BBB carries one bar (no gap
        // to validate). With a declared 1-minute interval, single-bar BBB is valid.
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
        )
        .expect_err("from_column interval source must be rejected");
        assert!(err.to_string().contains("from_column"), "{err}");
        assert!(err.to_string().contains("format family"), "{err}");
    }

    #[test]
    fn rejects_declared_interval_misaligned_with_data_gaps() {
        let accepted = accepted_dataset(&schema_with_close());
        // Data has uniform one-minute gaps; declaring one hour over-states the
        // true spacing. The declared spec is reconciled against the spec derived
        // from the open times (one minute), so the mismatch must fail loud.
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
        .expect_err("over-stated declared interval must be rejected against derived spec");
        assert!(
            err.to_string()
                .contains("does not match interval derived from open times"),
            "{err}"
        );
    }

    /// A row whose open-time gap is not a multiple of the minimum gap is
    /// misaligned, not a data hole. The shared derivation helper (reached through
    /// the declared-interval reconciliation) must surface "not a multiple".
    #[test]
    fn rejects_declared_interval_misaligned_within_instrument_gaps() {
        let accepted = accepted_dataset(&schema_with_close());
        // Gaps: 60 s then 90 s — 90 s is not a multiple of the 60 s minimum, so
        // the open times do not describe a single period regardless of what is
        // declared.
        let csv = "open_time,close_time,open,high,low,close,volume\n\
            1700000000000,1700000060000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,1700000120000,0.52,0.58,0.51,0.57,120\n\
            1700000150000,1700000210000,0.57,0.60,0.55,0.59,80\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect_err("misaligned within-instrument gaps must be rejected");
        assert!(
            err.to_string().contains("not a multiple"),
            "expected 'not a multiple' in error: {err}"
        );
    }

    /// F7: a CSV of true 60 s-spaced bars with NO close_time column declared with
    /// a SMALLER step (1 s) must FAIL LOUD. The old divisibility-only check let
    /// it pass (every 60 s gap is a multiple of 1 s), then derived a wrong 1 s
    /// close_time and wrote a mislabelled bar_spec. The shared derivation derives
    /// the fundamental 60 s spec, so the declared 1 s now fails to reconcile.
    #[test]
    fn rejects_declared_interval_understating_true_spacing() {
        let accepted = accepted_dataset(&["open_time", "open", "high", "low", "close", "volume"]);
        // Declared one-second step over data spaced one minute apart.
        let mapping = BarMappingConfig {
            interval_source: BarIntervalSource::Declared {
                step: 1,
                aggregation: BarAggregation::Second,
            },
            ..declared_minute_mapping()
        };
        let csv = "open_time,open,high,low,close,volume\n\
            1700000000000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,0.52,0.58,0.51,0.57,120\n\
            1700000120000,0.57,0.60,0.55,0.59,80\n";
        let err = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
        )
        .expect_err("under-stated declared interval must be rejected against derived spec");
        assert!(
            err.to_string()
                .contains("does not match interval derived from open times"),
            "expected derived-spec mismatch in error: {err}"
        );
    }

    /// F7: sparse-bar tolerance must be preserved. A correctly declared 60 s
    /// interval over data with HOLES (gaps 60 s, 120 s, 180 s) must still PASS —
    /// larger gaps are missing bars, never a different period.
    #[test]
    fn accepts_declared_interval_with_sparse_holes() {
        let accepted = accepted_dataset(&["open_time", "open", "high", "low", "close", "volume"]);
        // Open times at 0 s, 60 s, 180 s, 360 s: gaps of 60 s, 120 s, 180 s — all
        // multiples of the 60 s declared minute.
        let csv = "open_time,open,high,low,close,volume\n\
            1700000000000,0.50,0.55,0.49,0.52,100\n\
            1700000060000,0.52,0.58,0.51,0.57,120\n\
            1700000180000,0.57,0.60,0.55,0.59,80\n\
            1700000360000,0.59,0.62,0.58,0.61,90\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("sparse holes that are multiples of the declared minute must be accepted");
        let table = &tables[0];
        assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
        assert_eq!(table.rows.len(), 4);
        // close_time is derived from the declared (and reconciled) minute period.
        assert_eq!(table.rows[0].close_time, 1_700_000_060_000_000_000);
    }

    /// F7: a single-bar instrument cannot derive a period, so an explicit
    /// declaration is still accepted (no derivable gap to reconcile against).
    #[test]
    fn accepts_declared_interval_for_single_bar_instrument() {
        let accepted = accepted_dataset(&["open_time", "open", "high", "low", "close", "volume"]);
        let csv = "open_time,open,high,low,close,volume\n\
            1700000000000,0.50,0.55,0.49,0.52,100\n";
        let tables = normalize_csv_native_bars(
            &accepted,
            &single_identity(),
            &declared_minute_mapping(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("single-bar instrument keeps the declared period");
        let table = &tables[0];
        assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].close_time, 1_700_000_060_000_000_000);
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

    // ---------- F2: paged REST JSON klines ----------

    fn single_identity_value() -> CanonicalInstrumentIdentity {
        identity("BASEQUOTE")
    }

    fn positional_paged_mapping() -> PagedJsonBarMappingConfig {
        // Rows are positional arrays: [open_time, open, high, low, close, volume].
        PagedJsonBarMappingConfig {
            rows_path: "result.list".to_string(),
            row_shape: PagedJsonRowShape::PositionalArray {
                open_time_index: 0,
                open_index: 1,
                high_index: 2,
                low_index: 3,
                close_index: 4,
                volume_index: 5,
                close_time_index: None,
            },
            timestamp_unit: CsvTimestampUnit::Milliseconds,
            interval: DeclaredBarInterval {
                step: 1,
                aggregation: BarAggregation::Minute,
            },
            price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
        }
    }

    #[test]
    fn paged_json_positional_rows_sort_newest_first_input_ascending() {
        // One envelope, rows newest-first (the venue's natural order). The
        // adapter walks result.list, then sorts ascending by open time.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let json = r#"{"result":{"list":[["1700000060000","0.52","0.58","0.51","0.57","120"],["1700000000000","0.50","0.55","0.49","0.52","100"]]}}"#;
        let tables = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            json,
            42,
            "ingest-run-test",
        )
        .expect("normalize paged json positional bars");
        assert_eq!(tables.len(), 1);
        let table = &tables[0];
        assert_eq!(table.rows.len(), 2);
        assert!(table.rows[0].open_time < table.rows[1].open_time);
        assert_eq!(table.rows[0].open_time, 1_700_000_000_000_000_000);
        assert_eq!(table.bar_spec.aggregation, BarAggregation::Minute);
        // No close_time column: derived as open_time + one minute.
        assert_eq!(table.rows[0].close_time, 1_700_000_060_000_000_000);
        assert_eq!(table.rows[0].open, "0.50");
        assert_eq!(
            table.rows[0].canonical_instrument_key,
            "testvenue/prediction-market/BASEQUOTE"
        );
        // Provenance: paged-JSON rows stamp their OWN identity, not the CSV one.
        assert_eq!(
            table.rows[0].transform_hash,
            compute_bar_transform_hash(PAGED_JSON_BARS_TRANSFORM_IDENTITY)
        );
    }

    #[test]
    fn paged_json_field_keyed_rows_with_close_time() {
        // Rows are objects keyed by field name and carry an explicit close time.
        let accepted = accepted_dataset(&["t", "o", "h", "l", "c", "v", "ct"]);
        let mapping = PagedJsonBarMappingConfig {
            row_shape: PagedJsonRowShape::FieldKeyed {
                open_time_field: "t".to_string(),
                open_field: "o".to_string(),
                high_field: "h".to_string(),
                low_field: "l".to_string(),
                close_field: "c".to_string(),
                volume_field: "v".to_string(),
                close_time_field: Some("ct".to_string()),
            },
            ..positional_paged_mapping()
        };
        let json = r#"{"result":{"list":[
            {"t":"1700000000000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100","ct":"1700000060000"},
            {"t":"1700000060000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120","ct":"1700000120000"}
        ]}}"#;
        let tables = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &mapping,
            json,
            42,
            "ingest-run-test",
        )
        .expect("normalize paged json field-keyed bars");
        let table = &tables[0];
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].close_time, 1_700_000_060_000_000_000);
        assert_eq!(table.rows[1].close_time, 1_700_000_120_000_000_000);
    }

    #[test]
    fn paged_json_integer_timestamps_round_trip_exactly() {
        // Timestamps are JSON integers (the common REST shape); integers
        // round-trip losslessly. OHLCV stay strings to preserve exact precision.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let json = r#"{"result":{"list":[[1700000000000,"0.50","0.55","0.49","0.52","100"],[1700000060000,"0.52","0.58","0.51","0.57","120"]]}}"#;
        let tables = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            json,
            42,
            "ingest-run-test",
        )
        .expect("normalize paged json with integer timestamps");
        assert_eq!(tables[0].rows[0].open, "0.50");
        assert_eq!(tables[0].rows[0].open_time, 1_700_000_000_000_000_000);
    }

    #[test]
    fn paged_json_rejects_float_price_to_protect_precision() {
        // A JSON float price cannot preserve trailing zeros through f64, so the
        // adapter fails loud rather than silently rounding `0.50` to `0.5`.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let json = r#"{"result":{"list":[[1700000000000,0.50,0.55,0.49,0.52,"100"],[1700000060000,"0.52","0.58","0.51","0.57","120"]]}}"#;
        let err = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            json,
            42,
            "ingest-run-test",
        )
        .expect_err("JSON float price must be rejected to protect precision");
        assert!(err.to_string().contains("JSON float"), "{err}");
    }

    #[test]
    fn paged_json_overlapping_pages_collapse_byte_identical_duplicates() {
        // Two newline-separated envelopes (pages) overlap on the boundary minute;
        // the byte-identical duplicate collapses to one row.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let page_one = r#"{"result":{"list":[["1700000000000","0.50","0.55","0.49","0.52","100"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#;
        let page_two = r#"{"result":{"list":[["1700000060000","0.52","0.58","0.51","0.57","120"],["1700000120000","0.57","0.60","0.56","0.59","90"]]}}"#;
        let json = format!("{page_one}\n{page_two}");
        let tables = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            &json,
            42,
            "ingest-run-test",
        )
        .expect("normalize overlapping paged json pages");
        assert_eq!(tables[0].rows.len(), 3, "boundary minute collapses to one");
    }

    #[test]
    fn paged_json_overlapping_pages_reject_disagreeing_duplicate() {
        // The boundary minute disagrees across pages (different close): corrupt.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let page_one = r#"{"result":{"list":[["1700000000000","0.50","0.55","0.49","0.52","100"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#;
        let page_two = r#"{"result":{"list":[["1700000060000","0.52","0.58","0.51","0.99","120"],["1700000120000","0.57","0.60","0.56","0.59","90"]]}}"#;
        let json = format!("{page_one}\n{page_two}");
        let err = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            &json,
            42,
            "ingest-run-test",
        )
        .expect_err("disagreeing duplicate across pages must be rejected");
        assert!(err.to_string().contains("disagreeing"), "{err}");
    }

    #[test]
    fn paged_json_rejects_declared_interval_disagreeing_with_derived() {
        // The data is a one-minute period but the run-spec declares one hour.
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let mapping = PagedJsonBarMappingConfig {
            interval: DeclaredBarInterval {
                step: 1,
                aggregation: BarAggregation::Hour,
            },
            ..positional_paged_mapping()
        };
        let json = r#"{"result":{"list":[["1700000000000","0.50","0.55","0.49","0.52","100"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#;
        let err = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &mapping,
            json,
            42,
            "ingest-run-test",
        )
        .expect_err("declared/derived interval mismatch must be rejected");
        assert!(err.to_string().contains("does not match"), "{err}");
    }

    #[test]
    fn paged_json_rejects_non_positive_price_under_strictly_positive_policy() {
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        let json = r#"{"result":{"list":[["1700000000000","0.50","0.55","0","0.52","100"],["1700000060000","0.52","0.58","0.51","0.57","120"]]}}"#;
        let err = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            json,
            42,
            "ingest-run-test",
        )
        .expect_err("non-positive low must be rejected");
        assert!(err.to_string().contains("non-positive"), "{err}");
    }

    #[test]
    fn paged_json_rejects_rows_path_not_resolving() {
        let accepted = accepted_dataset(&["start", "open", "high", "low", "close", "volume"]);
        // The configured rows_path addresses a missing key.
        let json = r#"{"data":{"rows":[["1700000000000","0.50","0.55","0.49","0.52","100"]]}}"#;
        let err = normalize_paged_json_bars(
            &accepted,
            &single_identity_value(),
            &positional_paged_mapping(),
            json,
            42,
            "ingest-run-test",
        )
        .expect_err("unresolvable rows_path must be rejected");
        assert!(err.to_string().contains("rows_path"), "{err}");
    }

    // ---------- F3: line-delimited multi-interval klines ----------

    fn interval_token_map() -> BTreeMap<String, BarIntervalToken> {
        // Case-sensitive: lowercase week vs uppercase month map to different
        // periods.
        BTreeMap::from([
            (
                "1m".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Minute,
                },
            ),
            (
                "1h".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Hour,
                },
            ),
            (
                "1w".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Week,
                },
            ),
            (
                "1M".to_string(),
                BarIntervalToken {
                    step: 1,
                    aggregation: BarAggregation::Month,
                },
            ),
        ])
    }

    fn single_jsonl_mapping() -> JsonlBarMappingConfig {
        JsonlBarMappingConfig {
            instrument_field: None,
            interval_field: "interval".to_string(),
            timestamp_unit: CsvTimestampUnit::Milliseconds,
            open_time_field: "t".to_string(),
            close_time_field: Some("ct".to_string()),
            open_field: "o".to_string(),
            high_field: "h".to_string(),
            low_field: "l".to_string(),
            close_field: "c".to_string(),
            volume_field: "v".to_string(),
            interval_token_map: interval_token_map(),
            price_sign_policy: BarPriceSignPolicy::StrictlyPositive,
        }
    }

    #[test]
    fn jsonl_multi_interval_single_instrument_splits_by_interval() {
        // One instrument, two intervals interleaved -> two tables, each with its
        // own bar_spec, in first-seen order.
        let accepted = accepted_dataset(&["interval", "t", "ct", "o", "h", "l", "c", "v"]);
        let jsonl = concat!(
            r#"{"interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#,
            "\n",
            r#"{"interval":"1h","t":"1700000000000","ct":"1700003600000","o":"0.50","h":"0.60","l":"0.48","c":"0.59","v":"500"}"#,
            "\n",
            r#"{"interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120"}"#,
            "\n",
            r#"{"interval":"1h","t":"1700003600000","ct":"1700007200000","o":"0.59","h":"0.65","l":"0.55","c":"0.62","v":"450"}"#,
        );
        let tables = normalize_jsonl_multi_interval_bars(
            &accepted,
            &single_identity(),
            &single_jsonl_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize jsonl multi-interval bars");
        assert_eq!(tables.len(), 2);
        // First-seen order: 1m group first, 1h group second.
        assert_eq!(tables[0].bar_spec.aggregation, BarAggregation::Minute);
        assert_eq!(tables[0].rows.len(), 2);
        assert_eq!(tables[1].bar_spec.aggregation, BarAggregation::Hour);
        assert_eq!(tables[1].rows.len(), 2);
        // Each table is sorted ascending by open_time.
        assert!(tables[0].rows[0].open_time < tables[0].rows[1].open_time);
        assert!(tables[1].rows[0].open_time < tables[1].rows[1].open_time);
        // Provenance: JSONL multi-interval rows stamp their OWN identity, not CSV.
        assert_eq!(
            tables[0].rows[0].transform_hash,
            compute_bar_transform_hash(JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY)
        );
    }

    #[test]
    fn jsonl_multi_interval_token_map_is_case_sensitive() {
        // A lowercase week token and an uppercase month token map to different
        // aggregations, proving the map keys case-sensitively.
        let accepted = accepted_dataset(&["interval", "t", "ct", "o", "h", "l", "c", "v"]);
        let jsonl = concat!(
            r#"{"interval":"1w","t":"1700000000000","ct":"1700604800000","o":"0.50","h":"0.70","l":"0.40","c":"0.65","v":"9000"}"#,
            "\n",
            r#"{"interval":"1M","t":"1700000000000","ct":"1702592000000","o":"0.50","h":"0.90","l":"0.30","c":"0.80","v":"40000"}"#,
        );
        let mut tables = normalize_jsonl_multi_interval_bars(
            &accepted,
            &single_identity(),
            &single_jsonl_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize week/month tokens");
        assert_eq!(tables.len(), 2);
        tables.sort_by_key(|table| table.bar_spec.aggregation as u8);
        let aggregations: Vec<BarAggregation> = tables
            .iter()
            .map(|table| table.bar_spec.aggregation)
            .collect();
        assert!(aggregations.contains(&BarAggregation::Week));
        assert!(aggregations.contains(&BarAggregation::Month));
    }

    #[test]
    fn jsonl_multi_interval_month_requires_close_time() {
        // A month period has no fixed nanosecond length, so a month row missing
        // its close_time fails loud rather than inventing one.
        let accepted = accepted_dataset(&["interval", "t", "o", "h", "l", "c", "v"]);
        let mapping = JsonlBarMappingConfig {
            close_time_field: None,
            ..single_jsonl_mapping()
        };
        let jsonl = r#"{"interval":"1M","t":"1700000000000","o":"0.50","h":"0.90","l":"0.30","c":"0.80","v":"40000"}"#;
        let err = normalize_jsonl_multi_interval_bars(
            &accepted,
            &single_identity(),
            &mapping,
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("month row without close_time must fail loud");
        assert!(err.to_string().contains("no close_time"), "{err}");
    }

    #[test]
    fn jsonl_multi_interval_rejects_unmapped_token() {
        let accepted = accepted_dataset(&["interval", "t", "ct", "o", "h", "l", "c", "v"]);
        let jsonl = r#"{"interval":"5s","t":"1700000000000","ct":"1700000005000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#;
        let err = normalize_jsonl_multi_interval_bars(
            &accepted,
            &single_identity(),
            &single_jsonl_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect_err("unmapped interval token must be rejected");
        assert!(err.to_string().contains("interval_token_map"), "{err}");
    }

    #[test]
    fn jsonl_multi_interval_splits_by_instrument_and_interval() {
        // Two instruments x two intervals -> four tables.
        let accepted = accepted_dataset(&["sym", "interval", "t", "ct", "o", "h", "l", "c", "v"]);
        let mapping = JsonlBarMappingConfig {
            instrument_field: Some("sym".to_string()),
            ..single_jsonl_mapping()
        };
        let identities = BarInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("BASEONE")),
            ("BBB".to_string(), identity("BASETWO")),
        ]));
        let jsonl = concat!(
            r#"{"sym":"AAA","interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#,
            "\n",
            r#"{"sym":"AAA","interval":"1h","t":"1700000000000","ct":"1700003600000","o":"0.50","h":"0.60","l":"0.48","c":"0.59","v":"500"}"#,
            "\n",
            r#"{"sym":"AAA","interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120"}"#,
            "\n",
            r#"{"sym":"AAA","interval":"1h","t":"1700003600000","ct":"1700007200000","o":"0.59","h":"0.65","l":"0.55","c":"0.62","v":"450"}"#,
            "\n",
            r#"{"sym":"BBB","interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.30","h":"0.33","l":"0.29","c":"0.31","v":"40"}"#,
            "\n",
            r#"{"sym":"BBB","interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.31","h":"0.34","l":"0.30","c":"0.33","v":"45"}"#,
            "\n",
            r#"{"sym":"BBB","interval":"1h","t":"1700000000000","ct":"1700003600000","o":"0.30","h":"0.38","l":"0.28","c":"0.36","v":"200"}"#,
            "\n",
            r#"{"sym":"BBB","interval":"1h","t":"1700003600000","ct":"1700007200000","o":"0.36","h":"0.40","l":"0.34","c":"0.38","v":"210"}"#,
        );
        let tables = normalize_jsonl_multi_interval_bars(
            &accepted,
            &identities,
            &mapping,
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize multi-instrument multi-interval bars");
        assert_eq!(tables.len(), 4);
        // Each table carries exactly one instrument and one interval.
        for table in &tables {
            let instrument = &table.partition.instrument_id;
            assert!(
                instrument == "BASEONE" || instrument == "BASETWO",
                "unexpected instrument {instrument}"
            );
            assert!(
                table
                    .rows
                    .iter()
                    .all(|row| &row.instrument_id == instrument)
            );
        }
    }

    #[test]
    fn jsonl_multi_interval_single_instrument_identity_path() {
        // A single-instrument object omits the instrument field and binds one
        // identity to every group.
        let accepted = accepted_dataset(&["interval", "t", "ct", "o", "h", "l", "c", "v"]);
        let jsonl = concat!(
            r#"{"interval":"1m","t":"1700000000000","ct":"1700000060000","o":"0.50","h":"0.55","l":"0.49","c":"0.52","v":"100"}"#,
            "\n",
            r#"{"interval":"1m","t":"1700000060000","ct":"1700000120000","o":"0.52","h":"0.58","l":"0.51","c":"0.57","v":"120"}"#,
        );
        let tables = normalize_jsonl_multi_interval_bars(
            &accepted,
            &single_identity(),
            &single_jsonl_mapping(),
            jsonl,
            42,
            "ingest-run-test",
        )
        .expect("normalize single-instrument jsonl bars");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].partition.instrument_id, "BASEQUOTE");
        assert_eq!(tables[0].rows.len(), 2);
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
        let err =
            normalize_csv_native_bars(&accepted, &identities, &mapping, csv, 42, "ingest-run-test")
                .expect_err("empty instrument column value must be rejected");
        assert!(
            err.to_string().contains("empty instrument"),
            "expected 'empty instrument' in error: {err}"
        );
    }

    /// Pin the CSV bar adapter's transform_hash to its current byte value.
    ///
    /// The CSV native-bar adapter stamps `BAR_TRANSFORM_IDENTITY` through the
    /// shared `assemble_bar_table`, which derives the per-row `transform_hash`
    /// via the `bar_transform_hash()` convenience wrapper. That wrapper must
    /// produce the identical digest to `compute_bar_transform_hash(BAR_TRANSFORM_IDENTITY)`
    /// so that any inadvertent change to the identity string is caught before it
    /// reaches the catalog.
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

    /// Pin the two JSON bar adapters' `transform_hash` byte values and prove all
    /// three bar adapters stamp DISTINCT provenance identities.
    ///
    /// `assemble_bar_table` derives each row's `transform_hash` from the caller's
    /// own identity constant. A regression that reverts to one hardcoded identity
    /// (the prior defect, where paged-JSON and JSONL rows carried the CSV hash)
    /// collapses these three values to one and is caught here, alongside the
    /// per-adapter row-stamp assertions in the happy-path tests.
    #[test]
    fn json_bar_adapters_pin_distinct_transform_identities() {
        let csv = compute_bar_transform_hash(BAR_TRANSFORM_IDENTITY);
        let paged = compute_bar_transform_hash(PAGED_JSON_BARS_TRANSFORM_IDENTITY);
        let jsonl = compute_bar_transform_hash(JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY);
        // Computed from: sha256("paged-json-bars-to-canonical-bars.v1")
        assert_eq!(
            paged, "757b787a41dad91affcfd2abc57b4f36b649bbbb9cd26481b213ea1792de9219",
            "PAGED_JSON_BARS_TRANSFORM_IDENTITY hash changed — update this pin or revert the identity change"
        );
        // Computed from: sha256("jsonl-multi-interval-bars-to-canonical-bars.v1")
        assert_eq!(
            jsonl, "40ded09a46e7143c1759d93576fcee7d8a54267f726c87ff3337ec143d746822",
            "JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY hash changed — update this pin or revert the identity change"
        );
        assert_ne!(
            csv, paged,
            "CSV and paged-JSON bar identities must not collide"
        );
        assert_ne!(csv, jsonl, "CSV and JSONL bar identities must not collide");
        assert_ne!(
            paged, jsonl,
            "paged-JSON and JSONL bar identities must not collide"
        );
    }
}
