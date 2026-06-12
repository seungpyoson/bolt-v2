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
//! This slice implements the periodic-full-snapshot wire shape
//! ([`DeltaSourceFormat::Snapshot`]) over two container entry points that share
//! one group-and-expand core: a single decoded JSONL object
//! ([`normalize_jsonl_snapshot_deltas`]) and a streaming gzip-tar of JSONL
//! members ([`normalize_tar_jsonl_snapshot_deltas`]). The tar path accumulates
//! per-instrument groups across every member of the archive (a single pass, the
//! same multi-instrument split and exclusion fence as the JSONL path) and then
//! runs the identical per-group expansion.
//!
//! It also implements the typed-event-stream wire shape
//! ([`DeltaSourceFormat::EventStream`], [`normalize_parquet_event_stream_deltas`])
//! over a Parquet container. Here one staged object interleaves typed events
//! (snapshot / level-change / trade / tick-size-change) for many instruments,
//! keyed by an asset-key column, and **dual-emits** both the
//! [`CanonicalOrderBookDeltasTable`] family (from the snapshot and level-change
//! events) AND the [`super::canonical_trades::CanonicalTradesTable`] family (from
//! the trade events). Snapshot events reuse the identical [`expand_photos`] core;
//! level-change events become standalone single-delta book events; trade events
//! accumulate into per-instrument trades rows.
//!
//! # Dual-fidelity rule
//!
//! An event-stream object is accepted as ONE L2 archive
//! ([`SourceProofFidelityClass::L2Replay`]), so the accepted dataset stays
//! `L2Replay` — it IS an L2 archive. The single source of truth for L2
//! admissibility is therefore the accepted dataset, and the produced
//! [`CanonicalOrderBookDeltasTable`]s inherit
//! [`AcceptedDataset::fidelity_class`] (= `L2Replay`) and
//! [`AcceptedDataset::forbidden_claims`] verbatim, exactly as the snapshot paths
//! do — that is what [`CanonicalOrderBookDeltasTable::validate`] requires.
//!
//! The trade prints carried in the same object ARE trade-grade evidence, and
//! [`super::canonical_trades::CanonicalTradesTable::validate`] forbids the
//! `L2_REPLAY` label and requires explicit forbidden claims. The accepted dataset
//! cannot supply a second (trade-grade) fidelity, so each emitted trades table
//! sets [`SourceProofFidelityClass::TradeReplay`] and draws its forbidden claims
//! from the REQUIRED, non-empty `trade_forbidden_claims` field of the
//! [`DeltaSourceFormat::EventStream`] mapping. The trades-claims list is a
//! run-spec owned config fact (the converter declares what a trade event proves),
//! not a hardcode and not a re-acceptance: the L2 source proof remains the single
//! authority for the archive, and the converter declares the trade family's
//! fidelity/claims as part of the format that produces them.
//!
//! Input is only ever an [`AcceptedDataset`] from gate 1 — raw staged data never
//! reaches this module without first passing source-proof acceptance.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use arrow::{
    array::{Array, Int64Array, StringArray},
    record_batch::RecordBatch,
};
use bytes::Bytes;
use nautilus_model::enums::RecordFlag;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
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
        CanonicalInstrumentIdentity, CanonicalTradeRow, CanonicalTradesTable, CsvTimestampUnit,
        DELTAS_TRANSFORM_IDENTITY, EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
        TRADE_SOURCE_TYPE_NATIVE, TradeAggressorSide, TradesPartition,
    },
    source_proof::{AcceptedDataset, SourceProofFidelityClass},
    tar_reader::TarMember,
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
/// [`DeltaSourceFormat::Snapshot`] is the periodic-full-snapshot family — one
/// full L2 photo per JSONL line (single object or tar-of-JSONL container).
/// [`DeltaSourceFormat::EventStream`] is the typed-event family — one Parquet
/// stream that interleaves snapshot / level-change / trade / tick-size-change
/// events for many instruments, keyed by an asset-key column. The two families
/// are mutually exclusive: the snapshot entry points reject an `EventStream`
/// mapping and the event-stream entry point rejects a `Snapshot` mapping, both
/// failing loud (mirroring the per-kind dispatcher fences).
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
    /// A typed-event Parquet stream. Each row is one typed event whose kind is
    /// the value of `event_type_field`:
    ///
    /// - `snapshot_event_value` — a full photo. `bids_field` / `asks_field` each
    ///   hold a JSON string of `[price, size]` string pairs (the archive shape:
    ///   `"[[\"0.49\",\"10\"]]"`); an empty array string `"[]"` on both sides is
    ///   a genuine empty book (a lone CLEAR through the shared expansion rules).
    /// - `level_change_event_value` — a single price-level update on
    ///   `side_field`/`price_field`/`size_field`. By the NT level-set convention
    ///   (no book state is reconstructed here) a positive size is an `UPDATE` at
    ///   the absolute level and a zero size is a `DELETE`; each is its own
    ///   self-closing book event carrying `F_LAST`.
    /// - `trade_event_value` — a trade print on
    ///   `side_field`/`trade_price_field`/`trade_size_field`, accumulated into the
    ///   dual trades table family (see the module's dual-fidelity rule).
    /// - any value in `dropped_event_values` (for example a tick-size change) —
    ///   accepted and produces no row.
    ///
    /// Any other value bails loud. `side_field` decodes through
    /// `buy_side_values` / `sell_side_values`. Replay ordering uses
    /// `capture_time_field` (the monotonic ingest clock) with a stable row-index
    /// tiebreak; `tiebreak_is_row_index` documents and pins that tiebreak (it is
    /// always `true` — physical row order breaks exact capture-time ties — and is
    /// declared in config so the ordering authority has a single visible owner).
    /// `trade_id_field`, when set, supplies the trade id; otherwise a synthetic
    /// per-instrument ordinal is used (see
    /// [`normalize_parquet_event_stream_deltas`]). `trade_forbidden_claims` is
    /// the non-empty forbidden-claims list stamped onto every emitted trades
    /// table under the dual-fidelity rule.
    EventStream {
        event_type_field: String,
        snapshot_event_value: String,
        level_change_event_value: String,
        trade_event_value: String,
        dropped_event_values: Vec<String>,
        side_field: String,
        buy_side_values: Vec<String>,
        sell_side_values: Vec<String>,
        price_field: String,
        size_field: String,
        bids_field: String,
        asks_field: String,
        capture_time_field: String,
        capture_time_unit: CsvTimestampUnit,
        tiebreak_is_row_index: bool,
        trade_price_field: String,
        trade_size_field: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trade_id_field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_time_field: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_time_unit: Option<CsvTimestampUnit>,
        trade_forbidden_claims: Vec<String>,
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
/// [`OrderingAuthority::EventTime`] (the snapshot families): rows keep their
/// photo order and the per-instrument exchange event time must be
/// non-decreasing.
///
/// [`OrderingAuthority::CaptureTime`] (the event-stream family): the canonical
/// `event_time` is the monotonic ingest **capture** clock (the replay-ordering
/// clock), per instrument stably sorted by `(capture_time, source_row_index)`.
/// The original exchange time, when the format carries it, is preserved as each
/// row's `availability_time` rather than driving the order — capture time is the
/// only clock guaranteed monotonic across a multi-instrument event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderingAuthority {
    EventTime,
    CaptureTime,
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
    fn resolve(&self, instrument_key: Option<&str>) -> Result<&CanonicalInstrumentIdentity> {
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

/// Lowercase SHA-256 hex of the order-book-delta transform identity.
#[must_use]
pub fn delta_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(DELTAS_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// One parsed level of a photo (exact source decimal strings).
struct ParsedLevel {
    price: String,
    size: String,
}

/// One parsed photo, before identity/provenance assembly.
///
/// `event_time` is the per-instrument ordering clock (exchange event time for the
/// snapshot families, capture time for the event-stream family).
/// `availability_time` carries the original exchange time when the ordering clock
/// is the capture clock and the format declares an exchange time; it is `None`
/// for the snapshot families (where `event_time` already IS the exchange time).
struct ParsedPhoto {
    event_time: i64,
    availability_time: Option<i64>,
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
    let fields = validate_snapshot_mapping(mapping, ingest_run_id)?;
    let mut accumulator = PhotoGroups::default();
    parse_jsonl_into_groups(&fields, mapping, jsonl_text, &mut accumulator)?;
    expand_groups_into_tables(
        accepted,
        identities,
        mapping,
        accumulator,
        capture_time_nanos,
        ingest_run_id,
        SnapshotContainer::SingleObject,
    )
}

/// Normalize an accepted streaming gzip-tar of JSONL periodic-full-snapshot
/// members into one [`CanonicalOrderBookDeltasTable`] per instrument.
///
/// `members` is the streaming member iterator produced by
/// [`super::tar_reader::gzip_tar_members`]; the caller owns the container concern
/// (decompress + walk tar members) exactly as the JSONL path's caller owns
/// decoding a single object. Each member's text is parsed into the same
/// per-instrument accumulator the JSONL path uses, so grouping, the
/// multi-instrument split, and the exclusion fence span the *whole archive* in a
/// single pass. After accumulation every group's photos are expanded through the
/// identical [`expand_photos`] core.
///
/// Archive member order is not a per-instrument event-time order guarantee:
/// distinct members can interleave the same instrument with non-monotonic
/// boundaries (e.g. one member per minute, each carrying all instruments). Each
/// group's photos are therefore stably sorted by event time before expansion, so
/// the per-instrument timeline is monotonic regardless of how members were
/// chunked. The regressing-event-time bail stays *inside* [`expand_photos`] so
/// exact-tie semantics match the JSONL path.
///
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run that produced this
/// normalization, recorded for lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if `members` yields a streaming/tar failure, a member line
/// is malformed JSON, a configured field is missing, a level price/size is
/// non-positive, the keyed identity is unknown, the archive carries no in-scope
/// photo, or a produced table fails its contract.
pub fn normalize_tar_jsonl_snapshot_deltas(
    accepted: &AcceptedDataset,
    identities: &DeltaInstrumentIdentities,
    mapping: &DeltaMappingConfig,
    members: impl Iterator<Item = Result<TarMember>>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<CanonicalOrderBookDeltasTable>> {
    let fields = validate_snapshot_mapping(mapping, ingest_run_id)?;
    let mut accumulator = PhotoGroups::default();
    for member in members {
        let member = member.context("read next tar member")?;
        parse_jsonl_into_groups(&fields, mapping, &member.text, &mut accumulator)
            .with_context(|| format!("normalize tar member {:?}", member.name))?;
    }
    expand_groups_into_tables(
        accepted,
        identities,
        mapping,
        accumulator,
        capture_time_nanos,
        ingest_run_id,
        SnapshotContainer::TarArchive,
    )
}

/// Normalize an accepted typed-event Parquet stream into BOTH a per-instrument
/// [`CanonicalOrderBookDeltasTable`] family AND a per-instrument
/// [`CanonicalTradesTable`] family.
///
/// `parquet_bytes` is the decoded bytes of the accepted object whose hash already
/// matched the manifest (the caller verified it via gate 1). Decoding is via the
/// in-memory arrow reader idiom: the bytes are wrapped in a [`Bytes`] chunk
/// reader and walked batch by batch ([`decode_event_stream_rows`]), all column
/// names sourced from the [`DeltaSourceFormat::EventStream`] mapping.
///
/// Per instrument (the `asset_key` column, with the exclusion fence applied), the
/// rows are stably sorted by `(capture_time, source_row_index)` so the replay
/// timeline is monotonic regardless of physical layout. Snapshot events expand
/// through the shared [`expand_photos`] core (the lone-CLEAR and
/// adds-only-after-established-empty encoding apply); level-change events become
/// standalone single-delta book events (`UPDATE` at the absolute level when
/// size > 0, `DELETE` when size == 0, each self-closing with `F_LAST`); trade
/// events accumulate into the dual trades rows; dropped event values produce
/// nothing; unknown event values bail loud.
///
/// The two families bind different fidelities under the module's dual-fidelity
/// rule: the deltas tables inherit the accepted dataset's `L2_REPLAY` fidelity
/// and forbidden claims, while each trades table is `TRADE_REPLAY` with the
/// `trade_forbidden_claims` declared in the mapping. The synthetic trade id, when
/// `trade_id_field` is unset, is the per-instrument 0-based ordinal of the trade
/// event rendered as a decimal string — short, deterministic, and unique within
/// the instrument's trades table (a [`nautilus_model`] `TradeId` is capped at 36
/// characters, which the ordinal never exceeds; a configured `trade_id_field` is
/// used verbatim and fails loud at projection if it is over-length).
///
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run that produced this
/// normalization, recorded for lineage; it is not the source object URL.
///
/// # Errors
///
/// Returns an error if the mapping is not an `EventStream` format, the Parquet
/// bytes are unreadable, a configured column is missing or wrongly typed, an
/// event value is unknown, a level/trade price or size is invalid, the keyed
/// identity is unknown, the object carries no in-scope event, or a produced table
/// fails its contract.
pub fn normalize_parquet_event_stream_deltas(
    accepted: &AcceptedDataset,
    identities: &DeltaInstrumentIdentities,
    mapping: &DeltaMappingConfig,
    parquet_bytes: &[u8],
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<(
    Vec<CanonicalOrderBookDeltasTable>,
    Vec<CanonicalTradesTable>,
)> {
    let fields = validate_event_stream_mapping(mapping, accepted, ingest_run_id)?;
    let raw_rows = decode_event_stream_rows(&fields, parquet_bytes)?;
    expand_event_stream_into_tables(
        accepted,
        identities,
        mapping,
        &fields,
        raw_rows,
        capture_time_nanos,
        ingest_run_id,
    )
}

/// The validated, borrowed event-stream field mapping, resolved once before any
/// Parquet row is decoded. `trade_forbidden_claims` is cloned (not borrowed) so
/// the stamping of each trades table does not pin the mapping borrow across table
/// assembly.
struct EventStreamFields<'a> {
    event_type_field: &'a str,
    snapshot_event_value: &'a str,
    level_change_event_value: &'a str,
    trade_event_value: &'a str,
    dropped_event_values: &'a [String],
    side_field: &'a str,
    buy_side_values: &'a [String],
    sell_side_values: &'a [String],
    price_field: &'a str,
    size_field: &'a str,
    bids_field: &'a str,
    asks_field: &'a str,
    capture_time_field: &'a str,
    capture_time_unit: CsvTimestampUnit,
    trade_price_field: &'a str,
    trade_size_field: &'a str,
    trade_id_field: Option<&'a str>,
    event_time_field: Option<&'a str>,
    event_time_unit: Option<CsvTimestampUnit>,
    asset_key_field: Option<&'a str>,
    exclusion_filter: Option<&'a InstrumentExclusionFilter>,
    trade_forbidden_claims: Vec<String>,
}

/// Run every event-stream mapping invariant: field/value presence, distinct event
/// values, the non-empty trade-claims rule, the paired exchange-time
/// field/unit, the always-on row-index tiebreak, and the keyed `key_field`.
///
/// Split from [`validate_event_stream_mapping`] so each function stays focused;
/// the caller owns the L2 precondition and the borrowed-field construction.
fn check_event_stream_mapping(mapping: &DeltaMappingConfig) -> Result<()> {
    let DeltaSourceFormat::EventStream {
        event_type_field,
        snapshot_event_value,
        level_change_event_value,
        trade_event_value,
        dropped_event_values,
        side_field,
        buy_side_values,
        sell_side_values,
        price_field,
        size_field,
        bids_field,
        asks_field,
        capture_time_field,
        capture_time_unit: _,
        tiebreak_is_row_index,
        trade_price_field,
        trade_size_field,
        trade_id_field,
        event_time_field,
        event_time_unit,
        trade_forbidden_claims,
    } = &mapping.format
    else {
        bail!("event-stream mapping check requires an EventStream format mapping");
    };

    // The row-index tiebreak is the only tie-resolution this family supports and
    // is always on: physical Parquet order breaks exact capture-time ties so the
    // lone-CLEAR/adds-only collapse is deterministic. Config declares it so the
    // ordering authority has one visible owner; a `false` is a configuration
    // error rather than a silently-different ordering.
    ensure!(
        *tiebreak_is_row_index,
        "converter deltas.tiebreak_is_row_index must be true: physical row order is \
         the only supported capture-time tiebreak"
    );

    for (label, field) in [
        ("event_type_field", event_type_field),
        ("snapshot_event_value", snapshot_event_value),
        ("level_change_event_value", level_change_event_value),
        ("trade_event_value", trade_event_value),
        ("side_field", side_field),
        ("price_field", price_field),
        ("size_field", size_field),
        ("bids_field", bids_field),
        ("asks_field", asks_field),
        ("capture_time_field", capture_time_field),
        ("trade_price_field", trade_price_field),
        ("trade_size_field", trade_size_field),
    ] {
        ensure!(
            !field.trim().is_empty(),
            "converter deltas.{label} must not be empty"
        );
    }
    ensure!(
        !buy_side_values.is_empty(),
        "converter deltas.buy_side_values must not be empty"
    );
    ensure!(
        !sell_side_values.is_empty(),
        "converter deltas.sell_side_values must not be empty"
    );
    for value in buy_side_values.iter().chain(sell_side_values.iter()) {
        ensure!(
            !value.trim().is_empty(),
            "converter deltas side values must not be empty"
        );
    }
    // The three event discriminators and every dropped value must be distinct so
    // an event value resolves to exactly one branch.
    let mut seen = std::collections::BTreeSet::new();
    for value in [
        snapshot_event_value.as_str(),
        level_change_event_value.as_str(),
        trade_event_value.as_str(),
    ]
    .into_iter()
    .chain(dropped_event_values.iter().map(String::as_str))
    {
        ensure!(
            !value.trim().is_empty(),
            "converter deltas event values must not be empty"
        );
        ensure!(
            seen.insert(value),
            "converter deltas event value {value:?} is declared more than once"
        );
    }
    // Dual-fidelity rule: the trades tables cannot inherit the L2 forbidden
    // claims (they carry TRADE_REPLAY), so the mapping MUST declare a non-empty
    // trade-claims list whenever the format can emit a trade event.
    ensure!(
        !trade_forbidden_claims.is_empty(),
        "converter deltas.trade_forbidden_claims must be non-empty: an EventStream \
         format declares a trade event, and its trades tables carry TRADE_REPLAY \
         fidelity which requires explicit forbidden claims"
    );
    for claim in trade_forbidden_claims {
        ensure!(
            !claim.trim().is_empty(),
            "converter deltas.trade_forbidden_claims entries must not be empty"
        );
    }

    if let Some(trade_id_field) = trade_id_field {
        ensure!(
            !trade_id_field.trim().is_empty(),
            "converter deltas.trade_id_field must not be empty when set"
        );
    }
    // event_time_field and event_time_unit travel together: an exchange time
    // column needs its unit to convert into availability_time, and a unit with no
    // column has nothing to convert.
    ensure!(
        event_time_field.is_some() == event_time_unit.is_some(),
        "converter deltas.event_time_field and deltas.event_time_unit must both be \
         set or both be absent"
    );
    if let Some(event_time_field) = event_time_field {
        ensure!(
            !event_time_field.trim().is_empty(),
            "converter deltas.event_time_field must not be empty when set"
        );
    }
    if let Some(key_field) = &mapping.instrument_key.key_field {
        ensure!(
            !key_field.trim().is_empty(),
            "converter deltas.instrument_key.key_field must not be empty when set"
        );
    }
    Ok(())
}

/// Validate `ingest_run_id`, the dual-fidelity precondition, and the event-stream
/// field mapping, returning the borrowed field set the decoder reads against.
fn validate_event_stream_mapping<'a>(
    mapping: &'a DeltaMappingConfig,
    accepted: &AcceptedDataset,
    ingest_run_id: &str,
) -> Result<EventStreamFields<'a>> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    // Dual-fidelity precondition: the accepted dataset must be the L2 archive the
    // deltas tables inherit. The trades tables derive TRADE_REPLAY from the
    // mapping, so the accepted side stays the single L2 authority.
    ensure!(
        accepted.fidelity_class == SourceProofFidelityClass::L2Replay,
        "event-stream deltas require an L2_REPLAY accepted dataset, got {:?}",
        accepted.fidelity_class
    );

    let DeltaSourceFormat::EventStream {
        event_type_field,
        snapshot_event_value,
        level_change_event_value,
        trade_event_value,
        dropped_event_values,
        side_field,
        buy_side_values,
        sell_side_values,
        price_field,
        size_field,
        bids_field,
        asks_field,
        capture_time_field,
        capture_time_unit,
        tiebreak_is_row_index: _,
        trade_price_field,
        trade_size_field,
        trade_id_field,
        event_time_field,
        event_time_unit,
        trade_forbidden_claims,
    } = &mapping.format
    else {
        bail!(
            "event-stream delta path requires an EventStream format mapping; the \
             Snapshot format must use the JSONL/tar snapshot entry points, not \
             normalize_parquet_event_stream_deltas"
        );
    };

    check_event_stream_mapping(mapping)?;

    Ok(EventStreamFields {
        event_type_field,
        snapshot_event_value,
        level_change_event_value,
        trade_event_value,
        dropped_event_values,
        side_field,
        buy_side_values,
        sell_side_values,
        price_field,
        size_field,
        bids_field,
        asks_field,
        capture_time_field,
        capture_time_unit: *capture_time_unit,
        trade_price_field,
        trade_size_field,
        trade_id_field: trade_id_field.as_deref(),
        event_time_field: event_time_field.as_deref(),
        event_time_unit: *event_time_unit,
        asset_key_field: mapping.instrument_key.key_field.as_deref(),
        exclusion_filter: mapping.instrument_key.exclusion_filter.as_ref(),
        trade_forbidden_claims: trade_forbidden_claims.clone(),
    })
}

/// One raw decoded typed-event row, storage-agnostic so the expansion exercises
/// identical logic in tests and in the runner.
///
/// Every payload column is read as an exact source string (empty when the column
/// was null for the row), preserving source decimals verbatim. `capture_time` is
/// the ingest clock in Unix nanoseconds, `availability_time` the optional source
/// exchange time, and `source_row_index` the physical row order used as the tie
/// break.
struct RawEventRow {
    instrument_key: Option<String>,
    event_type: String,
    capture_time: i64,
    availability_time: Option<i64>,
    source_row_index: u64,
    bids: String,
    asks: String,
    price: String,
    size: String,
    side: String,
    trade_price: String,
    trade_size: String,
    trade_id: String,
}

/// Decode the accepted Parquet bytes into raw typed-event rows.
///
/// Wraps the bytes in an in-memory [`Bytes`] chunk reader (the same
/// `ParquetRecordBatchReaderBuilder` idiom the catalog read-back uses for a file)
/// and reads each batch column by column with the configured names. The asset-key
/// column is read only when the mapping keys instruments; the optional exchange
/// time column only when declared.
fn decode_event_stream_rows(
    fields: &EventStreamFields<'_>,
    parquet_bytes: &[u8],
) -> Result<Vec<RawEventRow>> {
    let reader = ParquetRecordBatchReaderBuilder::try_new(Bytes::from(parquet_bytes.to_vec()))
        .context("construct event-stream parquet reader")?
        .build()
        .context("build event-stream record batch reader")?;

    let mut rows = Vec::new();
    let mut source_row_index: u64 = 0;
    for batch in reader {
        let batch = batch.context("read event-stream parquet batch")?;
        decode_event_stream_batch(fields, &batch, &mut rows, &mut source_row_index)?;
    }
    Ok(rows)
}

/// Decode one Parquet batch's rows into [`RawEventRow`]s.
fn decode_event_stream_batch(
    fields: &EventStreamFields<'_>,
    batch: &RecordBatch,
    rows: &mut Vec<RawEventRow>,
    source_row_index: &mut u64,
) -> Result<()> {
    for row in 0..batch.num_rows() {
        let capture_raw = required_string_cell(batch, fields.capture_time_field, row)?;
        let capture_time = fields
            .capture_time_unit
            .parse_to_nanos(&capture_raw)
            .with_context(|| format!("row {row}: invalid capture time {capture_raw:?}"))?;
        ensure!(capture_time > 0, "row {row}: non-positive capture time");

        let availability_time = match (fields.event_time_field, fields.event_time_unit) {
            (Some(field), Some(unit)) => {
                let raw = optional_string_cell(batch, field, row)?;
                match raw {
                    Some(raw) if !raw.trim().is_empty() => {
                        let nanos = unit
                            .parse_to_nanos(&raw)
                            .with_context(|| format!("row {row}: invalid exchange time {raw:?}"))?;
                        ensure!(nanos > 0, "row {row}: non-positive exchange time");
                        Some(nanos)
                    }
                    _ => None,
                }
            }
            _ => None,
        };

        let instrument_key = match fields.asset_key_field {
            Some(field) => Some(required_string_cell(batch, field, row)?),
            None => None,
        };

        rows.push(RawEventRow {
            instrument_key,
            event_type: required_string_cell(batch, fields.event_type_field, row)?,
            capture_time,
            availability_time,
            source_row_index: *source_row_index,
            bids: optional_string_cell(batch, fields.bids_field, row)?.unwrap_or_default(),
            asks: optional_string_cell(batch, fields.asks_field, row)?.unwrap_or_default(),
            price: optional_string_cell(batch, fields.price_field, row)?.unwrap_or_default(),
            size: optional_string_cell(batch, fields.size_field, row)?.unwrap_or_default(),
            side: optional_string_cell(batch, fields.side_field, row)?.unwrap_or_default(),
            trade_price: optional_string_cell(batch, fields.trade_price_field, row)?
                .unwrap_or_default(),
            trade_size: optional_string_cell(batch, fields.trade_size_field, row)?
                .unwrap_or_default(),
            trade_id: match fields.trade_id_field {
                Some(field) => optional_string_cell(batch, field, row)?.unwrap_or_default(),
                None => String::new(),
            },
        });
        *source_row_index = source_row_index
            .checked_add(1)
            .context("event-stream row index overflow")?;
    }
    Ok(())
}

/// Read a required Utf8 cell, failing loud on a missing column, wrong type, or
/// null value.
fn required_string_cell(batch: &RecordBatch, column: &str, row: usize) -> Result<String> {
    optional_string_cell(batch, column, row)?
        .with_context(|| format!("event-stream column {column:?} is null at row {row}"))
}

/// Read an optional Utf8 cell, returning `None` for a null value. Integer
/// capture/exchange columns are also accepted (rendered to their decimal string)
/// so the timestamp parser sees a uniform string the same way the JSONL path
/// renders a numeric event time before parsing.
fn optional_string_cell(batch: &RecordBatch, column: &str, row: usize) -> Result<Option<String>> {
    let values = batch
        .column_by_name(column)
        .with_context(|| format!("event-stream parquet missing column {column:?}"))?;
    if values.is_null(row) {
        return Ok(None);
    }
    if let Some(strings) = values.as_any().downcast_ref::<StringArray>() {
        return Ok(Some(strings.value(row).to_string()));
    }
    if let Some(integers) = values.as_any().downcast_ref::<Int64Array>() {
        return Ok(Some(integers.value(row).to_string()));
    }
    bail!("event-stream column {column:?} is not Utf8 or Int64")
}

/// Per-instrument accumulator for the dual expansion: ordered group keys plus the
/// raw rows owned per group.
#[derive(Default)]
struct EventGroups {
    order: Vec<Option<String>>,
    groups: BTreeMap<Option<String>, Vec<RawEventRow>>,
}

/// Split the decoded rows per instrument (applying the exclusion fence) and run
/// the dual expansion for every group, returning the deltas and trades families.
fn expand_event_stream_into_tables(
    accepted: &AcceptedDataset,
    identities: &DeltaInstrumentIdentities,
    mapping: &DeltaMappingConfig,
    fields: &EventStreamFields<'_>,
    raw_rows: Vec<RawEventRow>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<(
    Vec<CanonicalOrderBookDeltasTable>,
    Vec<CanonicalTradesTable>,
)> {
    let mut accumulator = EventGroups::default();
    // Borrow the two fields separately so the `or_insert_with` closure pushes to
    // `order` while `groups.entry` holds its own disjoint mutable borrow (the same
    // pattern parse_jsonl_into_groups uses).
    let EventGroups { order, groups } = &mut accumulator;
    for mut raw in raw_rows {
        if let Some(filter) = fields.exclusion_filter
            && let Some(key) = raw.instrument_key.as_deref()
            && filter.excludes(key)
        {
            continue;
        }
        if fields.asset_key_field.is_some() {
            let key = raw
                .instrument_key
                .as_deref()
                .context("event-stream row missing keyed asset value")?;
            ensure!(
                !key.trim().is_empty(),
                "event-stream row has empty asset key"
            );
        } else {
            // Single-instrument shape: every row keys onto the None group even if
            // the decoder happened to read an unconfigured key column.
            raw.instrument_key = None;
        }
        let key = raw.instrument_key.clone();
        let group = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key);
            Vec::new()
        });
        group.push(raw);
    }
    ensure!(
        !accumulator.order.is_empty(),
        "event-stream object yielded no in-scope events"
    );

    let canonical_instrument_key_prefix = format!("{}/{}", accepted.venue, accepted.product_family);
    let delta_transform_hash = delta_transform_hash();
    let trade_transform_hash = event_stream_trade_transform_hash();

    let mut delta_tables = Vec::with_capacity(accumulator.order.len());
    let mut trade_tables = Vec::new();
    for instrument_key in &accumulator.order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let canonical_instrument_key = format!(
            "{canonical_instrument_key_prefix}/{}",
            identity.instrument_id
        );
        let mut group = accumulator
            .groups
            .remove(instrument_key)
            .expect("group order entry has a populated group");
        // Capture-time replay order with the physical row-index tiebreak.
        group.sort_by_key(|raw| (raw.capture_time, raw.source_row_index));

        let mut events: Vec<DeltaEvent> = Vec::new();
        let mut trade_rows: Vec<CanonicalTradeRow> = Vec::new();
        let delta_provenance = RowProvenance {
            accepted,
            identity,
            canonical_instrument_key: &canonical_instrument_key,
            transform_hash: &delta_transform_hash,
            capture_time_nanos,
            ingest_run_id,
        };
        let decode_ctx = EventDecodeContext {
            fields,
            provenance: &delta_provenance,
            trade_transform_hash: &trade_transform_hash,
            price_sign_policy: mapping.price_sign_policy,
        };
        for raw in &group {
            decode_event_into(&decode_ctx, raw, &mut events, &mut trade_rows)?;
        }

        let rows = expand_delta_events(
            &delta_provenance,
            mapping.ordering,
            mapping.empty_book_policy,
            &events,
        )?;
        let delta_table = CanonicalOrderBookDeltasTable {
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
            // Dual-fidelity rule: deltas inherit the accepted L2 archive's
            // fidelity and forbidden claims verbatim.
            fidelity_class: accepted.fidelity_class,
            forbidden_claims: accepted.forbidden_claims.clone(),
            transform_hash: delta_transform_hash.clone(),
            payload_hash: accepted.object.sha256.clone(),
            rows,
        };
        delta_table.validate()?;
        delta_tables.push(delta_table);

        if !trade_rows.is_empty() {
            let trade_table = CanonicalTradesTable {
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
                // Dual-fidelity rule: the prints are trade-grade evidence, so the
                // trades table is TRADE_REPLAY with the mapping-declared claims —
                // the accepted L2 dataset cannot carry a second fidelity.
                fidelity_class: SourceProofFidelityClass::TradeReplay,
                forbidden_claims: fields.trade_forbidden_claims.clone(),
                transform_hash: trade_transform_hash.clone(),
                payload_hash: accepted.object.sha256.clone(),
                rows: trade_rows,
            };
            trade_table.validate()?;
            trade_tables.push(trade_table);
        }
    }

    Ok((delta_tables, trade_tables))
}

/// One decoded book-mutating event on an instrument's unified replay timeline.
///
/// Snapshot and level-change events share ONE per-instrument delta stream (one
/// dense sequence, one monotonic capture-time check); trade events leave this
/// timeline entirely and accumulate into the dual trades family. Keeping both
/// book-mutating kinds in one ordered list is what lets the lone-CLEAR collapse
/// and the standalone single-delta events interleave correctly under capture-time
/// order.
enum DeltaEvent {
    /// A full photo, expanded through the shared snapshot rules.
    Snapshot(ParsedPhoto),
    /// A single price-level change: one self-closing book event.
    Standalone(StandaloneDelta),
}

impl DeltaEvent {
    /// The capture-time ordering clock for the event.
    const fn event_time(&self) -> i64 {
        match self {
            Self::Snapshot(photo) => photo.event_time,
            Self::Standalone(delta) => delta.event_time,
        }
    }
}

/// A standalone single-level delta event (a level change): its own self-closing
/// book event carrying `F_LAST` on its single row.
struct StandaloneDelta {
    event_time: i64,
    availability_time: Option<i64>,
    action: DeltaAction,
    side: DeltaSide,
    price: String,
    size: String,
}

/// Per-group decode context: the invariants every event row of one instrument
/// decodes against, bundled so [`decode_event_into`] takes one reference rather
/// than a long argument list (and the per-group constants have one owner).
struct EventDecodeContext<'a, 'f> {
    fields: &'a EventStreamFields<'f>,
    provenance: &'a RowProvenance<'a>,
    trade_transform_hash: &'a str,
    price_sign_policy: DeltaPriceSignPolicy,
}

/// Decode one raw typed-event row, appending to the unified delta timeline
/// (snapshot or level change), the trades rows (trade), or nothing (dropped).
/// Unknown event values bail loud.
fn decode_event_into(
    ctx: &EventDecodeContext<'_, '_>,
    raw: &RawEventRow,
    events: &mut Vec<DeltaEvent>,
    trade_rows: &mut Vec<CanonicalTradeRow>,
) -> Result<()> {
    let fields = ctx.fields;
    let event_type = raw.event_type.trim();
    if event_type == fields.snapshot_event_value {
        let bids = parse_event_stream_levels(&raw.bids, fields.bids_field, ctx.price_sign_policy)?;
        let asks = parse_event_stream_levels(&raw.asks, fields.asks_field, ctx.price_sign_policy)?;
        events.push(DeltaEvent::Snapshot(ParsedPhoto {
            event_time: raw.capture_time,
            availability_time: raw.availability_time,
            bids,
            asks,
        }));
    } else if event_type == fields.level_change_event_value {
        let side = parse_event_stream_side(fields, &raw.side)?;
        let (price, size, size_dec) = parse_price_size(&raw.price, &raw.size, "level_change")?;
        // No book state is reconstructed here: by the NT level-set convention a
        // positive size sets the absolute level (UPDATE), a zero size removes it
        // (DELETE). Each level change is its own self-closing book event.
        let action = if size_dec.is_zero() {
            DeltaAction::Delete
        } else {
            DeltaAction::Update
        };
        events.push(DeltaEvent::Standalone(StandaloneDelta {
            event_time: raw.capture_time,
            availability_time: raw.availability_time,
            action,
            side,
            price,
            size,
        }));
    } else if event_type == fields.trade_event_value {
        trade_rows.push(decode_trade_row(ctx, raw, trade_rows.len())?);
    } else if fields
        .dropped_event_values
        .iter()
        .any(|value| value == event_type)
    {
        // Accepted no-op (for example a tick-size change): produces no row.
    } else {
        bail!("unknown event-stream event_type: {event_type:?}");
    }
    Ok(())
}

/// Decode one trade event row into a canonical trades row.
fn decode_trade_row(
    ctx: &EventDecodeContext<'_, '_>,
    raw: &RawEventRow,
    trade_ordinal: usize,
) -> Result<CanonicalTradeRow> {
    let provenance = ctx.provenance;
    let aggressor = parse_trade_aggressor(ctx.fields, &raw.side)?;
    let (price, size, size_dec) = parse_price_size(&raw.trade_price, &raw.trade_size, "trade")?;
    ensure!(
        size_dec > Decimal::ZERO,
        "trade: non-positive size {size:?}"
    );
    let price_dec: Decimal = price.parse().context("trade price parse")?;
    let notional = price_dec
        .checked_mul(size_dec)
        .context("trade notional overflow")?;
    let trade_id = resolve_trade_id(ctx.fields, raw, trade_ordinal);
    Ok(CanonicalTradeRow {
        schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
        ingest_run_id: provenance.ingest_run_id.to_string(),
        source_binding: provenance.accepted.source_binding.clone(),
        venue: provenance.accepted.venue.clone(),
        product_family: provenance.accepted.product_family.clone(),
        product_category: provenance.accepted.product_category.clone(),
        instrument_id: provenance.identity.instrument_id.clone(),
        canonical_instrument_key: provenance.canonical_instrument_key.to_string(),
        venue_symbol: provenance.identity.venue_symbol.clone(),
        nt_instrument_id: Some(provenance.identity.nt_instrument_id.clone()),
        event_time: raw.capture_time,
        capture_time: provenance.capture_time_nanos,
        availability_time: raw.availability_time,
        source_sequence: Some(trade_id.clone()),
        raw_payload_id: provenance.accepted.object.sha256.clone(),
        source_proof_id: provenance.accepted.source_proof_id.clone(),
        payload_hash: provenance.accepted.object.sha256.clone(),
        transform_hash: ctx.trade_transform_hash.to_string(),
        trade_source_type: TRADE_SOURCE_TYPE_NATIVE.to_string(),
        trade_id,
        aggressor_side: aggressor.as_str().to_string(),
        price,
        size,
        notional: notional.normalize().to_string(),
    })
}

/// Expand an instrument's unified, capture-time-ordered event timeline into
/// canonical delta rows with one dense sequence.
///
/// Snapshot events follow the shared rules of [`expand_photos`] verbatim
/// (lone-CLEAR for an empty photo, run collapse, adds-only after an
/// established-empty book, `F_LAST` on the final snapshot row). A standalone
/// level-change event is a one-row, self-closing book event carrying `F_LAST` on
/// its own row; it never collapses with an adjacent CLEAR. After any standalone
/// the book is no longer "provably empty", so a following snapshot opens with its
/// own CLEAR exactly as it would after a populated photo.
fn expand_delta_events(
    provenance: &RowProvenance<'_>,
    ordering: OrderingAuthority,
    empty_book_policy: EmptyBookPolicy,
    events: &[DeltaEvent],
) -> Result<Vec<CanonicalOrderBookDeltaRow>> {
    match ordering {
        OrderingAuthority::EventTime | OrderingAuthority::CaptureTime => {}
    }
    let EmptyBookPolicy::LoneClearLast = empty_book_policy;

    let snapshot_flags = RecordFlag::F_SNAPSHOT as u8 | RecordFlag::F_MBP as u8;
    let last_flag = RecordFlag::F_LAST as u8;

    let mut rows: Vec<CanonicalOrderBookDeltaRow> = Vec::new();
    let mut previous_event_time = i64::MIN;
    let mut previous_was_lone_clear = false;

    for event in events {
        let event_time = event.event_time();
        ensure!(
            event_time >= previous_event_time,
            "instrument {:?}: event time {} precedes previous {}",
            provenance.identity.instrument_id,
            event_time,
            previous_event_time
        );
        previous_event_time = event_time;

        match event {
            DeltaEvent::Snapshot(photo) => {
                let is_empty = photo.bids.is_empty() && photo.asks.is_empty();
                if is_empty {
                    if previous_was_lone_clear {
                        continue;
                    }
                    rows.push(make_row(
                        provenance,
                        &RowPayload {
                            event_time: photo.event_time,
                            availability_time: photo.availability_time,
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
                if !book_established_empty {
                    rows.push(make_row(
                        provenance,
                        &RowPayload {
                            event_time: photo.event_time,
                            availability_time: photo.availability_time,
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
                                availability_time: photo.availability_time,
                                action: DeltaAction::Add,
                                side: side.as_str(),
                                price: &level.price,
                                size: &level.size,
                                flags: snapshot_flags,
                            },
                        ));
                    }
                }
                let last = rows.last_mut().expect("non-empty photo emitted rows");
                last.flags |= last_flag;
            }
            DeltaEvent::Standalone(delta) => {
                // A standalone delta is its own one-row book event: it carries no
                // snapshot flag, sets/removes a single absolute level, and closes
                // itself with F_LAST. It does not collapse with a prior CLEAR, so
                // the established-empty latch is cleared.
                previous_was_lone_clear = false;
                rows.push(make_row(
                    provenance,
                    &RowPayload {
                        event_time: delta.event_time,
                        availability_time: delta.availability_time,
                        action: delta.action,
                        side: delta.side.as_str(),
                        price: &delta.price,
                        size: &delta.size,
                        flags: last_flag,
                    },
                ));
            }
        }
    }

    ensure!(
        !rows.is_empty(),
        "instrument {:?} yielded no delta rows",
        provenance.identity.instrument_id
    );
    for (sequence, row) in rows.iter_mut().enumerate() {
        let sequence = sequence as u64;
        row.sequence = sequence;
        row.source_sequence = Some(sequence.to_string());
    }
    Ok(rows)
}

/// Parse one side's snapshot levels from an archive-shape JSON array string of
/// `[price, size]` string pairs (`"[[\"0.49\",\"10\"]]"`), enforcing the level
/// sign policy. An empty array string `"[]"` yields no levels (a genuine
/// empty-book side).
fn parse_event_stream_levels(
    raw: &str,
    field: &str,
    policy: DeltaPriceSignPolicy,
) -> Result<Vec<ParsedLevel>> {
    let trimmed = raw.trim();
    ensure!(!trimmed.is_empty(), "{field}: empty levels cell");
    let parsed: Value = serde_json::from_str(trimmed)
        .with_context(|| format!("{field}: invalid levels JSON {trimmed:?}"))?;
    let array = parsed
        .as_array()
        .with_context(|| format!("{field}: expected a JSON array, got {trimmed:?}"))?;
    let mut levels = Vec::with_capacity(array.len());
    for (index, pair) in array.iter().enumerate() {
        let pair = pair
            .as_array()
            .with_context(|| format!("{field}[{index}]: expected a [price, size] pair"))?;
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
        apply_level_sign_policy_str(field, index, policy, price, size)?;
        levels.push(ParsedLevel {
            price: price.to_string(),
            size: size.to_string(),
        });
    }
    Ok(levels)
}

/// Enforce the level sign policy for one event-stream snapshot level.
fn apply_level_sign_policy_str(
    field: &str,
    index: usize,
    policy: DeltaPriceSignPolicy,
    price: &str,
    size: &str,
) -> Result<()> {
    match policy {
        DeltaPriceSignPolicy::StrictlyPositive => {
            for (label, value) in [("price", price), ("size", size)] {
                let parsed: Decimal = value
                    .parse()
                    .with_context(|| format!("{field}[{index}]: invalid {label} {value:?}"))?;
                ensure!(
                    parsed > Decimal::ZERO,
                    "{field}[{index}]: non-positive {label} {value:?}"
                );
            }
            Ok(())
        }
    }
}

/// Decode a `price`/`size` decimal-string pair, enforcing a positive price and a
/// non-negative size. Returns the trimmed strings and the parsed size so callers
/// can detect a removal (size == 0).
fn parse_price_size(price: &str, size: &str, ctx: &str) -> Result<(String, String, Decimal)> {
    let price = price.trim();
    let size = size.trim();
    ensure!(!price.is_empty(), "{ctx}: empty price");
    ensure!(!size.is_empty(), "{ctx}: empty size");
    let price_dec: Decimal = price
        .parse()
        .with_context(|| format!("{ctx}: invalid price {price:?}"))?;
    let size_dec: Decimal = size
        .parse()
        .with_context(|| format!("{ctx}: invalid size {size:?}"))?;
    ensure!(
        price_dec > Decimal::ZERO,
        "{ctx}: non-positive price {price:?}"
    );
    ensure!(size_dec >= Decimal::ZERO, "{ctx}: negative size {size:?}");
    Ok((price.to_string(), size.to_string(), size_dec))
}

/// Resolve a level-change/book side token through the configured side values.
fn parse_event_stream_side(fields: &EventStreamFields<'_>, raw: &str) -> Result<DeltaSide> {
    let raw = raw.trim();
    if fields
        .buy_side_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw))
    {
        return Ok(DeltaSide::Buy);
    }
    if fields
        .sell_side_values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(raw))
    {
        return Ok(DeltaSide::Sell);
    }
    bail!("unknown event-stream side token: {raw:?}")
}

/// Resolve a trade aggressor side through the configured side values.
fn parse_trade_aggressor(fields: &EventStreamFields<'_>, raw: &str) -> Result<TradeAggressorSide> {
    match parse_event_stream_side(fields, raw)? {
        DeltaSide::Buy => Ok(TradeAggressorSide::Buyer),
        DeltaSide::Sell => Ok(TradeAggressorSide::Seller),
    }
}

/// Resolve a trade id: the configured field verbatim when present and non-empty,
/// otherwise the synthetic per-instrument 0-based ordinal (`trade_ordinal`)
/// rendered as a decimal string.
fn resolve_trade_id(
    fields: &EventStreamFields<'_>,
    raw: &RawEventRow,
    trade_ordinal: usize,
) -> String {
    if fields.trade_id_field.is_some() {
        let configured = raw.trade_id.trim();
        if !configured.is_empty() {
            return configured.to_string();
        }
    }
    trade_ordinal.to_string()
}

/// Lowercase SHA-256 hex of the event-stream trade transform identity.
#[must_use]
fn event_stream_trade_transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// The container the photos were parsed from, used only to fail loud with the
/// right empty-input message and to decide whether per-group photos need a
/// stable event-time sort before expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotContainer {
    /// One decoded JSONL object: photos already arrive in a single time-ordered
    /// stream per instrument, so no re-sort is needed.
    SingleObject,
    /// A streaming gzip-tar of JSONL members: archive member order is not a
    /// per-instrument time order guarantee, so each group is stably sorted by
    /// event time before expansion.
    TarArchive,
}

impl SnapshotContainer {
    /// Human-readable noun for the empty-input failure.
    const fn empty_input_noun(self) -> &'static str {
        match self {
            Self::SingleObject => "snapshot object",
            Self::TarArchive => "snapshot tar archive",
        }
    }
}

/// The validated, borrowed snapshot field mapping shared by both container entry
/// points, resolved once before any line is parsed.
struct SnapshotFields<'a> {
    bids_field: &'a str,
    asks_field: &'a str,
    level_price_field: &'a str,
    level_size_field: &'a str,
    event_time_field: &'a str,
    event_time_unit: CsvTimestampUnit,
}

/// Per-instrument parsed-photo accumulator shared across one container's input.
///
/// Single-instrument objects use one group keyed by `None`; multi-instrument
/// objects key groups by the configured `key_field` value. `order` preserves
/// first-seen group order so produced tables are deterministic, and `groups`
/// owns the photos. For the tar path this accumulates across every member.
#[derive(Default)]
struct PhotoGroups {
    order: Vec<Option<String>>,
    groups: BTreeMap<Option<String>, Vec<ParsedPhoto>>,
}

/// Validate `ingest_run_id` and the snapshot field mapping, returning the
/// borrowed field set both entry points parse against.
fn validate_snapshot_mapping<'a>(
    mapping: &'a DeltaMappingConfig,
    ingest_run_id: &str,
) -> Result<SnapshotFields<'a>> {
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
    } = &mapping.format
    else {
        bail!(
            "snapshot delta path requires a Snapshot format mapping; the EventStream \
             format must use normalize_parquet_event_stream_deltas, not the snapshot \
             JSONL/tar entry points"
        );
    };
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
    Ok(SnapshotFields {
        bids_field,
        asks_field,
        level_price_field,
        level_size_field,
        event_time_field,
        event_time_unit: *event_time_unit,
    })
}

/// Parse one chunk of JSONL photos into the per-instrument accumulator.
///
/// One JSON photo per line; blank lines are skipped. Each line resolves its
/// instrument key (single-instrument shape keys by `None`), applies the
/// exclusion fence, parses its event time and both level arrays, and appends a
/// [`ParsedPhoto`] to its group. Called once for the JSONL path and once per
/// member for the tar path, so grouping spans the whole container.
fn parse_jsonl_into_groups(
    fields: &SnapshotFields<'_>,
    mapping: &DeltaMappingConfig,
    jsonl_text: &str,
    accumulator: &mut PhotoGroups,
) -> Result<()> {
    // Borrow the two fields separately so the `or_insert_with` closure pushes to
    // `order` while `groups.entry` holds its own disjoint mutable borrow.
    let PhotoGroups { order, groups } = accumulator;
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

        let event_time_raw = value.get(fields.event_time_field).with_context(|| {
            format!(
                "line {index}: missing event time field {:?}",
                fields.event_time_field
            )
        })?;
        let event_time = parse_event_time(fields.event_time_unit, event_time_raw)
            .with_context(|| format!("line {index}: invalid event time {event_time_raw}"))?;
        ensure!(event_time > 0, "line {index}: non-positive event time");

        let bids = parse_levels(
            index,
            &value,
            fields.bids_field,
            fields.level_price_field,
            fields.level_size_field,
            mapping.price_sign_policy,
        )?;
        let asks = parse_levels(
            index,
            &value,
            fields.asks_field,
            fields.level_price_field,
            fields.level_size_field,
            mapping.price_sign_policy,
        )?;

        let group = groups.entry(instrument_key.clone()).or_insert_with(|| {
            order.push(instrument_key.clone());
            Vec::new()
        });
        group.push(ParsedPhoto {
            event_time,
            availability_time: None,
            bids,
            asks,
        });
    }
    Ok(())
}

/// Expand every accumulated per-instrument group into a validated table.
///
/// Shared by both container entry points: the JSONL path passes its single
/// object's groups, the tar path passes groups accumulated across the whole
/// archive. For the tar container each group's photos are stably sorted by event
/// time first (archive member order is not a per-instrument time order), then the
/// identical [`expand_photos`] core and table assembly run for every group.
fn expand_groups_into_tables(
    accepted: &AcceptedDataset,
    identities: &DeltaInstrumentIdentities,
    mapping: &DeltaMappingConfig,
    mut accumulator: PhotoGroups,
    capture_time_nanos: i64,
    ingest_run_id: &str,
    container: SnapshotContainer,
) -> Result<Vec<CanonicalOrderBookDeltasTable>> {
    ensure!(
        !accumulator.order.is_empty(),
        "{} yielded no in-scope photos",
        container.empty_input_noun()
    );

    let canonical_instrument_key_prefix = format!("{}/{}", accepted.venue, accepted.product_family);
    let transform_hash = delta_transform_hash();

    let mut tables = Vec::with_capacity(accumulator.order.len());
    for instrument_key in &accumulator.order {
        let identity = identities.resolve(instrument_key.as_deref())?;
        let canonical_instrument_key = format!(
            "{canonical_instrument_key_prefix}/{}",
            identity.instrument_id
        );
        let mut photos = accumulator
            .groups
            .remove(instrument_key)
            .expect("group order entry has a populated group");

        if container == SnapshotContainer::TarArchive {
            // Stable sort by event time: members can interleave an instrument
            // across non-monotonic chunk boundaries, but the per-instrument
            // timeline must be monotonic for expansion. A stable sort preserves
            // the in-member order of exact ties so the lone-CLEAR/adds-only
            // collapse is deterministic.
            photos.sort_by_key(|photo| photo.event_time);
        }

        let provenance = RowProvenance {
            accepted,
            identity,
            canonical_instrument_key: &canonical_instrument_key,
            transform_hash: &transform_hash,
            capture_time_nanos,
            ingest_run_id,
        };
        // Snapshot families are a pure-photo timeline: wrap each photo as a
        // snapshot event and expand through the one shared core that the
        // event-stream path also uses, so the lone-CLEAR/adds-only collapse has a
        // single owner.
        let events: Vec<DeltaEvent> = photos.into_iter().map(DeltaEvent::Snapshot).collect();
        let rows = expand_delta_events(
            &provenance,
            mapping.ordering,
            mapping.empty_book_policy,
            &events,
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
    availability_time: Option<i64>,
    action: DeltaAction,
    side: &'a str,
    price: &'a str,
    size: &'a str,
    flags: u8,
}

/// Build one canonical delta row from the per-table [`RowProvenance`] and the
/// row's [`RowPayload`].
///
/// `sequence` / `source_sequence` are assigned by the caller after expansion and
/// collapse; this constructor leaves `sequence` at `0` and `source_sequence` at
/// `None`.
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
        availability_time: payload.availability_time,
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
        assert_eq!(table.rows[0].transform_hash, delta_transform_hash());
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

    /// Wrap a `[(name, jsonl)]` slice as a fallible tar-member iterator.
    fn members(members: &[(&str, &str)]) -> Vec<Result<TarMember>> {
        members
            .iter()
            .map(|(name, text)| {
                Ok(TarMember {
                    name: (*name).to_string(),
                    text: (*text).to_string(),
                })
            })
            .collect()
    }

    #[test]
    fn tar_path_accumulates_one_instrument_across_members() {
        let accepted = accepted_dataset();
        // Two members, each one photo, same single instrument: grouping must
        // span both members into one table with both photos.
        let member_a = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let member_b = "{\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";
        let tables = normalize_tar_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            members(&[("000.data", member_a), ("001.data", member_b)]).into_iter(),
            42,
            "ingest-run-test",
        )
        .expect("normalize across tar members");
        assert_eq!(tables.len(), 1);
        // Two photos, each CLEAR + bid ADD + ask ADD = 6 rows, sequenced 0..6.
        assert_eq!(tables[0].rows.len(), 6);
        assert_eq!(tables[0].rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(tables[0].rows[3].event_time, 1_700_000_060_000_000_000);
    }

    #[test]
    fn tar_path_splits_two_instruments_across_members() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("BASEONE")),
            ("BBB".to_string(), identity("BASETWO")),
        ]));
        // Member 1 carries instrument AAA, member 2 carries BBB: the split must
        // span members and produce one table per instrument.
        let member_a = "{\"coin\":\"AAA\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let member_b = "{\"coin\":\"BBB\",\"time\":1700000000000,\"bids\":[{\"px\":\"0.30\",\"sz\":\"5\"}],\"asks\":[{\"px\":\"0.33\",\"sz\":\"7\"}]}\n";
        let mut tables = normalize_tar_jsonl_snapshot_deltas(
            &accepted,
            &identities,
            &keyed_mapping(None),
            members(&[("000.data", member_a), ("001.data", member_b)]).into_iter(),
            42,
            "ingest-run-test",
        )
        .expect("normalize cross-member split");
        tables.sort_by(|left, right| {
            left.partition
                .instrument_id
                .cmp(&right.partition.instrument_id)
        });
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].partition.instrument_id, "BASEONE");
        assert_eq!(tables[1].partition.instrument_id, "BASETWO");
    }

    #[test]
    fn tar_path_sorts_photos_by_event_time_across_non_monotonic_members() {
        let accepted = accepted_dataset();
        // The later-timed photo arrives in the FIRST member and the earlier in
        // the SECOND: archive member order is not per-instrument time order.
        // The stable event-time sort must reorder them so expansion sees a
        // monotonic timeline rather than bailing on a regression.
        let later = "{\"time\":1700000060000,\"bids\":[{\"px\":\"0.50\",\"sz\":\"11\"}],\"asks\":[{\"px\":\"0.52\",\"sz\":\"13\"}]}\n";
        let earlier = "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n";
        let tables = normalize_tar_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            members(&[("000.data", later), ("001.data", earlier)]).into_iter(),
            42,
            "ingest-run-test",
        )
        .expect("normalize reorders non-monotonic members");
        let table = &tables[0];
        assert_eq!(table.rows.len(), 6);
        // After the sort the earlier photo's CLEAR leads the table.
        assert_eq!(table.rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(table.rows[3].event_time, 1_700_000_060_000_000_000);
    }

    #[test]
    fn tar_path_propagates_member_stream_error() {
        let accepted = accepted_dataset();
        let failing: Vec<Result<TarMember>> = vec![
            Ok(TarMember {
                name: "000.data".to_string(),
                text: "{\"time\":1700000000000,\"bids\":[{\"px\":\"0.49\",\"sz\":\"10\"}],\"asks\":[{\"px\":\"0.51\",\"sz\":\"12\"}]}\n".to_string(),
            }),
            Err(anyhow::anyhow!("synthetic tar stream failure")),
        ];
        let err = normalize_tar_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            failing.into_iter(),
            42,
            "ingest-run-test",
        )
        .expect_err("a tar stream failure must fail loud");
        assert!(err.to_string().contains("read next tar member"), "{err}");
    }

    #[test]
    fn tar_path_rejects_empty_archive() {
        let accepted = accepted_dataset();
        let empty: Vec<Result<TarMember>> = Vec::new();
        let err = normalize_tar_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            empty.into_iter(),
            42,
            "ingest-run-test",
        )
        .expect_err("an archive with no in-scope photos must fail loud");
        assert!(err.to_string().contains("snapshot tar archive"), "{err}");
    }

    // ---- Event-stream (Parquet typed-event dual-emit) tests ----

    use arrow::{
        array::ArrayRef,
        datatypes::{DataType, Field, Schema},
    };
    use parquet::arrow::ArrowWriter;
    use std::sync::Arc;

    use crate::canonical_market_data::DeltaAction as DA;
    use crate::canonical_trades::TradeAggressorSide;
    use crate::source_proof::SourceProofFidelityClass as Fid;

    /// One synthetic event row destined for an in-memory typed-event Parquet
    /// object. Every column is a string; null is expressed with `None`.
    #[derive(Clone, Default)]
    struct EventRowSpec {
        coin: Option<&'static str>,
        event_type: &'static str,
        capture_time: Option<&'static str>,
        exchange_time: Option<&'static str>,
        bids: Option<&'static str>,
        asks: Option<&'static str>,
        price: Option<&'static str>,
        size: Option<&'static str>,
        side: Option<&'static str>,
        trade_price: Option<&'static str>,
        trade_size: Option<&'static str>,
    }

    /// Build an in-memory typed-event Parquet object from the row specs. Columns
    /// are all Utf8 (nullable) and keyed by the names the event-stream mapping
    /// uses, so the decoder reads them through the configured names.
    fn build_event_parquet(rows: &[EventRowSpec]) -> Vec<u8> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("coin", DataType::Utf8, true),
            Field::new("event_type", DataType::Utf8, true),
            Field::new("capture_time", DataType::Utf8, true),
            Field::new("exchange_time", DataType::Utf8, true),
            Field::new("bids", DataType::Utf8, true),
            Field::new("asks", DataType::Utf8, true),
            Field::new("price", DataType::Utf8, true),
            Field::new("size", DataType::Utf8, true),
            Field::new("side", DataType::Utf8, true),
            Field::new("trade_price", DataType::Utf8, true),
            Field::new("trade_size", DataType::Utf8, true),
        ]));
        let column = |pick: fn(&EventRowSpec) -> Option<&'static str>| -> ArrayRef {
            Arc::new(StringArray::from(rows.iter().map(pick).collect::<Vec<_>>()))
        };
        let event_type_col: ArrayRef = Arc::new(StringArray::from(
            rows.iter().map(|r| Some(r.event_type)).collect::<Vec<_>>(),
        ));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                column(|r| r.coin),
                event_type_col,
                column(|r| r.capture_time),
                column(|r| r.exchange_time),
                column(|r| r.bids),
                column(|r| r.asks),
                column(|r| r.price),
                column(|r| r.size),
                column(|r| r.side),
                column(|r| r.trade_price),
                column(|r| r.trade_size),
            ],
        )
        .expect("synthetic event-stream record batch");
        let mut buffer: Vec<u8> = Vec::new();
        let mut writer =
            ArrowWriter::try_new(&mut buffer, schema, None).expect("event-stream parquet writer");
        writer.write(&batch).expect("write event-stream batch");
        writer.close().expect("finalize event-stream parquet");
        buffer
    }

    fn event_stream_mapping(key_field: Option<&str>) -> DeltaMappingConfig {
        DeltaMappingConfig {
            format: DeltaSourceFormat::EventStream {
                event_type_field: "event_type".to_string(),
                snapshot_event_value: "book".to_string(),
                level_change_event_value: "price_change".to_string(),
                trade_event_value: "last_trade".to_string(),
                dropped_event_values: vec!["tick_size_change".to_string()],
                side_field: "side".to_string(),
                buy_side_values: vec!["BUY".to_string()],
                sell_side_values: vec!["SELL".to_string()],
                price_field: "price".to_string(),
                size_field: "size".to_string(),
                bids_field: "bids".to_string(),
                asks_field: "asks".to_string(),
                capture_time_field: "capture_time".to_string(),
                capture_time_unit: CsvTimestampUnit::Milliseconds,
                tiebreak_is_row_index: true,
                trade_price_field: "trade_price".to_string(),
                trade_size_field: "trade_size".to_string(),
                trade_id_field: None,
                event_time_field: Some("exchange_time".to_string()),
                event_time_unit: Some(CsvTimestampUnit::Milliseconds),
                trade_forbidden_claims: vec![
                    "No order-book-imbalance claims from trade prints.".to_string(),
                ],
            },
            instrument_key: InstrumentKeySpec {
                key_field: key_field.map(str::to_string),
                exclusion_filter: None,
            },
            ordering: OrderingAuthority::CaptureTime,
            price_sign_policy: DeltaPriceSignPolicy::StrictlyPositive,
            empty_book_policy: EmptyBookPolicy::LoneClearLast,
        }
    }

    fn snapshot_row(
        capture: &'static str,
        exchange: Option<&'static str>,
        bids: &'static str,
        asks: &'static str,
    ) -> EventRowSpec {
        EventRowSpec {
            event_type: "book",
            capture_time: Some(capture),
            exchange_time: exchange,
            bids: Some(bids),
            asks: Some(asks),
            ..EventRowSpec::default()
        }
    }

    fn level_change_row(
        capture: &'static str,
        side: &'static str,
        price: &'static str,
        size: &'static str,
    ) -> EventRowSpec {
        EventRowSpec {
            event_type: "price_change",
            capture_time: Some(capture),
            side: Some(side),
            price: Some(price),
            size: Some(size),
            ..EventRowSpec::default()
        }
    }

    fn trade_row(
        capture: &'static str,
        side: &'static str,
        price: &'static str,
        size: &'static str,
    ) -> EventRowSpec {
        EventRowSpec {
            event_type: "last_trade",
            capture_time: Some(capture),
            side: Some(side),
            trade_price: Some(price),
            trade_size: Some(size),
            ..EventRowSpec::default()
        }
    }

    #[test]
    fn event_stream_dual_emits_deltas_and_trades() {
        let accepted = accepted_dataset();
        let rows = vec![
            // Snapshot, then a level change, then a trade, then a dropped event.
            snapshot_row(
                "1700000000000",
                Some("1699999999000"),
                "[[\"0.49\",\"10\"]]",
                "[[\"0.51\",\"12\"]]",
            ),
            level_change_row("1700000001000", "BUY", "0.48", "5"),
            trade_row("1700000002000", "BUY", "0.50", "3"),
            EventRowSpec {
                event_type: "tick_size_change",
                capture_time: Some("1700000003000"),
                ..EventRowSpec::default()
            },
        ];
        let parquet = build_event_parquet(&rows);
        let (deltas, trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("event-stream dual emission");

        assert_eq!(deltas.len(), 1, "single instrument => one deltas table");
        assert_eq!(trades.len(), 1, "trade present => one trades table");
        let delta_table = &deltas[0];
        // Snapshot: CLEAR + bid ADD + ask ADD (3) then a standalone UPDATE (1).
        assert_eq!(delta_table.rows.len(), 4);
        assert_eq!(delta_table.fidelity_class, Fid::L2Replay);
        delta_table.validate().expect("deltas validate");
        // The standalone level change is an UPDATE closing its own event.
        let standalone = &delta_table.rows[3];
        assert_eq!(standalone.action, DA::Update.as_str());
        assert_eq!(standalone.side, DeltaSide::Buy.as_str());
        assert_eq!(standalone.price, "0.48");
        assert_eq!(standalone.size, "5");
        assert_ne!(standalone.flags & RecordFlag::F_LAST as u8, 0);
        assert_eq!(standalone.flags & RecordFlag::F_SNAPSHOT as u8, 0);
        // event_time is the capture clock; availability_time is the exchange time.
        assert_eq!(delta_table.rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(
            delta_table.rows[0].availability_time,
            Some(1_699_999_999_000_000_000)
        );

        let trade_table = &trades[0];
        assert_eq!(trade_table.fidelity_class, Fid::TradeReplay);
        assert_ne!(trade_table.fidelity_class, Fid::L2Replay);
        assert_eq!(trade_table.rows.len(), 1);
        trade_table.validate().expect("trades validate");
        let trade = &trade_table.rows[0];
        assert_eq!(trade.aggressor_side, TradeAggressorSide::Buyer.as_str());
        assert_eq!(trade.price, "0.50");
        assert_eq!(trade.size, "3");
        assert_eq!(trade.trade_source_type, "native");
        // Synthetic per-instrument trade ordinal when no trade_id_field is set.
        assert_eq!(trade.trade_id, "0");
        assert_eq!(trade.event_time, 1_700_000_002_000_000_000);
        assert_eq!(
            trade_table.forbidden_claims,
            vec!["No order-book-imbalance claims from trade prints.".to_string()]
        );
    }

    #[test]
    fn event_stream_empty_book_is_lone_clear() {
        let accepted = accepted_dataset();
        let rows = vec![snapshot_row("1700000000000", None, "[]", "[]")];
        let parquet = build_event_parquet(&rows);
        let (deltas, trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("empty-book event stream");
        assert!(trades.is_empty(), "no trade events => no trades table");
        let table = &deltas[0];
        assert_eq!(table.rows.len(), 1);
        let clear = &table.rows[0];
        assert_eq!(clear.action, DA::Clear.as_str());
        assert_ne!(clear.flags & RecordFlag::F_SNAPSHOT as u8, 0);
        assert_ne!(clear.flags & RecordFlag::F_LAST as u8, 0);
        table.validate().expect("lone-clear table validates");
    }

    #[test]
    fn event_stream_level_change_zero_size_is_delete() {
        let accepted = accepted_dataset();
        // A snapshot first establishes a non-empty book, then a zero-size level
        // change removes a level (DELETE). DELETE rows carry size "0".
        let rows = vec![
            snapshot_row(
                "1700000000000",
                None,
                "[[\"0.49\",\"10\"]]",
                "[[\"0.51\",\"12\"]]",
            ),
            level_change_row("1700000001000", "SELL", "0.51", "0"),
        ];
        let parquet = build_event_parquet(&rows);
        let (deltas, _trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("delete event stream");
        let table = &deltas[0];
        let delete = table.rows.last().expect("a row");
        assert_eq!(delete.action, DA::Delete.as_str());
        assert_eq!(delete.side, DeltaSide::Sell.as_str());
        assert_eq!(delete.size, "0");
        assert_ne!(delete.flags & RecordFlag::F_LAST as u8, 0);
        table.validate().expect("delete table validates");
    }

    #[test]
    fn event_stream_orders_by_capture_time_with_stale_exchange_time() {
        let accepted = accepted_dataset();
        // The later-captured snapshot is physically FIRST and carries an EARLIER
        // exchange time; the earlier-captured snapshot is physically SECOND with a
        // LATER exchange time. Capture-time ordering must reorder them so the
        // replay timeline is monotonic on the capture clock, not the stale
        // exchange clock.
        let rows = vec![
            snapshot_row(
                "1700000060000",
                Some("1699999998000"),
                "[[\"0.50\",\"11\"]]",
                "[[\"0.52\",\"13\"]]",
            ),
            snapshot_row(
                "1700000000000",
                Some("1700000050000"),
                "[[\"0.49\",\"10\"]]",
                "[[\"0.51\",\"12\"]]",
            ),
        ];
        let parquet = build_event_parquet(&rows);
        let (deltas, _trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("capture-time reorder");
        let table = &deltas[0];
        // The earlier capture time (the physically-second row) leads the table.
        assert_eq!(table.rows[0].event_time, 1_700_000_000_000_000_000);
        assert_eq!(table.rows[3].event_time, 1_700_000_060_000_000_000);
        // ts is non-decreasing on the capture clock.
        let mut prev = i64::MIN;
        for row in &table.rows {
            assert!(row.event_time >= prev);
            prev = row.event_time;
        }
    }

    #[test]
    fn event_stream_splits_by_asset_key_and_fences_excluded() {
        let accepted = accepted_dataset();
        let identities = DeltaInstrumentIdentities::Keyed(BTreeMap::from([
            ("AAA".to_string(), identity("BASEONE")),
            ("BBB".to_string(), identity("BASETWO")),
        ]));
        let mut mapping = event_stream_mapping(Some("coin"));
        mapping.instrument_key.exclusion_filter = Some(InstrumentExclusionFilter {
            exclude_if_contains: vec![":".to_string()],
            exclude_if_prefix: vec!["@".to_string()],
        });
        let rows = vec![
            EventRowSpec {
                coin: Some("AAA"),
                ..snapshot_row(
                    "1700000000000",
                    None,
                    "[[\"0.49\",\"10\"]]",
                    "[[\"0.51\",\"12\"]]",
                )
            },
            EventRowSpec {
                coin: Some("BBB"),
                ..snapshot_row(
                    "1700000000000",
                    None,
                    "[[\"0.30\",\"5\"]]",
                    "[[\"0.33\",\"7\"]]",
                )
            },
            // Fenced out: contains ':' and starts with '@'.
            EventRowSpec {
                coin: Some("dex:AAA"),
                ..snapshot_row(
                    "1700000000000",
                    None,
                    "[[\"0.1\",\"1\"]]",
                    "[[\"0.2\",\"1\"]]",
                )
            },
            EventRowSpec {
                coin: Some("@7"),
                ..snapshot_row(
                    "1700000000000",
                    None,
                    "[[\"0.1\",\"1\"]]",
                    "[[\"0.2\",\"1\"]]",
                )
            },
        ];
        let parquet = build_event_parquet(&rows);
        let (mut deltas, _trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &identities,
            &mapping,
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("keyed split with fence");
        deltas.sort_by(|a, b| a.partition.instrument_id.cmp(&b.partition.instrument_id));
        assert_eq!(deltas.len(), 2, "only AAA and BBB survive the fence");
        assert_eq!(deltas[0].partition.instrument_id, "BASEONE");
        assert_eq!(deltas[1].partition.instrument_id, "BASETWO");
    }

    #[test]
    fn event_stream_sequence_is_dense_across_mixed_events() {
        let accepted = accepted_dataset();
        let rows = vec![
            snapshot_row(
                "1700000000000",
                None,
                "[[\"0.49\",\"10\"]]",
                "[[\"0.51\",\"12\"]]",
            ),
            level_change_row("1700000001000", "BUY", "0.48", "5"),
            level_change_row("1700000002000", "SELL", "0.52", "9"),
        ];
        let parquet = build_event_parquet(&rows);
        let (deltas, _trades) = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect("mixed events");
        let table = &deltas[0];
        // CLEAR + bid ADD + ask ADD + UPDATE + UPDATE = 5 dense rows.
        assert_eq!(table.rows.len(), 5);
        for (index, row) in table.rows.iter().enumerate() {
            assert_eq!(row.sequence, index as u64);
        }
        table.validate().expect("dense mixed table validates");
    }

    #[test]
    fn event_stream_rejects_unknown_event_value() {
        let accepted = accepted_dataset();
        let rows = vec![EventRowSpec {
            event_type: "mystery_event",
            capture_time: Some("1700000000000"),
            ..EventRowSpec::default()
        }];
        let parquet = build_event_parquet(&rows);
        let err = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect_err("unknown event value must fail loud");
        assert!(
            err.to_string().contains("unknown event-stream event_type"),
            "{err}"
        );
    }

    #[test]
    fn event_stream_rejects_empty_object() {
        let accepted = accepted_dataset();
        let rows: Vec<EventRowSpec> = Vec::new();
        let parquet = build_event_parquet(&rows);
        let err = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect_err("an object with no in-scope events must fail loud");
        assert!(err.to_string().contains("no in-scope events"), "{err}");
    }

    #[test]
    fn event_stream_rejects_empty_trade_forbidden_claims() {
        let accepted = accepted_dataset();
        let mut mapping = event_stream_mapping(None);
        if let DeltaSourceFormat::EventStream {
            trade_forbidden_claims,
            ..
        } = &mut mapping.format
        {
            trade_forbidden_claims.clear();
        }
        let rows = vec![snapshot_row(
            "1700000000000",
            None,
            "[[\"0.49\",\"10\"]]",
            "[[\"0.51\",\"12\"]]",
        )];
        let parquet = build_event_parquet(&rows);
        let err = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &mapping,
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect_err("empty trade_forbidden_claims must fail loud");
        assert!(err.to_string().contains("trade_forbidden_claims"), "{err}");
    }

    #[test]
    fn event_stream_rejects_false_tiebreak_flag() {
        let accepted = accepted_dataset();
        let mut mapping = event_stream_mapping(None);
        if let DeltaSourceFormat::EventStream {
            tiebreak_is_row_index,
            ..
        } = &mut mapping.format
        {
            *tiebreak_is_row_index = false;
        }
        let rows = vec![snapshot_row(
            "1700000000000",
            None,
            "[[\"0.49\",\"10\"]]",
            "[[\"0.51\",\"12\"]]",
        )];
        let parquet = build_event_parquet(&rows);
        let err = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &mapping,
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect_err("tiebreak_is_row_index=false must fail loud");
        assert!(err.to_string().contains("tiebreak_is_row_index"), "{err}");
    }

    #[test]
    fn snapshot_path_rejects_event_stream_mapping() {
        let accepted = accepted_dataset();
        let err = normalize_jsonl_snapshot_deltas(
            &accepted,
            &single_identity(),
            &event_stream_mapping(None),
            SINGLE_JSONL,
            42,
            "ingest-run-test",
        )
        .expect_err("snapshot path must reject an EventStream mapping");
        assert!(
            err.to_string().contains("requires a Snapshot format"),
            "{err}"
        );
    }

    #[test]
    fn event_stream_path_rejects_snapshot_mapping() {
        let accepted = accepted_dataset();
        let parquet = build_event_parquet(&[snapshot_row(
            "1700000000000",
            None,
            "[[\"0.49\",\"10\"]]",
            "[[\"0.51\",\"12\"]]",
        )]);
        let err = normalize_parquet_event_stream_deltas(
            &accepted,
            &single_identity(),
            &single_mapping(),
            &parquet,
            42,
            "ingest-run-test",
        )
        .expect_err("event-stream path must reject a Snapshot mapping");
        assert!(
            err.to_string().contains("requires an EventStream format"),
            "{err}"
        );
    }
}
