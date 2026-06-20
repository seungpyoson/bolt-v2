//! Gate 2 — canonical normalized `trades` table.
//!
//! Normalizes an accepted raw tick-trades object into the `trades` table family
//! of the `backfill-table-contract.v1` contract
//! (`specs/023-nt-research-analytics-platform/reference/backfill-table-contract.md`).
//!
//! The normalized table carries the common identity/provenance columns plus the
//! native-trade fields, preserves the exact source price/size strings, and is
//! written as a canonical Parquet artifact. It is the single bridge from raw
//! evidence to the NautilusTrader catalog projection in
//! [`super::catalog_projection`].
//!
//! Input is only ever an [`AcceptedDataset`] from gate 1 — raw staged data never
//! reaches this module without first passing source-proof acceptance.

use std::{fs::File, path::Path, sync::Arc};

use anyhow::{Context, Result, bail, ensure};
use arrow::{
    array::{Array, Int64Array, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::source_proof::{AcceptedDataset, SourceProofFidelityClass};

/// Contracted semantic schema version for normalized market-data rows.
pub const NORMALIZED_SCHEMA_VERSION: &str = "market_data.v1";

/// Stable identity of the generic CSV native-trades normalization transform.
pub const TRANSFORM_IDENTITY: &str = "csv-native-trades-to-canonical-trades.v1";

/// Version of the registered compiled converter implementation.
pub const TRANSFORM_VERSION: &str = "1";

/// Source-proof table family accepted by the native trade converter.
pub const TRADE_TABLE_FAMILY: &str = "trades";

/// Native trade prints only; aggregated prints must never satisfy this table.
pub const TRADE_SOURCE_TYPE_NATIVE: &str = "native";

/// Stable identity of the config-driven CSV native-bars normalization transform.
pub const BAR_TRANSFORM_IDENTITY: &str = "csv-native-bars-to-canonical-bars.v1";

/// Version of the registered compiled bar-converter implementation.
pub const BAR_TRANSFORM_VERSION: &str = "1";

/// Source-proof table family accepted by the native bar converter.
pub const BAR_TABLE_FAMILY: &str = "bars";

/// Stable identity of the config-driven paged-JSON kline normalization transform.
///
/// Distinct from [`BAR_TRANSFORM_IDENTITY`] because the wire shape differs (OHLCV
/// rows nested in a paged REST JSON envelope vs flat CSV); the table family,
/// normalized schema, and NT data type match the CSV bar adapter because both
/// produce the same `bars` rows.
pub const PAGED_JSON_BARS_TRANSFORM_IDENTITY: &str = "paged-json-bars-to-canonical-bars.v1";

/// Version of the registered compiled paged-JSON bar converter implementation.
pub const PAGED_JSON_BARS_TRANSFORM_VERSION: &str = "1";

/// Stable identity of the config-driven line-delimited multi-interval kline
/// normalization transform.
///
/// Distinct from the other bar identities because each line carries its own
/// interval token and one object emits one stream per `(instrument, interval)`
/// group; the table family, normalized schema, and NT data type still match the
/// other bar adapters because every group produces the same `bars` rows.
pub const JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY: &str =
    "jsonl-multi-interval-bars-to-canonical-bars.v1";

/// Version of the registered compiled JSONL multi-interval bar converter
/// implementation.
pub const JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION: &str = "1";

/// Stable identity of the config-driven JSONL periodic-full-snapshot
/// order-book-delta normalization transform.
pub const DELTAS_TRANSFORM_IDENTITY: &str =
    "jsonl-snapshot-deltas-to-canonical-order-book-deltas.v1";

/// Version of the registered compiled order-book-delta converter implementation.
pub const DELTAS_TRANSFORM_VERSION: &str = "1";

/// Stable identity of the streaming tar-of-JSONL periodic-full-snapshot
/// order-book-delta normalization transform.
///
/// Distinct from [`DELTAS_TRANSFORM_IDENTITY`] because the container differs (a
/// streaming gzip-tar of JSONL members vs a single decoded JSONL object); the
/// table family, normalized schema, and NT data type match the JSONL deltas
/// adapter because both produce the same `order_book_snapshot_deltas` rows.
pub const TAR_DELTAS_TRANSFORM_IDENTITY: &str =
    "tar-jsonl-snapshot-deltas-to-canonical-order-book-deltas.v1";

/// Version of the registered compiled tar-of-JSONL order-book-delta converter
/// implementation.
pub const TAR_DELTAS_TRANSFORM_VERSION: &str = "1";

/// Stable identity of the typed-event Parquet stream normalization transform that
/// dual-emits the order-book-delta AND trades families from one L2 archive
/// object.
///
/// Distinct from the snapshot delta identities because the wire shape differs
/// (typed interleaved events in a Parquet stream vs full photos), and because
/// this transform additionally produces the trades family under the
/// dual-fidelity rule documented in
/// [`super::canonical_order_book_deltas`]. The delta rows it produces share the
/// `order_book_snapshot_deltas` table family and the `OrderBookDelta` NT type.
pub const EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY: &str =
    "parquet-event-stream-to-canonical-order-book-deltas-and-trades.v1";

/// Version of the registered compiled event-stream dual-emit converter
/// implementation.
pub const EVENT_STREAM_DELTAS_TRANSFORM_VERSION: &str = "1";

/// Source-proof table family accepted by the JSONL snapshot-delta converter.
pub const DELTAS_TABLE_FAMILY: &str = "order_book_snapshot_deltas";

/// NT catalog path-prefix and source-proof table family for top-of-book quotes.
///
/// Matches NautilusTrader's own `QuoteTick` catalog prefix
/// (`impl_catalog_path_prefix!(QuoteTick, "quotes")`), so the canonical family
/// name and the NT catalog directory agree.
pub const QUOTE_TABLE_FAMILY: &str = "quotes";

/// Stable identity of the config-driven top-of-book snapshot quote normalization
/// transform.
///
/// Distinct from the delta identities because the wire shape and meaning differ:
/// a single best-bid/ask snapshot (a `QuoteTick`) versus full-depth L2 photos /
/// typed events (the `order_book_snapshot_deltas` family). Both bind their own
/// fidelity class — quotes are `QUOTE_REPLAY`, deltas are `L2_REPLAY`.
pub const SNAPSHOT_QUOTES_TRANSFORM_IDENTITY: &str = "jsonl-top-of-book-to-canonical-quotes.v1";

/// Version of the registered compiled snapshot-quote converter implementation.
pub const SNAPSHOT_QUOTES_TRANSFORM_VERSION: &str = "1";

/// NT catalog path-prefix and source-proof table family for index-price updates.
///
/// Matches NautilusTrader's own `IndexPriceUpdate` catalog prefix
/// (`impl_catalog_path_prefix!(IndexPriceUpdate, "index_prices")`), so the
/// canonical family name and the NT catalog directory agree.
pub const INDEX_PRICES_TABLE_FAMILY: &str = "index_prices";

/// Stable identity of the config-driven index-price normalization transform.
///
/// Source-agnostic: a new index/oracle source that emits the same point-update
/// shape binds this identity. The raw wire normalizer that fills a
/// `CanonicalIndexPricesTable` from raw bytes is OUT OF SCOPE for this slice
/// (data acquisition is tracked in bolt-v2 #836/#437); only the canonical->NT
/// projection path is delivered here. The operator dispatch fails loud naming
/// that follow-up.
pub const INDEX_PRICES_TRANSFORM_IDENTITY: &str = "index-price-source-to-canonical-index-prices.v1";

/// Version of the registered index-price converter contract.
pub const INDEX_PRICES_TRANSFORM_VERSION: &str = "1";

/// NT catalog path-prefix and source-proof table family for mark-price updates.
///
/// Matches NautilusTrader's own `MarkPriceUpdate` catalog prefix
/// (`impl_catalog_path_prefix!(MarkPriceUpdate, "mark_prices")`), so the
/// canonical family name and the NT catalog directory agree.
pub const MARK_PRICES_TABLE_FAMILY: &str = "mark_prices";

/// Stable identity of the config-driven mark-price normalization transform.
///
/// Source-agnostic: a new mark/reference source that emits the same point-update
/// shape binds this identity. The raw wire normalizer that fills a
/// `CanonicalMarkPricesTable` from raw bytes is OUT OF SCOPE for this slice
/// (data acquisition is tracked in bolt-v2 #836/#437); only the canonical->NT
/// projection path is delivered here. The operator dispatch fails loud naming
/// that follow-up.
pub const MARK_PRICES_TRANSFORM_IDENTITY: &str = "mark-price-source-to-canonical-mark-prices.v1";

/// Version of the registered mark-price converter contract.
pub const MARK_PRICES_TRANSFORM_VERSION: &str = "1";

/// Contract table family for funding-rate updates.
///
/// The canonical family uses the contract name `funding_rates`; the NT catalog
/// projection writes NautilusTrader's `funding_rate_update` prefix.
pub const FUNDING_RATES_TABLE_FAMILY: &str = "funding_rates";

/// Stable identity of the config-driven funding-rate normalization transform.
///
/// Source-agnostic: a new funding source that emits the same point-update shape
/// binds this identity. Raw acquisition remains a follow-up; this registration
/// keeps the operator path wired to the canonical table and NT projection.
pub const FUNDING_RATES_TRANSFORM_IDENTITY: &str =
    "funding-rate-source-to-canonical-funding-rates.v1";

/// Version of the registered funding-rate converter contract.
pub const FUNDING_RATES_TRANSFORM_VERSION: &str = "1";

const NANOS_PER_SECOND: i64 = 1_000_000_000;

/// Expected sample raw header, in order.
#[cfg(test)]
pub const SAMPLE_SPOT_TICK_TRADES_HEADER: [&str; 6] =
    ["id", "timestamp", "price", "volume", "side", "rpi"];

#[cfg(test)]
const NANOS_PER_MILLISECOND: i64 = 1_000_000;

/// Registered source adapter implementation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceAdapterKind {
    CsvNativeTrades,
    CsvNativeBars,
    PagedJsonBars,
    JsonlMultiIntervalBars,
    JsonlSnapshotDeltas,
    TarJsonlSnapshotDeltas,
    ParquetEventStreamDeltas,
    SnapshotQuotes,
    SeededL2Quotes,
    IndexPrices,
    MarkPrices,
    FundingRates,
    #[cfg(test)]
    SyntheticOrderBookDeltas,
}

/// Registered raw-source adapter.
///
/// A new venue that can emit the same CSV native-trades shape selects this
/// converter from TOML and supplies its column/side mapping in `[converter.csv]`.
/// Rust registration is only for a genuinely new raw format or NT data family,
/// leaving operator, runner, result contract, and NT catalog/backtest wiring
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceAdapterDefinition {
    pub identity: &'static str,
    pub version: &'static str,
    pub kind: SourceAdapterKind,
    pub table_family: &'static str,
    pub normalized_schema_version: &'static str,
    pub nt_data_type: &'static str,
}

/// Backwards-compatible name for the currently implemented trade adapter path.
pub type TradeConverterDefinition = SourceAdapterDefinition;

/// Run-spec owned converter config.
///
/// `csv` carries the native-trades column mapping (present in every existing
/// trade run-spec). `bars` is the optional native-bars column mapping, present
/// only when the registered adapter kind is [`SourceAdapterKind::CsvNativeBars`];
/// the bar dispatch in [`normalize_registered_bar_converter`] fail-closes when a
/// bar-kind config omits it. `deltas` is the optional JSONL snapshot-delta field
/// mapping, present only when the registered adapter kind is
/// [`SourceAdapterKind::JsonlSnapshotDeltas`]; the delta dispatch in
/// [`normalize_registered_order_book_delta_converter`] fail-closes when a
/// delta-kind config omits it. Existing trade run-specs carry no `bars`/`deltas`
/// key and deserialize unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConverterConfig {
    pub identity: String,
    pub version: String,
    pub raw_payload: RawPayloadConfig,
    pub csv: CsvTradeMappingConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<super::canonical_bars::BarMappingConfig>,
    /// Paged-JSON kline mapping, present only when the registered adapter kind is
    /// [`SourceAdapterKind::PagedJsonBars`]; the paged-JSON bar dispatch in
    /// [`normalize_registered_paged_json_bar_converter`] fail-closes when a
    /// paged-JSON-bar-kind config omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paged_json_bars: Option<super::canonical_bars::PagedJsonBarMappingConfig>,
    /// Line-delimited multi-interval kline mapping, present only when the
    /// registered adapter kind is [`SourceAdapterKind::JsonlMultiIntervalBars`];
    /// the dispatch in [`normalize_registered_jsonl_multi_interval_bar_converter`]
    /// fail-closes when a JSONL-bar-kind config omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsonl_bars: Option<super::canonical_bars::JsonlBarMappingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deltas: Option<super::canonical_order_book_deltas::DeltaMappingConfig>,
    /// Top-of-book snapshot-quote field mapping, present only when the registered
    /// adapter kind is [`SourceAdapterKind::SnapshotQuotes`]; the snapshot-quotes
    /// operator dispatch fail-closes when a quote-kind config omits it. Existing
    /// run-specs carry no `quotes` key and deserialize unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quotes: Option<super::canonical_market_data::QuoteMappingConfig>,
    /// Snapshot-seeded L2 quote mapping, present only when the registered
    /// adapter kind is [`SourceAdapterKind::SeededL2Quotes`]. The adapter reads
    /// L2 snapshot+delta rows, seeds a book from the first snapshot, then emits
    /// top-of-book `QuoteTick` rows from absolute level-replace updates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeded_l2_quotes: Option<super::seeded_l2_quotes::SeededL2QuoteMappingConfig>,
}

impl ConverterConfig {
    pub fn content_hash(&self) -> Result<String> {
        let bytes = serde_json::to_vec(self).context("serialize converter config for hash")?;
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Ok(hex::encode(hasher.finalize()))
    }
}

/// Raw accepted-object container decoded before CSV normalization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPayloadConfig {
    pub container: RawPayloadContainer,
    /// Maximum accepted-object byte length allowed before local read/hash/decode.
    pub max_object_bytes: u64,
    /// Maximum decoded text byte length allowed after single-text container
    /// decoding (CSV/JSONL). Per-member tar bounds use [`Self::max_member_bytes`]
    /// instead; the Parquet passthrough is bounded only by
    /// [`Self::max_object_bytes`].
    pub max_decoded_bytes: u64,
    /// Required for [`RawPayloadContainer::SingleCsvZip`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zip_member: Option<String>,
    /// Per-member decoded byte bound for [`RawPayloadContainer::TarGzipJsonl`].
    /// Mirrors the byte-stability mechanism of `zip_member`: optional, omitted
    /// from serialization when absent, so existing single-text run-specs stay
    /// byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_member_bytes: Option<u64>,
    /// Required member-name suffix for [`RawPayloadContainer::TarGzipJsonl`]; only
    /// members whose names end with this suffix are streamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_suffix: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawPayloadContainer {
    CsvGzip,
    CsvText,
    SingleCsvZip,
    /// ZIP archive with exactly one UTF-8 line-delimited JSON member; decoded to
    /// one bounded text string after ZIP CRC/length verification.
    SingleJsonlZip,
    /// Plain UTF-8 line-delimited JSON (also single-envelope or pretty-printed
    /// paged JSON), decoded to one text string bounded by `max_decoded_bytes`.
    JsonlText,
    /// Gzip-compressed `JsonlText`; decompressed and decoded to one text string
    /// bounded by `max_decoded_bytes`.
    JsonlGzip,
    /// Gzip-compressed POSIX tar of JSONL members; members whose names end with
    /// `member_suffix` are streamed, each bounded by `max_member_bytes`.
    TarGzipJsonl,
    /// Raw Parquet object bytes passed through after the `max_object_bytes` object
    /// cap; read columnar downstream by the dual-emit event-stream adapter.
    ParquetFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CsvTradeMappingConfig {
    pub has_headers: bool,
    pub trade_id_column: String,
    pub timestamp_column: String,
    pub timestamp_unit: CsvTimestampUnit,
    pub price_column: String,
    pub size_column: String,
    pub side_column: String,
    pub buyer_side_values: Vec<String>,
    pub seller_side_values: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvTimestampUnit {
    Seconds,
    DecimalSeconds,
    Milliseconds,
    Microseconds,
    Nanoseconds,
}

pub const CSV_NATIVE_TRADES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: TRANSFORM_IDENTITY,
    version: TRANSFORM_VERSION,
    kind: SourceAdapterKind::CsvNativeTrades,
    table_family: TRADE_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: "TradeTick",
};

pub const CSV_NATIVE_BARS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: BAR_TRANSFORM_IDENTITY,
    version: BAR_TRANSFORM_VERSION,
    kind: SourceAdapterKind::CsvNativeBars,
    table_family: BAR_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_BAR,
};

pub const PAGED_JSON_BARS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: PAGED_JSON_BARS_TRANSFORM_IDENTITY,
    version: PAGED_JSON_BARS_TRANSFORM_VERSION,
    kind: SourceAdapterKind::PagedJsonBars,
    table_family: BAR_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_BAR,
};

pub const JSONL_MULTI_INTERVAL_BARS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
    version: JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION,
    kind: SourceAdapterKind::JsonlMultiIntervalBars,
    table_family: BAR_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_BAR,
};

pub const JSONL_SNAPSHOT_DELTAS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: DELTAS_TRANSFORM_IDENTITY,
    version: DELTAS_TRANSFORM_VERSION,
    kind: SourceAdapterKind::JsonlSnapshotDeltas,
    table_family: DELTAS_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_ORDER_BOOK_DELTA,
};

pub const TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: TAR_DELTAS_TRANSFORM_IDENTITY,
    version: TAR_DELTAS_TRANSFORM_VERSION,
    kind: SourceAdapterKind::TarJsonlSnapshotDeltas,
    table_family: DELTAS_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_ORDER_BOOK_DELTA,
};

/// The dual-emit event-stream adapter. Its `table_family` /
/// `normalized_schema_version` / `nt_data_type` describe the order-book-delta
/// family it primarily registers under (the same family the snapshot delta
/// adapters use); the trades family it additionally emits binds its own
/// fidelity/claims at normalization under the dual-fidelity rule, not through a
/// second registry row.
pub const PARQUET_EVENT_STREAM_DELTAS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
    version: EVENT_STREAM_DELTAS_TRANSFORM_VERSION,
    kind: SourceAdapterKind::ParquetEventStreamDeltas,
    table_family: DELTAS_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_ORDER_BOOK_DELTA,
};

pub const SNAPSHOT_QUOTES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: SNAPSHOT_QUOTES_TRANSFORM_IDENTITY,
    version: SNAPSHOT_QUOTES_TRANSFORM_VERSION,
    kind: SourceAdapterKind::SnapshotQuotes,
    table_family: QUOTE_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_QUOTE_TICK,
};

pub const SEEDED_L2_QUOTES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: super::seeded_l2_quotes::SEEDED_L2_QUOTES_TRANSFORM_IDENTITY,
    version: super::seeded_l2_quotes::SEEDED_L2_QUOTES_TRANSFORM_VERSION,
    kind: SourceAdapterKind::SeededL2Quotes,
    table_family: QUOTE_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_QUOTE_TICK,
};

pub const INDEX_PRICES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: INDEX_PRICES_TRANSFORM_IDENTITY,
    version: INDEX_PRICES_TRANSFORM_VERSION,
    kind: SourceAdapterKind::IndexPrices,
    table_family: INDEX_PRICES_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_INDEX_PRICE_UPDATE,
};

pub const MARK_PRICES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: MARK_PRICES_TRANSFORM_IDENTITY,
    version: MARK_PRICES_TRANSFORM_VERSION,
    kind: SourceAdapterKind::MarkPrices,
    table_family: MARK_PRICES_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_MARK_PRICE_UPDATE,
};

pub const FUNDING_RATES_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: FUNDING_RATES_TRANSFORM_IDENTITY,
    version: FUNDING_RATES_TRANSFORM_VERSION,
    kind: SourceAdapterKind::FundingRates,
    table_family: FUNDING_RATES_TABLE_FAMILY,
    normalized_schema_version: NORMALIZED_SCHEMA_VERSION,
    nt_data_type: crate::catalog_projection::NT_DATA_TYPE_FUNDING_RATE_UPDATE,
};

#[cfg(test)]
pub const SYNTHETIC_ORDER_BOOK_DELTAS_ADAPTER: SourceAdapterDefinition = SourceAdapterDefinition {
    identity: "synthetic-order-book-deltas-fixture.v1",
    version: "1",
    kind: SourceAdapterKind::SyntheticOrderBookDeltas,
    table_family: "order_book_snapshot_deltas",
    normalized_schema_version: "market_data.v1",
    nt_data_type: "OrderBookDelta",
};

#[cfg(not(test))]
pub const REGISTERED_SOURCE_ADAPTERS: &[SourceAdapterDefinition] = &[
    CSV_NATIVE_TRADES_ADAPTER,
    CSV_NATIVE_BARS_ADAPTER,
    PAGED_JSON_BARS_ADAPTER,
    JSONL_MULTI_INTERVAL_BARS_ADAPTER,
    JSONL_SNAPSHOT_DELTAS_ADAPTER,
    TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
    PARQUET_EVENT_STREAM_DELTAS_ADAPTER,
    SNAPSHOT_QUOTES_ADAPTER,
    SEEDED_L2_QUOTES_ADAPTER,
    INDEX_PRICES_ADAPTER,
    MARK_PRICES_ADAPTER,
    FUNDING_RATES_ADAPTER,
];

#[cfg(test)]
pub const REGISTERED_SOURCE_ADAPTERS: &[SourceAdapterDefinition] = &[
    CSV_NATIVE_TRADES_ADAPTER,
    CSV_NATIVE_BARS_ADAPTER,
    PAGED_JSON_BARS_ADAPTER,
    JSONL_MULTI_INTERVAL_BARS_ADAPTER,
    JSONL_SNAPSHOT_DELTAS_ADAPTER,
    TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER,
    PARQUET_EVENT_STREAM_DELTAS_ADAPTER,
    SNAPSHOT_QUOTES_ADAPTER,
    SEEDED_L2_QUOTES_ADAPTER,
    INDEX_PRICES_ADAPTER,
    MARK_PRICES_ADAPTER,
    FUNDING_RATES_ADAPTER,
    SYNTHETIC_ORDER_BOOK_DELTAS_ADAPTER,
];

pub const CSV_NATIVE_TRADES_CONVERTER: TradeConverterDefinition = CSV_NATIVE_TRADES_ADAPTER;

pub const REGISTERED_TRADE_CONVERTERS: &[TradeConverterDefinition] = &[CSV_NATIVE_TRADES_ADAPTER];

#[must_use]
pub fn registered_source_adapter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    REGISTERED_SOURCE_ADAPTERS
        .iter()
        .find(|adapter| adapter.identity == identity && adapter.version == version)
}

pub fn require_registered_source_adapter(
    identity: &str,
    version: &str,
) -> Result<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version).with_context(|| {
        format!("adapter {identity:?} version {version:?} is not a registered source adapter")
    })
}

pub fn require_registered_source_adapter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter = require_registered_source_adapter(identity, version)?;
    ensure!(
        adapter.table_family == table_family,
        "adapter {:?} version {:?} supports table_family {:?}, got {:?}",
        adapter.identity,
        adapter.version,
        adapter.table_family,
        table_family
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_trade_converter(
    identity: &str,
    version: &str,
) -> Option<&'static TradeConverterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::CsvNativeTrades)
}

pub fn require_registered_trade_converter(
    identity: &str,
    version: &str,
) -> Result<&'static TradeConverterDefinition> {
    let adapter = require_registered_source_adapter(identity, version)?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeTrades,
        "adapter {:?} version {:?} is {:?}, not a CSV native-trades converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

pub fn require_registered_trade_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static TradeConverterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeTrades,
        "adapter {:?} version {:?} is {:?}, not a CSV native-trades converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_bar_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::CsvNativeBars)
}

pub fn require_registered_bar_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::CsvNativeBars,
        "adapter {:?} version {:?} is {:?}, not a CSV native-bars converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_paged_json_bar_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::PagedJsonBars)
}

pub fn require_registered_paged_json_bar_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::PagedJsonBars,
        "adapter {:?} version {:?} is {:?}, not a paged-JSON bar converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_jsonl_multi_interval_bar_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::JsonlMultiIntervalBars)
}

pub fn require_registered_jsonl_multi_interval_bar_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::JsonlMultiIntervalBars,
        "adapter {:?} version {:?} is {:?}, not a JSONL multi-interval bar converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_order_book_delta_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::JsonlSnapshotDeltas)
}

pub fn require_registered_order_book_delta_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::JsonlSnapshotDeltas,
        "adapter {:?} version {:?} is {:?}, not a JSONL snapshot-delta converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_tar_order_book_delta_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::TarJsonlSnapshotDeltas)
}

pub fn require_registered_tar_order_book_delta_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::TarJsonlSnapshotDeltas,
        "adapter {:?} version {:?} is {:?}, not a tar JSONL snapshot-delta converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_event_stream_delta_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::ParquetEventStreamDeltas)
}

pub fn require_registered_event_stream_delta_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::ParquetEventStreamDeltas,
        "adapter {:?} version {:?} is {:?}, not a Parquet event-stream delta converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_quote_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::SnapshotQuotes)
}

pub fn require_registered_quote_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::SnapshotQuotes,
        "adapter {:?} version {:?} is {:?}, not a snapshot-quotes converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

#[must_use]
pub fn registered_seeded_l2_quote_converter(
    identity: &str,
    version: &str,
) -> Option<&'static SourceAdapterDefinition> {
    registered_source_adapter(identity, version)
        .filter(|adapter| adapter.kind == SourceAdapterKind::SeededL2Quotes)
}

pub fn require_registered_seeded_l2_quote_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::SeededL2Quotes,
        "adapter {:?} version {:?} is {:?}, not a seeded L2 quote converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

/// Resolve the registered index-price source adapter for the table family.
///
/// # Errors
///
/// Returns an error if no adapter is registered for the identity/version, the
/// adapter does not serve the table family, or the adapter is not an
/// index-price converter.
pub fn require_registered_index_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::IndexPrices,
        "adapter {:?} version {:?} is {:?}, not an index-price converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

/// Resolve the registered mark-price source adapter for the table family.
///
/// # Errors
///
/// Returns an error if no adapter is registered for the identity/version, the
/// adapter does not serve the table family, or the adapter is not a mark-price
/// converter.
pub fn require_registered_mark_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::MarkPrices,
        "adapter {:?} version {:?} is {:?}, not a mark-price converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

/// Resolve the registered funding-rate source adapter for the table family.
///
/// # Errors
///
/// Returns an error if no adapter is registered for the identity/version, the
/// adapter does not serve the table family, or the adapter is not a funding-rate
/// converter.
pub fn require_registered_funding_converter_for_table_family(
    identity: &str,
    version: &str,
    table_family: &str,
) -> Result<&'static SourceAdapterDefinition> {
    let adapter =
        require_registered_source_adapter_for_table_family(identity, version, table_family)?;
    ensure!(
        adapter.kind == SourceAdapterKind::FundingRates,
        "adapter {:?} version {:?} is {:?}, not a funding-rate converter",
        adapter.identity,
        adapter.version,
        adapter.kind
    );
    Ok(adapter)
}

/// Aggressor side of a native trade print.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeAggressorSide {
    Buyer,
    Seller,
}

impl TradeAggressorSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Buyer => "BUYER",
            Self::Seller => "SELLER",
        }
    }

    #[cfg(test)]
    fn parse_sample_side(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "buy" => Ok(Self::Buyer),
            "sell" => Ok(Self::Seller),
            other => bail!("unknown trade side token: {other:?}"),
        }
    }

    fn parse_from_mapping(raw: &str, mapping: &CsvTradeMappingConfig) -> Result<Self> {
        let raw = raw.trim();
        if mapping
            .buyer_side_values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(raw))
        {
            return Ok(Self::Buyer);
        }
        if mapping
            .seller_side_values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(raw))
        {
            return Ok(Self::Seller);
        }
        bail!("unknown trade side token: {raw:?}")
    }
}

impl CsvTimestampUnit {
    pub(crate) fn parse_to_nanos(self, raw: &str) -> Result<i64> {
        if self == Self::DecimalSeconds {
            return decimal_seconds_to_nanos(raw);
        }
        let value: i64 = raw.parse().context("timestamp is not an integer")?;
        let multiplier = match self {
            Self::Seconds => NANOS_PER_SECOND,
            // DecimalSeconds returns via decimal_seconds_to_nanos above before any
            // integer parse; reaching this arm is an internal invariant breach, so
            // fail loud instead of panicking.
            Self::DecimalSeconds => {
                bail!("internal: decimal seconds must be handled before integer parsing")
            }
            Self::Milliseconds => 1_000_000,
            Self::Microseconds => 1_000,
            Self::Nanoseconds => 1,
        };
        value
            .checked_mul(multiplier)
            .context("timestamp overflows nanoseconds")
    }
}

fn decimal_seconds_to_nanos(raw: &str) -> Result<i64> {
    let raw = raw.trim();
    ensure!(!raw.is_empty(), "timestamp must not be empty");
    ensure!(!raw.starts_with('-'), "timestamp must not be negative");
    let (whole_seconds, fractional_seconds) = raw.split_once('.').unwrap_or((raw, ""));
    ensure!(
        !whole_seconds.is_empty(),
        "timestamp seconds must not be empty"
    );
    ensure!(
        whole_seconds.chars().all(|value| value.is_ascii_digit()),
        "timestamp seconds must be decimal digits"
    );
    ensure!(
        fractional_seconds
            .chars()
            .all(|value| value.is_ascii_digit()),
        "timestamp fractional seconds must be decimal digits"
    );

    let whole_seconds: i64 = whole_seconds
        .parse()
        .context("timestamp seconds overflow i64")?;
    let whole_nanos = whole_seconds
        .checked_mul(NANOS_PER_SECOND)
        .context("timestamp seconds overflow nanoseconds")?;
    let nanos_digits = if fractional_seconds.len() > 9 {
        let (nanos_digits, sub_nanos_digits) = fractional_seconds.split_at(9);
        ensure!(
            sub_nanos_digits.chars().all(|value| value == '0'),
            "timestamp has sub-nanosecond precision"
        );
        nanos_digits
    } else {
        fractional_seconds
    };
    let mut padded_nanos = nanos_digits.to_string();
    while padded_nanos.len() < 9 {
        padded_nanos.push('0');
    }
    let fractional_nanos = if padded_nanos.is_empty() {
        0
    } else {
        padded_nanos
            .parse::<i64>()
            .context("timestamp fractional seconds overflow i64")?
    };
    whole_nanos
        .checked_add(fractional_nanos)
        .context("timestamp overflows nanoseconds")
}

/// Venue-native instrument identity for the normalized rows.
///
/// Built by the caller from accepted instrument-universe data plus the accepted
/// dataset, so no instrument identity is hardcoded in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalInstrumentIdentity {
    /// Venue-native instrument id, unique within `(venue, product_family)`.
    pub instrument_id: String,
    /// Display or wire symbol from the source.
    pub venue_symbol: String,
    /// NautilusTrader instrument id, such as `SYMBOL.VENUE`.
    pub nt_instrument_id: String,
}

/// Partition key for a normalized `trades` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradesPartition {
    pub venue: String,
    pub product_family: String,
    pub product_category: String,
    pub instrument_id: String,
    /// Archive date partition `YYYY-MM-DD`.
    pub dt: String,
}

/// One normalized native-trade row with full provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalTradeRow {
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
    /// Native trade id / sequence.
    pub source_sequence: Option<String>,
    pub raw_payload_id: String,
    pub source_proof_id: String,
    /// Lowercase SHA-256 hex over the canonical raw object bytes.
    pub payload_hash: String,
    /// Lowercase SHA-256 hex over the transform identity.
    pub transform_hash: String,
    pub trade_source_type: String,
    pub trade_id: String,
    pub aggressor_side: String,
    /// Exact source price string.
    pub price: String,
    /// Exact source size string.
    pub size: String,
    /// Decimal-string notional (`price * size`).
    pub notional: String,
}

/// A validated canonical normalized `trades` table for one accepted object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalTradesTable {
    pub schema_version: String,
    pub partition: TradesPartition,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub fidelity_class: SourceProofFidelityClass,
    pub forbidden_claims: Vec<String>,
    pub transform_hash: String,
    pub payload_hash: String,
    pub rows: Vec<CanonicalTradeRow>,
}

/// Lowercase SHA-256 hex of the transform identity.
#[must_use]
pub fn transform_hash() -> String {
    let mut hasher = Sha256::new();
    hasher.update(TRANSFORM_IDENTITY.as_bytes());
    hex::encode(hasher.finalize())
}

/// Normalize the committed sample spot tick-trades CSV into the canonical
/// `trades` table.
///
/// `csv_text` must be the decompressed text of the accepted object whose hash
/// already matched the manifest (the caller verified it via gate 1).
/// `capture_time_nanos` is the ingest capture timestamp recorded for the run.
/// `ingest_run_id` is the stable identifier of the ingest/run that produced this
/// normalization (the backtest run id), recorded for lineage; it is not the
/// source object URL.
///
/// # Errors
///
/// Returns an error if the header does not match the accepted schema, a row is
/// malformed, or a field fails to parse.
#[cfg(test)]
pub fn normalize_sample_spot_tick_trades(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<CanonicalTradesTable> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        accepted.object.schema_columns == SAMPLE_SPOT_TICK_TRADES_HEADER,
        "accepted object schema {:?} does not match expected spot tick-trades header {:?}",
        accepted.object.schema_columns,
        SAMPLE_SPOT_TICK_TRADES_HEADER
    );

    let canonical_instrument_key = format!(
        "{}/{}/{}",
        accepted.venue, accepted.product_family, identity.instrument_id
    );
    let transform_hash = transform_hash();

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .trim(csv::Trim::All)
        .from_reader(csv_text.as_bytes());
    let header_columns: Vec<&str> = reader
        .headers()
        .context("empty csv: missing header")?
        .iter()
        .collect();
    ensure!(
        header_columns == SAMPLE_SPOT_TICK_TRADES_HEADER,
        "csv header {header_columns:?} does not match expected header {SAMPLE_SPOT_TICK_TRADES_HEADER:?}"
    );

    let mut rows = Vec::new();
    for (index, record) in reader.records().enumerate() {
        let fields = record.with_context(|| format!("row {index}: malformed csv record"))?;
        if fields.iter().all(str::is_empty) {
            continue;
        }
        ensure!(
            fields.len() == SAMPLE_SPOT_TICK_TRADES_HEADER.len(),
            "row {index} has {} fields, expected {}",
            fields.len(),
            SAMPLE_SPOT_TICK_TRADES_HEADER.len()
        );

        let trade_id = fields.get(0).context("missing trade id")?;
        let timestamp_ms: i64 = fields
            .get(1)
            .context("missing timestamp")?
            .parse()
            .with_context(|| format!("row {index}: invalid timestamp {:?}", fields.get(1)))?;
        let price_raw = fields.get(2).context("missing price")?;
        let size_raw = fields.get(3).context("missing size")?;
        let side = TradeAggressorSide::parse_sample_side(fields.get(4).context("missing side")?)?;

        ensure!(!trade_id.is_empty(), "row {index}: empty trade id");
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("row {index}: invalid size {size_raw:?}"))?;
        ensure!(price > Decimal::ZERO, "row {index}: non-positive price");
        ensure!(size > Decimal::ZERO, "row {index}: non-positive size");

        let event_time = timestamp_ms
            .checked_mul(NANOS_PER_MILLISECOND)
            .with_context(|| format!("row {index}: timestamp overflow"))?;
        let notional = price
            .checked_mul(size)
            .with_context(|| format!("row {index}: notional overflow"))?;

        rows.push(CanonicalTradeRow {
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
            event_time,
            capture_time: capture_time_nanos,
            availability_time: None,
            source_sequence: Some(trade_id.to_string()),
            raw_payload_id: accepted.object.sha256.clone(),
            source_proof_id: accepted.source_proof_id.clone(),
            payload_hash: accepted.object.sha256.clone(),
            transform_hash: transform_hash.clone(),
            trade_source_type: TRADE_SOURCE_TYPE_NATIVE.to_string(),
            trade_id: trade_id.to_string(),
            aggressor_side: side.as_str().to_string(),
            price: price_raw.to_string(),
            size: size_raw.to_string(),
            notional: notional.normalize().to_string(),
        });
    }

    let table = CanonicalTradesTable {
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
        rows,
    };
    table.validate()?;
    Ok(table)
}

pub fn normalize_csv_native_trades(
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    mapping: &CsvTradeMappingConfig,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<CanonicalTradesTable> {
    ensure!(
        !ingest_run_id.trim().is_empty(),
        "ingest_run_id must not be empty"
    );
    ensure!(
        !mapping.trade_id_column.trim().is_empty(),
        "converter csv.trade_id_column must not be empty"
    );
    ensure!(
        !mapping.timestamp_column.trim().is_empty(),
        "converter csv.timestamp_column must not be empty"
    );
    ensure!(
        !mapping.price_column.trim().is_empty(),
        "converter csv.price_column must not be empty"
    );
    ensure!(
        !mapping.size_column.trim().is_empty(),
        "converter csv.size_column must not be empty"
    );
    ensure!(
        !mapping.side_column.trim().is_empty(),
        "converter csv.side_column must not be empty"
    );
    ensure!(
        !mapping.buyer_side_values.is_empty(),
        "converter csv.buyer_side_values must not be empty"
    );
    ensure!(
        !mapping.seller_side_values.is_empty(),
        "converter csv.seller_side_values must not be empty"
    );

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

    let trade_id_index = column_index(&header_columns, &mapping.trade_id_column)?;
    let timestamp_index = column_index(&header_columns, &mapping.timestamp_column)?;
    let price_index = column_index(&header_columns, &mapping.price_column)?;
    let size_index = column_index(&header_columns, &mapping.size_column)?;
    let side_index = column_index(&header_columns, &mapping.side_column)?;

    let canonical_instrument_key = format!(
        "{}/{}/{}",
        accepted.venue, accepted.product_family, identity.instrument_id
    );
    let transform_hash = transform_hash();

    let mut rows = Vec::new();
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

        let trade_id = fields.get(trade_id_index).context("missing trade id")?;
        let timestamp_raw = fields.get(timestamp_index).context("missing timestamp")?;
        let event_time = mapping
            .timestamp_unit
            .parse_to_nanos(timestamp_raw)
            .with_context(|| format!("row {index}: invalid timestamp {timestamp_raw:?}"))?;
        let price_raw = fields.get(price_index).context("missing price")?;
        let size_raw = fields.get(size_index).context("missing size")?;
        let side = TradeAggressorSide::parse_from_mapping(
            fields.get(side_index).context("missing side")?,
            mapping,
        )?;

        ensure!(!trade_id.is_empty(), "row {index}: empty trade id");
        let price: Decimal = price_raw
            .parse()
            .with_context(|| format!("row {index}: invalid price {price_raw:?}"))?;
        let size: Decimal = size_raw
            .parse()
            .with_context(|| format!("row {index}: invalid size {size_raw:?}"))?;
        ensure!(price > Decimal::ZERO, "row {index}: non-positive price");
        ensure!(size > Decimal::ZERO, "row {index}: non-positive size");
        let notional = price * size;

        rows.push(CanonicalTradeRow {
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
            event_time,
            capture_time: capture_time_nanos,
            availability_time: None,
            source_sequence: Some(trade_id.to_string()),
            raw_payload_id: accepted.object.sha256.clone(),
            source_proof_id: accepted.source_proof_id.clone(),
            payload_hash: accepted.object.sha256.clone(),
            transform_hash: transform_hash.clone(),
            trade_source_type: TRADE_SOURCE_TYPE_NATIVE.to_string(),
            trade_id: trade_id.to_string(),
            aggressor_side: side.as_str().to_string(),
            price: price_raw.to_string(),
            size: size_raw.to_string(),
            notional: notional.normalize().to_string(),
        });
    }

    let table = CanonicalTradesTable {
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
        rows,
    };
    table.validate()?;
    Ok(table)
}

pub fn normalize_registered_trade_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<CanonicalTradesTable> {
    let converter = require_registered_trade_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::CsvNativeTrades => normalize_csv_native_trades(
            accepted,
            identity,
            &converter_config.csv,
            csv_text,
            capture_time_nanos,
            ingest_run_id,
        ),
        SourceAdapterKind::CsvNativeBars => {
            bail!("CSV native-bars adapter cannot normalize native trades")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!("paged-JSON bar adapter cannot normalize native trades")
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!("JSONL multi-interval bar adapter cannot normalize native trades")
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!("JSONL snapshot-delta adapter cannot normalize native trades")
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!("tar JSONL snapshot-delta adapter cannot normalize native trades")
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!(
                "Parquet event-stream delta adapter dual-emits trades alongside \
                 order-book deltas; call normalize_registered_event_stream_delta_converter \
                 with the parquet bytes, not the single-object native-trades path"
            )
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize native trades")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize native trades")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize native trades")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize native trades")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize native trades")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize native trades")
        }
    }
}

/// Normalize an accepted CSV bar object through the registered bar converter
/// selected by the run-spec, fail-closing when the kind/config do not match.
///
/// Mirrors [`normalize_registered_trade_converter`]: the adapter kind must be
/// [`SourceAdapterKind::CsvNativeBars`] for the accepted object's table family,
/// and the run-spec must carry the `bars` mapping that kind requires.
///
/// # Errors
///
/// Returns an error if the converter is not a registered bar converter for the
/// table family, the `bars` mapping is absent, or normalization fails.
pub fn normalize_registered_bar_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identities: &super::canonical_bars::BarInstrumentIdentities,
    csv_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalBarsTable>> {
    let converter = require_registered_bar_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::CsvNativeBars => {
            let mapping = converter_config.bars.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a CSV native-bars adapter but carries no [converter.bars] mapping",
                    converter.identity
                )
            })?;
            super::canonical_bars::normalize_csv_native_bars(
                accepted,
                identities,
                mapping,
                csv_text,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize native bars")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!(
                "paged-JSON bar adapter requires the JSON entry point; call \
                 normalize_registered_paged_json_bar_converter with the JSON text, not \
                 the CSV native-bars path"
            )
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!(
                "JSONL multi-interval bar adapter requires the JSONL entry point; call \
                 normalize_registered_jsonl_multi_interval_bar_converter with the JSONL text, \
                 not the CSV native-bars path"
            )
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!("JSONL snapshot-delta adapter cannot normalize native bars")
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!("tar JSONL snapshot-delta adapter cannot normalize native bars")
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!("Parquet event-stream delta adapter cannot normalize native bars")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize native bars")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize native bars")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize native bars")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize native bars")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize native bars")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize native bars")
        }
    }
}

/// Normalize an accepted paged-JSON kline object through the registered
/// paged-JSON bar converter selected by the run-spec, fail-closing when the
/// kind/config do not match.
///
/// Mirrors [`normalize_registered_bar_converter`]: the adapter kind must be
/// [`SourceAdapterKind::PagedJsonBars`] for the accepted object's table family,
/// and the run-spec must carry the `paged_json_bars` mapping that kind requires.
/// Paged REST is per-instrument, so the caller binds the single identity.
///
/// # Errors
///
/// Returns an error if the converter is not a registered paged-JSON bar converter
/// for the table family, the `paged_json_bars` mapping is absent, or
/// normalization fails.
pub fn normalize_registered_paged_json_bar_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    json_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalBarsTable>> {
    let converter = require_registered_paged_json_bar_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::PagedJsonBars => {
            let mapping = converter_config.paged_json_bars.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a paged-JSON bar adapter but carries no [converter.paged_json_bars] mapping",
                    converter.identity
                )
            })?;
            super::canonical_bars::normalize_paged_json_bars(
                accepted,
                identity,
                mapping,
                json_text,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!(
                "CSV native-bars adapter requires the CSV entry point; call \
                 normalize_registered_bar_converter with the CSV text, not the paged-JSON path"
            )
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!(
                "JSONL multi-interval bar adapter requires the JSONL entry point; call \
                 normalize_registered_jsonl_multi_interval_bar_converter, not the paged-JSON path"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!("JSONL snapshot-delta adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!("tar JSONL snapshot-delta adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!("Parquet event-stream delta adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize paged-JSON bars")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize paged-JSON bars")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize paged-JSON bars")
        }
    }
}

/// Normalize an accepted line-delimited multi-interval kline object through the
/// registered JSONL multi-interval bar converter selected by the run-spec,
/// fail-closing when the kind/config do not match.
///
/// Mirrors [`normalize_registered_bar_converter`]: the adapter kind must be
/// [`SourceAdapterKind::JsonlMultiIntervalBars`] for the accepted object's table
/// family, and the run-spec must carry the `jsonl_bars` mapping that kind
/// requires. One object emits one table per `(instrument, interval)` group, so a
/// multi-instrument object binds keyed identities.
///
/// # Errors
///
/// Returns an error if the converter is not a registered JSONL multi-interval bar
/// converter for the table family, the `jsonl_bars` mapping is absent, or
/// normalization fails.
pub fn normalize_registered_jsonl_multi_interval_bar_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identities: &super::canonical_bars::BarInstrumentIdentities,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalBarsTable>> {
    let converter = require_registered_jsonl_multi_interval_bar_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::JsonlMultiIntervalBars => {
            let mapping = converter_config.jsonl_bars.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a JSONL multi-interval bar adapter but carries no [converter.jsonl_bars] mapping",
                    converter.identity
                )
            })?;
            super::canonical_bars::normalize_jsonl_multi_interval_bars(
                accepted,
                identities,
                mapping,
                jsonl_text,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!(
                "CSV native-bars adapter requires the CSV entry point; call \
                 normalize_registered_bar_converter with the CSV text, not the JSONL \
                 multi-interval path"
            )
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!(
                "paged-JSON bar adapter requires the JSON entry point; call \
                 normalize_registered_paged_json_bar_converter, not the JSONL multi-interval path"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!("JSONL snapshot-delta adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!("tar JSONL snapshot-delta adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!("Parquet event-stream delta adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize JSONL multi-interval bars")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize JSONL multi-interval bars")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize JSONL multi-interval bars")
        }
    }
}

/// Normalize an accepted JSONL periodic-full-snapshot object through the
/// registered order-book-delta converter selected by the run-spec, fail-closing
/// when the kind/config do not match.
///
/// Mirrors [`normalize_registered_bar_converter`]: the adapter kind must be
/// [`SourceAdapterKind::JsonlSnapshotDeltas`] for the accepted object's table
/// family, and the run-spec must carry the `deltas` mapping that kind requires.
///
/// # Errors
///
/// Returns an error if the converter is not a registered order-book-delta
/// converter for the table family, the `deltas` mapping is absent, or
/// normalization fails.
pub fn normalize_registered_order_book_delta_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identities: &super::canonical_order_book_deltas::DeltaInstrumentIdentities,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalOrderBookDeltasTable>> {
    let converter = require_registered_order_book_delta_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::JsonlSnapshotDeltas => {
            let mapping = converter_config.deltas.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a JSONL snapshot-delta adapter but carries no [converter.deltas] mapping",
                    converter.identity
                )
            })?;
            super::canonical_order_book_deltas::normalize_jsonl_snapshot_deltas(
                accepted,
                identities,
                mapping,
                jsonl_text,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!(
                "tar JSONL snapshot-delta adapter requires the streaming member \
                 entry point; call normalize_registered_tar_order_book_delta_converter \
                 with the tar member iterator, not the single-object jsonl_text path"
            )
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!(
                "Parquet event-stream delta adapter requires the dual-emit parquet \
                 entry point; call normalize_registered_event_stream_delta_converter \
                 with the parquet bytes, not the single-object jsonl_text path"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!("CSV native-bars adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!("paged-JSON bar adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!("JSONL multi-interval bar adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize order-book deltas")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize order-book deltas")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize order-book deltas")
        }
    }
}

/// Normalize an accepted streaming gzip-tar of JSONL periodic-full-snapshot
/// members through the registered tar order-book-delta converter selected by the
/// run-spec, fail-closing when the kind/config do not match.
///
/// The container concern (decompress the gzip-tar + walk its members) stays with
/// the caller exactly as the single-object decode does for the JSONL path: the
/// caller passes the streaming member iterator from
/// [`super::tar_reader::gzip_tar_members`]. This mirrors how `CsvGzip` vs
/// `SingleCsvZip` containers are distinguished at the decode boundary rather than
/// inside the per-kind dispatcher — the dispatcher stays per-kind and never owns
/// the container. The kind must be [`SourceAdapterKind::TarJsonlSnapshotDeltas`]
/// for the accepted object's table family, and the run-spec must carry the
/// `deltas` mapping that kind requires (the wire shape is shared with the JSONL
/// path; only the container differs).
///
/// # Errors
///
/// Returns an error if the converter is not a registered tar order-book-delta
/// converter for the table family, the `deltas` mapping is absent, or
/// normalization fails.
pub fn normalize_registered_tar_order_book_delta_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identities: &super::canonical_order_book_deltas::DeltaInstrumentIdentities,
    members: impl Iterator<Item = Result<super::tar_reader::TarMember>>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalOrderBookDeltasTable>> {
    let converter = require_registered_tar_order_book_delta_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            let mapping = converter_config.deltas.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a tar JSONL snapshot-delta adapter but carries no [converter.deltas] mapping",
                    converter.identity
                )
            })?;
            super::canonical_order_book_deltas::normalize_tar_jsonl_snapshot_deltas(
                accepted,
                identities,
                mapping,
                members,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!(
                "JSONL snapshot-delta adapter requires the single-object entry \
                 point; call normalize_registered_order_book_delta_converter with \
                 the decoded jsonl_text, not the tar member iterator"
            )
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!(
                "Parquet event-stream delta adapter requires the dual-emit parquet \
                 entry point; call normalize_registered_event_stream_delta_converter \
                 with the parquet bytes, not the tar member iterator"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!("CSV native-bars adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!("paged-JSON bar adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!("JSONL multi-interval bar adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize tar order-book deltas")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize tar order-book deltas")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize tar order-book deltas")
        }
    }
}

/// Normalize an accepted typed-event Parquet stream through the registered
/// dual-emit converter selected by the run-spec, fail-closing when the kind/config
/// do not match.
///
/// The container concern (decode the accepted object to Parquet bytes) stays with
/// the caller exactly as the JSONL/tar paths own their decode. The kind must be
/// [`SourceAdapterKind::ParquetEventStreamDeltas`] for the accepted object's table
/// family, and the run-spec must carry the `deltas` mapping that kind requires
/// (an [`super::canonical_order_book_deltas::DeltaSourceFormat::EventStream`]
/// format). Returns BOTH the order-book-delta family and the trades family under
/// the dual-fidelity rule.
///
/// # Errors
///
/// Returns an error if the converter is not a registered event-stream delta
/// converter for the table family, the `deltas` mapping is absent or not an
/// `EventStream` format, or normalization fails.
pub fn normalize_registered_event_stream_delta_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identities: &super::canonical_order_book_deltas::DeltaInstrumentIdentities,
    parquet_bytes: &[u8],
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<(
    Vec<super::canonical_market_data::CanonicalOrderBookDeltasTable>,
    Vec<CanonicalTradesTable>,
)> {
    let converter = require_registered_event_stream_delta_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::ParquetEventStreamDeltas => {
            let mapping = converter_config.deltas.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a Parquet event-stream delta adapter but carries no [converter.deltas] mapping",
                    converter.identity
                )
            })?;
            super::canonical_order_book_deltas::normalize_parquet_event_stream_deltas(
                accepted,
                identities,
                mapping,
                parquet_bytes,
                capture_time_nanos,
                ingest_run_id,
            )
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!(
                "JSONL snapshot-delta adapter requires the single-object entry point; \
                 call normalize_registered_order_book_delta_converter, not the \
                 dual-emit parquet path"
            )
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!(
                "tar JSONL snapshot-delta adapter requires the streaming member entry \
                 point; call normalize_registered_tar_order_book_delta_converter, not \
                 the dual-emit parquet path"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!("CSV native-bars adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!("paged-JSON bar adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!("JSONL multi-interval bar adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::SnapshotQuotes => {
            bail!("snapshot-quotes adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!("seeded L2 quote adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize event-stream deltas")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize event-stream deltas")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize event-stream deltas")
        }
    }
}

/// Normalize accepted snapshot-seeded L2 JSONL into top-of-book quote tables.
///
/// The adapter kind must be [`SourceAdapterKind::SeededL2Quotes`] for the
/// accepted object's quote table family. The mapping is run-spec-owned and
/// describes the JSON action/time/level fields; replay starts only after a
/// source snapshot seeds the book.
///
/// # Errors
///
/// Returns an error if the converter is not a seeded-L2 quote converter, if the
/// mapping is absent, if an update precedes the seeding snapshot, or if the
/// produced canonical quote table fails validation.
pub fn normalize_registered_seeded_l2_quote_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    jsonl_text: &str,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalQuotesTable>> {
    let converter = require_registered_seeded_l2_quote_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    let mapping = converter_config.seeded_l2_quotes.as_ref().with_context(|| {
        format!(
            "converter {:?} is a seeded L2 quote adapter but carries no [converter.seeded_l2_quotes] mapping",
            converter.identity
        )
    })?;
    super::seeded_l2_quotes::normalize_seeded_l2_jsonl_quotes(
        accepted,
        identity,
        mapping,
        jsonl_text,
        capture_time_nanos,
        ingest_run_id,
    )
}

/// Normalize accepted tar-bundled snapshot-seeded L2 JSONL into top-of-book
/// quote tables.
///
/// # Errors
///
/// Returns an error if the converter is not a seeded-L2 quote converter, if the
/// mapping is absent, if a tar member is malformed, if an update precedes the
/// seeding snapshot, or if the produced canonical quote table fails validation.
pub fn normalize_registered_tar_seeded_l2_quote_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    identity: &CanonicalInstrumentIdentity,
    members: impl IntoIterator<Item = super::tar_reader::TarMember>,
    capture_time_nanos: i64,
    ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalQuotesTable>> {
    let converter = require_registered_seeded_l2_quote_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    let mapping = converter_config.seeded_l2_quotes.as_ref().with_context(|| {
        format!(
            "converter {:?} is a seeded L2 quote adapter but carries no [converter.seeded_l2_quotes] mapping",
            converter.identity
        )
    })?;
    super::seeded_l2_quotes::normalize_seeded_l2_tar_jsonl_quotes(
        accepted,
        identity,
        mapping,
        members,
        capture_time_nanos,
        ingest_run_id,
    )
}

/// Normalize an accepted top-of-book snapshot object through the registered
/// snapshot-quotes converter selected by the run-spec, fail-closing when the
/// kind/config do not match.
///
/// Mirrors [`normalize_registered_order_book_delta_converter`]: the adapter kind
/// must be [`SourceAdapterKind::SnapshotQuotes`] for the accepted object's table
/// family, and the run-spec must carry the `quotes` mapping that kind requires.
///
/// SCOPE (slice S3quote): this slice owns the canonical [`CanonicalQuotesTable`],
/// its projection, the registered adapter, the operator dispatch wiring, and the
/// `quotes` config struct. The wire-format parser that fills a
/// [`CanonicalQuotesTable`] from raw bytes lands in a FOLLOW-UP slice. This entry
/// point therefore validates that the converter is a real registered
/// snapshot-quotes adapter carrying its `quotes` mapping, then fails loud naming
/// the follow-up — a registered seam that fails loud is tracked work, and the
/// canonical table + projection are proven by the synthetic round-trip test in
/// [`super::catalog_projection`].
///
/// # Errors
///
/// Returns an error if the converter is not a registered snapshot-quotes
/// converter for the table family, the `quotes` mapping is absent, or — until the
/// follow-up parser slice — always (naming that follow-up).
pub fn normalize_registered_quote_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    _identity: &CanonicalInstrumentIdentity,
    _text: &str,
    _capture_time_nanos: i64,
    _ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalQuotesTable>> {
    let converter = require_registered_quote_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    match converter.kind {
        SourceAdapterKind::SnapshotQuotes => {
            let _mapping = converter_config.quotes.as_ref().with_context(|| {
                format!(
                    "converter {:?} is a snapshot-quotes adapter but carries no [converter.quotes] mapping",
                    converter.identity
                )
            })?;
            bail!(
                "snapshot-quotes wire normalizer is a registered seam but its parsing \
                 path lands in a follow-up slice; the CanonicalQuotesTable contract and \
                 its catalog projection are proven by the synthetic round-trip test"
            )
        }
        SourceAdapterKind::CsvNativeTrades => {
            bail!("CSV native-trades adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::CsvNativeBars => {
            bail!("CSV native-bars adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::PagedJsonBars => {
            bail!("paged-JSON bar adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::JsonlMultiIntervalBars => {
            bail!("JSONL multi-interval bar adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::JsonlSnapshotDeltas => {
            bail!("JSONL snapshot-delta adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::TarJsonlSnapshotDeltas => {
            bail!("tar JSONL snapshot-delta adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::ParquetEventStreamDeltas => {
            bail!("Parquet event-stream delta adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::SeededL2Quotes => {
            bail!(
                "seeded L2 quote adapter requires the seeded-L2 quote entry point; \
                 call normalize_registered_seeded_l2_quote_converter"
            )
        }
        SourceAdapterKind::IndexPrices => {
            bail!("index-price adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::MarkPrices => {
            bail!("mark-price adapter cannot normalize top-of-book quotes")
        }
        SourceAdapterKind::FundingRates => {
            bail!("funding-rate adapter cannot normalize top-of-book quotes")
        }
        #[cfg(test)]
        SourceAdapterKind::SyntheticOrderBookDeltas => {
            bail!("test fixture adapter cannot normalize top-of-book quotes")
        }
    }
}

/// Registered-seam entry point for the index-price source normalizer.
///
/// Symmetric with [`normalize_registered_quote_converter`]: raw index-price wire
/// acquisition from source bytes into a [`CanonicalIndexPricesTable`] lands in a
/// FOLLOW-UP slice tracked by bolt-v2 #836/#437. This entry point validates that
/// the converter is a real registered index-price adapter for the table family,
/// then fails loud naming the follow-up — a registered seam that fails loud is
/// tracked work, and the canonical table + its NT `IndexPriceUpdate` projection
/// are proven by the synthetic round-trip test in [`super::catalog_projection`].
///
/// # Errors
///
/// Returns an error if the converter is not a registered index-price converter
/// for the table family, or — until the follow-up acquisition slice — always
/// (naming that follow-up).
pub fn normalize_registered_index_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    _identity: &CanonicalInstrumentIdentity,
    _text: &str,
    _capture_time_nanos: i64,
    _ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalIndexPricesTable>> {
    let _adapter = require_registered_index_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    bail!(
        "index-price wire normalizer is a registered seam but its raw acquisition \
         path lands in a follow-up slice tracked by bolt-v2 #836/#437; the CanonicalIndexPricesTable \
         contract and its catalog projection are proven by the synthetic round-trip test"
    )
}

/// Registered-seam entry point for the mark-price source normalizer.
///
/// Symmetric with [`normalize_registered_quote_converter`]: raw mark-price wire
/// acquisition from source bytes into a [`CanonicalMarkPricesTable`] lands in a
/// FOLLOW-UP slice tracked by bolt-v2 #836/#437. This entry point validates that
/// the converter is a real registered mark-price adapter for the table family,
/// then fails loud naming the follow-up — a registered seam that fails loud is
/// tracked work, and the canonical table + its NT `MarkPriceUpdate` projection
/// are proven by the synthetic round-trip test in [`super::catalog_projection`].
///
/// # Errors
///
/// Returns an error if the converter is not a registered mark-price converter
/// for the table family, or — until the follow-up acquisition slice — always
/// (naming that follow-up).
pub fn normalize_registered_mark_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    _identity: &CanonicalInstrumentIdentity,
    _text: &str,
    _capture_time_nanos: i64,
    _ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalMarkPricesTable>> {
    let _adapter = require_registered_mark_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    bail!(
        "mark-price wire normalizer is a registered seam but its raw acquisition \
         path lands in a follow-up slice tracked by bolt-v2 #836/#437; the CanonicalMarkPricesTable \
         contract and its catalog projection are proven by the synthetic round-trip test"
    )
}

/// Registered-seam entry point for the funding-rate source normalizer.
///
/// Symmetric with [`normalize_registered_index_converter`]: raw funding-rate
/// acquisition from source bytes into a [`CanonicalFundingRatesTable`] lands in a
/// FOLLOW-UP slice tracked by bolt-v2 #836/#437. This entry point validates that
/// the converter is a real registered funding-rate adapter for the table family,
/// then fails loud naming the follow-up. The canonical table + its NT
/// `FundingRateUpdate` projection are proven by the synthetic round-trip test
/// in [`super::catalog_projection`].
///
/// # Errors
///
/// Returns an error if the converter is not a registered funding-rate converter
/// for the table family, or — until the follow-up acquisition slice — always
/// (naming that follow-up).
pub fn normalize_registered_funding_converter(
    converter_config: &ConverterConfig,
    accepted: &AcceptedDataset,
    _identity: &CanonicalInstrumentIdentity,
    _text: &str,
    _capture_time_nanos: i64,
    _ingest_run_id: &str,
) -> Result<Vec<super::canonical_market_data::CanonicalFundingRatesTable>> {
    let _adapter = require_registered_funding_converter_for_table_family(
        &converter_config.identity,
        &converter_config.version,
        &accepted.table_family,
    )?;
    bail!(
        "funding-rate wire normalizer is a registered seam but its raw acquisition \
         path lands in a follow-up slice tracked by bolt-v2 #836/#437; the CanonicalFundingRatesTable \
         contract and its catalog projection are proven by the synthetic round-trip test"
    )
}

pub(crate) fn column_index(header_columns: &[String], column_name: &str) -> Result<usize> {
    header_columns
        .iter()
        .position(|column| column == column_name)
        .with_context(|| format!("configured converter column {column_name:?} missing from csv"))
}

impl CanonicalTradesTable {
    /// Validate required fields, timestamps, instrument ids, partition, and
    /// schema version.
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
        ensure!(!self.rows.is_empty(), "canonical trades table is empty");
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
            self.fidelity_class != SourceProofFidelityClass::L2Replay,
            "trade prints must not be labelled L2_REPLAY"
        );
        ensure!(
            !self.forbidden_claims.is_empty(),
            "trade-replay table must carry explicit forbidden claims"
        );

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
                row.instrument_id == self.partition.instrument_id,
                "row {index}: instrument_id does not match partition"
            );
            ensure!(
                row.trade_source_type == TRADE_SOURCE_TYPE_NATIVE,
                "row {index}: only native trade prints are allowed"
            );
            for field in [
                &row.instrument_id,
                &row.canonical_instrument_key,
                &row.venue_symbol,
                &row.raw_payload_id,
                &row.source_proof_id,
                &row.payload_hash,
                &row.trade_id,
                &row.price,
                &row.size,
                &row.notional,
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
        }
        Ok(())
    }

    /// Arrow schema for the canonical `trades` table.
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
            utf8("trade_source_type"),
            utf8("trade_id"),
            utf8("aggressor_side"),
            utf8("price"),
            utf8("size"),
            utf8("notional"),
        ]))
    }

    fn to_record_batch(&self) -> Result<RecordBatch> {
        let utf8_col = |f: fn(&CanonicalTradeRow) -> &str| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as arrow::array::ArrayRef
        };
        let int64_col = |f: fn(&CanonicalTradeRow) -> i64| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as arrow::array::ArrayRef
        };
        let opt_utf8_col = |f: fn(&CanonicalTradeRow) -> Option<&str>| {
            Arc::new(StringArray::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as arrow::array::ArrayRef
        };
        let opt_int64_col = |f: fn(&CanonicalTradeRow) -> Option<i64>| {
            Arc::new(Int64Array::from(
                self.rows.iter().map(f).collect::<Vec<_>>(),
            )) as arrow::array::ArrayRef
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
                utf8_col(|r| r.trade_source_type.as_str()),
                utf8_col(|r| r.trade_id.as_str()),
                utf8_col(|r| r.aggressor_side.as_str()),
                utf8_col(|r| r.price.as_str()),
                utf8_col(|r| r.size.as_str()),
                utf8_col(|r| r.notional.as_str()),
            ],
        )
        .context("failed to build canonical trades record batch")
    }

    /// Write the canonical normalized table as a Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the table is invalid or the file cannot be written.
    pub fn write_parquet(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let batch = self.to_record_batch()?;
        let file = File::create(path)
            .with_context(|| format!("failed to create canonical artifact {}", path.display()))?;
        let mut writer = ArrowWriter::try_new(file, batch.schema(), None)
            .context("failed to construct parquet writer")?;
        writer.write(&batch).context("failed to write batch")?;
        writer.close().context("failed to finalize parquet")?;
        Ok(())
    }

    /// Read a canonical normalized table from an existing Parquet artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if the artifact does not match the canonical schema or
    /// does not bind to the accepted source proof/object.
    pub fn read_parquet(path: &Path, accepted: &AcceptedDataset) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open canonical artifact {}", path.display()))?;
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .context("failed to construct canonical parquet reader")?
            .build()
            .context("failed to build canonical parquet reader")?;

        let mut rows = Vec::new();
        for batch in reader {
            let batch = batch.context("failed to read canonical parquet batch")?;
            let schema_version = string_column(&batch, "schema_version")?;
            let ingest_run_id = string_column(&batch, "ingest_run_id")?;
            let source_binding = string_column(&batch, "source_binding")?;
            let venue = string_column(&batch, "venue")?;
            let product_family = string_column(&batch, "product_family")?;
            let product_category = string_column(&batch, "product_category")?;
            let instrument_id = string_column(&batch, "instrument_id")?;
            let canonical_instrument_key = string_column(&batch, "canonical_instrument_key")?;
            let venue_symbol = string_column(&batch, "venue_symbol")?;
            let nt_instrument_id = string_column(&batch, "nt_instrument_id")?;
            let event_time = int64_column(&batch, "event_time")?;
            let capture_time = int64_column(&batch, "capture_time")?;
            let availability_time = int64_column(&batch, "availability_time")?;
            let source_sequence = string_column(&batch, "source_sequence")?;
            let raw_payload_id = string_column(&batch, "raw_payload_id")?;
            let source_proof_id = string_column(&batch, "source_proof_id")?;
            let payload_hash = string_column(&batch, "payload_hash")?;
            let row_transform_hash = string_column(&batch, "transform_hash")?;
            let trade_source_type = string_column(&batch, "trade_source_type")?;
            let trade_id = string_column(&batch, "trade_id")?;
            let aggressor_side = string_column(&batch, "aggressor_side")?;
            let price = string_column(&batch, "price")?;
            let size = string_column(&batch, "size")?;
            let notional = string_column(&batch, "notional")?;

            for index in 0..batch.num_rows() {
                rows.push(CanonicalTradeRow {
                    schema_version: required_string(schema_version, index, "schema_version")?,
                    ingest_run_id: required_string(ingest_run_id, index, "ingest_run_id")?,
                    source_binding: required_string(source_binding, index, "source_binding")?,
                    venue: required_string(venue, index, "venue")?,
                    product_family: required_string(product_family, index, "product_family")?,
                    product_category: required_string(product_category, index, "product_category")?,
                    instrument_id: required_string(instrument_id, index, "instrument_id")?,
                    canonical_instrument_key: required_string(
                        canonical_instrument_key,
                        index,
                        "canonical_instrument_key",
                    )?,
                    venue_symbol: required_string(venue_symbol, index, "venue_symbol")?,
                    nt_instrument_id: optional_string(nt_instrument_id, index),
                    event_time: required_i64(event_time, index, "event_time")?,
                    capture_time: required_i64(capture_time, index, "capture_time")?,
                    availability_time: optional_i64(availability_time, index),
                    source_sequence: optional_string(source_sequence, index),
                    raw_payload_id: required_string(raw_payload_id, index, "raw_payload_id")?,
                    source_proof_id: required_string(source_proof_id, index, "source_proof_id")?,
                    payload_hash: required_string(payload_hash, index, "payload_hash")?,
                    transform_hash: required_string(row_transform_hash, index, "transform_hash")?,
                    trade_source_type: required_string(
                        trade_source_type,
                        index,
                        "trade_source_type",
                    )?,
                    trade_id: required_string(trade_id, index, "trade_id")?,
                    aggressor_side: required_string(aggressor_side, index, "aggressor_side")?,
                    price: required_string(price, index, "price")?,
                    size: required_string(size, index, "size")?,
                    notional: required_string(notional, index, "notional")?,
                });
            }
        }

        let first = rows.first().context("canonical trades parquet is empty")?;
        let table = Self {
            schema_version: NORMALIZED_SCHEMA_VERSION.to_string(),
            partition: TradesPartition {
                venue: first.venue.clone(),
                product_family: first.product_family.clone(),
                product_category: first.product_category.clone(),
                instrument_id: first.instrument_id.clone(),
                dt: accepted.object.archive_date.clone(),
            },
            source_proof_id: accepted.source_proof_id.clone(),
            source_proof_version: accepted.source_proof_version,
            fidelity_class: accepted.fidelity_class,
            forbidden_claims: accepted.forbidden_claims.clone(),
            transform_hash: transform_hash(),
            payload_hash: accepted.object.sha256.clone(),
            rows,
        };
        table.validate()?;
        table.validate_loaded_against_accepted(accepted)?;
        Ok(table)
    }

    fn validate_loaded_against_accepted(&self, accepted: &AcceptedDataset) -> Result<()> {
        ensure!(
            self.partition.venue == accepted.venue,
            "canonical artifact venue mismatch: expected {:?}, got {:?}",
            accepted.venue,
            self.partition.venue
        );
        ensure!(
            self.partition.product_family == accepted.product_family,
            "canonical artifact product_family mismatch"
        );
        ensure!(
            self.partition.product_category == accepted.product_category,
            "canonical artifact product_category mismatch"
        );
        ensure!(
            self.partition.dt == accepted.object.archive_date,
            "canonical artifact date mismatch"
        );
        ensure!(
            self.source_proof_id == accepted.source_proof_id,
            "canonical artifact source_proof_id mismatch"
        );
        ensure!(
            self.source_proof_version == accepted.source_proof_version,
            "canonical artifact source_proof_version mismatch"
        );
        ensure!(
            self.payload_hash == accepted.object.sha256,
            "canonical artifact payload_hash mismatch"
        );
        for (index, row) in self.rows.iter().enumerate() {
            ensure!(
                row.source_binding == accepted.source_binding,
                "row {index}: source_binding mismatch"
            );
            ensure!(row.venue == accepted.venue, "row {index}: venue mismatch");
            ensure!(
                row.product_family == accepted.product_family,
                "row {index}: product_family mismatch"
            );
            ensure!(
                row.product_category == accepted.product_category,
                "row {index}: product_category mismatch"
            );
            ensure!(
                row.raw_payload_id == accepted.object.sha256,
                "row {index}: raw_payload_id mismatch"
            );
            ensure!(
                row.payload_hash == accepted.object.sha256,
                "row {index}: payload_hash mismatch"
            );
            ensure!(
                row.source_proof_id == accepted.source_proof_id,
                "row {index}: source_proof_id mismatch"
            );
            ensure!(
                row.transform_hash == self.transform_hash,
                "row {index}: transform_hash mismatch"
            );
        }
        Ok(())
    }
}

fn string_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("canonical parquet missing column {name:?}"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("canonical parquet column {name:?} is not Utf8"))
}

fn int64_column<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int64Array> {
    batch
        .column_by_name(name)
        .with_context(|| format!("canonical parquet missing column {name:?}"))?
        .as_any()
        .downcast_ref::<Int64Array>()
        .with_context(|| format!("canonical parquet column {name:?} is not Int64"))
}

fn required_string(column: &StringArray, row: usize, name: &str) -> Result<String> {
    ensure!(
        !column.is_null(row),
        "row {row}: required column {name:?} is null"
    );
    Ok(column.value(row).to_string())
}

fn optional_string(column: &StringArray, row: usize) -> Option<String> {
    (!column.is_null(row)).then(|| column.value(row).to_string())
}

fn required_i64(column: &Int64Array, row: usize, name: &str) -> Result<i64> {
    ensure!(
        !column.is_null(row),
        "row {row}: required column {name:?} is null"
    );
    Ok(column.value(row))
}

fn optional_i64(column: &Int64Array, row: usize) -> Option<i64> {
    (!column.is_null(row)).then(|| column.value(row))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_projection::{
        NT_DATA_TYPE_BAR, NT_DATA_TYPE_ORDER_BOOK_DELTA, NT_DATA_TYPE_TRADE_TICK,
    };
    use crate::source_proof::{
        AcceptanceMode, AcceptanceScope, EvidenceState, FixtureType, IngestManifestObjectRecord,
        L2ReplayEvidence, LicenseScope, NtMappingStatus, RequiredCheck, RequiredChecks,
        SourceCandidateClass, SourceProofClaimLimit, SourceProofReport, SourceProofStatus,
        SourceProofUsageScope, SourceSelectionStatus, TimeRange,
    };

    #[test]
    fn source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter(TRANSFORM_IDENTITY, TRANSFORM_VERSION)
            .expect("registered source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::CsvNativeTrades);
        assert_eq!(adapter.table_family, TRADE_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_TRADE_TICK);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&CSV_NATIVE_TRADES_ADAPTER));
        assert_eq!(REGISTERED_TRADE_CONVERTERS, &[CSV_NATIVE_TRADES_ADAPTER]);
    }

    #[test]
    fn source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{TRADE_TABLE_FAMILY}_mismatch");
        let err = require_registered_source_adapter_for_table_family(
            TRANSFORM_IDENTITY,
            TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn bar_source_adapter_registry_exposes_data_family_metadata() {
        let adapter =
            require_registered_source_adapter(BAR_TRANSFORM_IDENTITY, BAR_TRANSFORM_VERSION)
                .expect("registered bar source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::CsvNativeBars);
        assert_eq!(adapter.table_family, BAR_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_BAR);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&CSV_NATIVE_BARS_ADAPTER));
        assert_eq!(
            registered_bar_converter(BAR_TRANSFORM_IDENTITY, BAR_TRANSFORM_VERSION),
            Some(&CSV_NATIVE_BARS_ADAPTER)
        );
    }

    #[test]
    fn bar_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{BAR_TABLE_FAMILY}_mismatch");
        let err = require_registered_bar_converter_for_table_family(
            BAR_TRANSFORM_IDENTITY,
            BAR_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("bar adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn bar_and_trade_converter_guards_reject_each_others_kind() {
        // The trade guard rejects the bar adapter id, and the bar guard rejects
        // the trade adapter id, so a run-spec cannot cross-wire the two families.
        let trade_guard_on_bars = require_registered_trade_converter_for_table_family(
            BAR_TRANSFORM_IDENTITY,
            BAR_TRANSFORM_VERSION,
            BAR_TABLE_FAMILY,
        )
        .expect_err("trade guard must reject the bar adapter kind");
        assert!(
            trade_guard_on_bars
                .to_string()
                .contains("not a CSV native-trades converter"),
            "{trade_guard_on_bars}"
        );

        let bar_guard_on_trades = require_registered_bar_converter_for_table_family(
            TRANSFORM_IDENTITY,
            TRANSFORM_VERSION,
            TRADE_TABLE_FAMILY,
        )
        .expect_err("bar guard must reject the trade adapter kind");
        assert!(
            bar_guard_on_trades
                .to_string()
                .contains("not a CSV native-bars converter"),
            "{bar_guard_on_trades}"
        );
    }

    #[test]
    fn paged_json_bar_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter(
            PAGED_JSON_BARS_TRANSFORM_IDENTITY,
            PAGED_JSON_BARS_TRANSFORM_VERSION,
        )
        .expect("registered paged-JSON bar source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::PagedJsonBars);
        assert_eq!(adapter.table_family, BAR_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_BAR);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&PAGED_JSON_BARS_ADAPTER));
        assert_eq!(
            registered_paged_json_bar_converter(
                PAGED_JSON_BARS_TRANSFORM_IDENTITY,
                PAGED_JSON_BARS_TRANSFORM_VERSION
            ),
            Some(&PAGED_JSON_BARS_ADAPTER)
        );
        // Distinct identity from the CSV bar adapter: same `bars` family, a
        // different wire shape, so the registry must not collapse the two onto
        // one entry.
        assert_ne!(PAGED_JSON_BARS_TRANSFORM_IDENTITY, BAR_TRANSFORM_IDENTITY);
    }

    #[test]
    fn paged_json_bar_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{BAR_TABLE_FAMILY}_mismatch");
        let err = require_registered_paged_json_bar_converter_for_table_family(
            PAGED_JSON_BARS_TRANSFORM_IDENTITY,
            PAGED_JSON_BARS_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("paged-JSON bar adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn jsonl_multi_interval_bar_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION,
        )
        .expect("registered JSONL multi-interval bar source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::JsonlMultiIntervalBars);
        assert_eq!(adapter.table_family, BAR_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_BAR);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&JSONL_MULTI_INTERVAL_BARS_ADAPTER));
        assert_eq!(
            registered_jsonl_multi_interval_bar_converter(
                JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
                JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION
            ),
            Some(&JSONL_MULTI_INTERVAL_BARS_ADAPTER)
        );
        // Distinct identity from the other bar adapters: each line carries its
        // own interval, so the registry must not collapse it onto a single-
        // interval bar row.
        assert_ne!(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            BAR_TRANSFORM_IDENTITY
        );
        assert_ne!(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            PAGED_JSON_BARS_TRANSFORM_IDENTITY
        );
    }

    #[test]
    fn jsonl_multi_interval_bar_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{BAR_TABLE_FAMILY}_mismatch");
        let err = require_registered_jsonl_multi_interval_bar_converter_for_table_family(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("JSONL multi-interval bar adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn paged_json_and_jsonl_bar_converter_guards_reject_other_kinds() {
        // The paged-JSON bar guard rejects the CSV-bar and JSONL-bar adapter
        // ids, and vice versa, so a run-spec cannot cross-wire the three bar
        // wire shapes onto each other.
        let paged_guard_on_csv = require_registered_paged_json_bar_converter_for_table_family(
            BAR_TRANSFORM_IDENTITY,
            BAR_TRANSFORM_VERSION,
            BAR_TABLE_FAMILY,
        )
        .expect_err("paged-JSON guard must reject the CSV bar adapter kind");
        assert!(
            paged_guard_on_csv
                .to_string()
                .contains("not a paged-JSON bar converter"),
            "{paged_guard_on_csv}"
        );

        let paged_guard_on_jsonl = require_registered_paged_json_bar_converter_for_table_family(
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_IDENTITY,
            JSONL_MULTI_INTERVAL_BARS_TRANSFORM_VERSION,
            BAR_TABLE_FAMILY,
        )
        .expect_err("paged-JSON guard must reject the JSONL bar adapter kind");
        assert!(
            paged_guard_on_jsonl
                .to_string()
                .contains("not a paged-JSON bar converter"),
            "{paged_guard_on_jsonl}"
        );

        let jsonl_guard_on_csv =
            require_registered_jsonl_multi_interval_bar_converter_for_table_family(
                BAR_TRANSFORM_IDENTITY,
                BAR_TRANSFORM_VERSION,
                BAR_TABLE_FAMILY,
            )
            .expect_err("JSONL guard must reject the CSV bar adapter kind");
        assert!(
            jsonl_guard_on_csv
                .to_string()
                .contains("not a JSONL multi-interval bar converter"),
            "{jsonl_guard_on_csv}"
        );

        let jsonl_guard_on_paged =
            require_registered_jsonl_multi_interval_bar_converter_for_table_family(
                PAGED_JSON_BARS_TRANSFORM_IDENTITY,
                PAGED_JSON_BARS_TRANSFORM_VERSION,
                BAR_TABLE_FAMILY,
            )
            .expect_err("JSONL guard must reject the paged-JSON bar adapter kind");
        assert!(
            jsonl_guard_on_paged
                .to_string()
                .contains("not a JSONL multi-interval bar converter"),
            "{jsonl_guard_on_paged}"
        );

        // And the CSV bar guard rejects both new bar adapter ids.
        let bar_guard_on_paged = require_registered_bar_converter_for_table_family(
            PAGED_JSON_BARS_TRANSFORM_IDENTITY,
            PAGED_JSON_BARS_TRANSFORM_VERSION,
            BAR_TABLE_FAMILY,
        )
        .expect_err("CSV bar guard must reject the paged-JSON bar adapter kind");
        assert!(
            bar_guard_on_paged
                .to_string()
                .contains("not a CSV native-bars converter"),
            "{bar_guard_on_paged}"
        );
    }

    #[test]
    fn order_book_delta_source_adapter_registry_exposes_data_family_metadata() {
        let adapter =
            require_registered_source_adapter(DELTAS_TRANSFORM_IDENTITY, DELTAS_TRANSFORM_VERSION)
                .expect("registered order-book-delta source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::JsonlSnapshotDeltas);
        assert_eq!(adapter.table_family, DELTAS_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&JSONL_SNAPSHOT_DELTAS_ADAPTER));
        assert_eq!(
            registered_order_book_delta_converter(
                DELTAS_TRANSFORM_IDENTITY,
                DELTAS_TRANSFORM_VERSION
            ),
            Some(&JSONL_SNAPSHOT_DELTAS_ADAPTER)
        );
    }

    #[test]
    fn order_book_delta_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{DELTAS_TABLE_FAMILY}_mismatch");
        let err = require_registered_order_book_delta_converter_for_table_family(
            DELTAS_TRANSFORM_IDENTITY,
            DELTAS_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("delta adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn order_book_delta_converter_guard_rejects_other_kinds() {
        // The delta guard rejects the trade and bar adapter ids, so a run-spec
        // cannot cross-wire the delta family onto another adapter.
        let delta_guard_on_trades = require_registered_order_book_delta_converter_for_table_family(
            TRANSFORM_IDENTITY,
            TRANSFORM_VERSION,
            TRADE_TABLE_FAMILY,
        )
        .expect_err("delta guard must reject the trade adapter kind");
        assert!(
            delta_guard_on_trades
                .to_string()
                .contains("not a JSONL snapshot-delta converter"),
            "{delta_guard_on_trades}"
        );

        let delta_guard_on_bars = require_registered_order_book_delta_converter_for_table_family(
            BAR_TRANSFORM_IDENTITY,
            BAR_TRANSFORM_VERSION,
            BAR_TABLE_FAMILY,
        )
        .expect_err("delta guard must reject the bar adapter kind");
        assert!(
            delta_guard_on_bars
                .to_string()
                .contains("not a JSONL snapshot-delta converter"),
            "{delta_guard_on_bars}"
        );
    }

    #[test]
    fn tar_order_book_delta_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter(
            TAR_DELTAS_TRANSFORM_IDENTITY,
            TAR_DELTAS_TRANSFORM_VERSION,
        )
        .expect("registered tar order-book-delta source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::TarJsonlSnapshotDeltas);
        assert_eq!(adapter.table_family, DELTAS_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER));
        assert_eq!(
            registered_tar_order_book_delta_converter(
                TAR_DELTAS_TRANSFORM_IDENTITY,
                TAR_DELTAS_TRANSFORM_VERSION
            ),
            Some(&TAR_JSONL_SNAPSHOT_DELTAS_ADAPTER)
        );
        // The tar adapter is a distinct identity from the JSONL deltas adapter:
        // same wire shape, different container, so the registry must not collapse
        // the two onto one entry.
        assert_ne!(
            TAR_DELTAS_TRANSFORM_IDENTITY, DELTAS_TRANSFORM_IDENTITY,
            "tar and JSONL delta adapters must have distinct identities"
        );
    }

    #[test]
    fn tar_order_book_delta_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{DELTAS_TABLE_FAMILY}_mismatch");
        let err = require_registered_tar_order_book_delta_converter_for_table_family(
            TAR_DELTAS_TRANSFORM_IDENTITY,
            TAR_DELTAS_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("tar delta adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn tar_order_book_delta_converter_guard_rejects_other_kinds() {
        // The tar delta guard rejects the JSONL delta, trade, and bar adapter
        // ids, so a run-spec cannot cross-wire the tar container onto another
        // adapter kind.
        let tar_guard_on_jsonl =
            require_registered_tar_order_book_delta_converter_for_table_family(
                DELTAS_TRANSFORM_IDENTITY,
                DELTAS_TRANSFORM_VERSION,
                DELTAS_TABLE_FAMILY,
            )
            .expect_err("tar delta guard must reject the JSONL delta adapter kind");
        assert!(
            tar_guard_on_jsonl
                .to_string()
                .contains("not a tar JSONL snapshot-delta converter"),
            "{tar_guard_on_jsonl}"
        );

        // And the JSONL delta guard rejects the tar adapter id, so the two
        // container entry points cannot be swapped at dispatch.
        let jsonl_guard_on_tar = require_registered_order_book_delta_converter_for_table_family(
            TAR_DELTAS_TRANSFORM_IDENTITY,
            TAR_DELTAS_TRANSFORM_VERSION,
            DELTAS_TABLE_FAMILY,
        )
        .expect_err("JSONL delta guard must reject the tar adapter kind");
        assert!(
            jsonl_guard_on_tar
                .to_string()
                .contains("not a JSONL snapshot-delta converter"),
            "{jsonl_guard_on_tar}"
        );
    }

    #[test]
    fn event_stream_delta_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter(
            EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
            EVENT_STREAM_DELTAS_TRANSFORM_VERSION,
        )
        .expect("registered event-stream delta source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::ParquetEventStreamDeltas);
        assert_eq!(adapter.table_family, DELTAS_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(adapter.nt_data_type, NT_DATA_TYPE_ORDER_BOOK_DELTA);
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&PARQUET_EVENT_STREAM_DELTAS_ADAPTER));
        assert_eq!(
            registered_event_stream_delta_converter(
                EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
                EVENT_STREAM_DELTAS_TRANSFORM_VERSION
            ),
            Some(&PARQUET_EVENT_STREAM_DELTAS_ADAPTER)
        );
        // The event-stream identity is distinct from every snapshot delta
        // identity: different wire shape and a dual-emit transform, so the
        // registry must not collapse it onto a snapshot row.
        assert_ne!(
            EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
            DELTAS_TRANSFORM_IDENTITY
        );
        assert_ne!(
            EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
            TAR_DELTAS_TRANSFORM_IDENTITY
        );
    }

    #[test]
    fn event_stream_delta_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{DELTAS_TABLE_FAMILY}_mismatch");
        let err = require_registered_event_stream_delta_converter_for_table_family(
            EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
            EVENT_STREAM_DELTAS_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("event-stream adapter table-family mismatch must fail closed");

        assert!(err.to_string().contains("adapter"), "{err}");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn event_stream_delta_converter_guard_rejects_other_kinds() {
        // The event-stream guard rejects the JSONL and tar delta adapter ids, so
        // a run-spec cannot cross-wire the dual-emit parquet container onto a
        // snapshot adapter kind.
        let guard_on_jsonl = require_registered_event_stream_delta_converter_for_table_family(
            DELTAS_TRANSFORM_IDENTITY,
            DELTAS_TRANSFORM_VERSION,
            DELTAS_TABLE_FAMILY,
        )
        .expect_err("event-stream guard must reject the JSONL delta adapter kind");
        assert!(
            guard_on_jsonl
                .to_string()
                .contains("not a Parquet event-stream delta converter"),
            "{guard_on_jsonl}"
        );

        // And the JSONL/tar delta guards reject the event-stream adapter id.
        let jsonl_guard_on_event_stream =
            require_registered_order_book_delta_converter_for_table_family(
                EVENT_STREAM_DELTAS_TRANSFORM_IDENTITY,
                EVENT_STREAM_DELTAS_TRANSFORM_VERSION,
                DELTAS_TABLE_FAMILY,
            )
            .expect_err("JSONL delta guard must reject the event-stream adapter kind");
        assert!(
            jsonl_guard_on_event_stream
                .to_string()
                .contains("not a JSONL snapshot-delta converter"),
            "{jsonl_guard_on_event_stream}"
        );
    }

    #[test]
    fn index_price_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter_for_table_family(
            INDEX_PRICES_TRANSFORM_IDENTITY,
            INDEX_PRICES_TRANSFORM_VERSION,
            INDEX_PRICES_TABLE_FAMILY,
        )
        .expect("registered index-price source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::IndexPrices);
        assert_eq!(adapter.table_family, INDEX_PRICES_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(
            adapter.nt_data_type,
            crate::catalog_projection::NT_DATA_TYPE_INDEX_PRICE_UPDATE
        );
        assert_eq!(adapter.nt_data_type, "IndexPriceUpdate");
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&INDEX_PRICES_ADAPTER));
        // The index table family is the NT `IndexPriceUpdate` catalog prefix.
        assert_eq!(INDEX_PRICES_TABLE_FAMILY, "index_prices");
    }

    #[test]
    fn index_price_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{INDEX_PRICES_TABLE_FAMILY}_mismatch");
        let err = require_registered_source_adapter_for_table_family(
            INDEX_PRICES_TRANSFORM_IDENTITY,
            INDEX_PRICES_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("index-price adapter table-family mismatch must fail closed");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn mark_price_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter_for_table_family(
            MARK_PRICES_TRANSFORM_IDENTITY,
            MARK_PRICES_TRANSFORM_VERSION,
            MARK_PRICES_TABLE_FAMILY,
        )
        .expect("registered mark-price source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::MarkPrices);
        assert_eq!(adapter.table_family, MARK_PRICES_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(
            adapter.nt_data_type,
            crate::catalog_projection::NT_DATA_TYPE_MARK_PRICE_UPDATE
        );
        assert_eq!(adapter.nt_data_type, "MarkPriceUpdate");
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&MARK_PRICES_ADAPTER));
        // The mark table family is the NT `MarkPriceUpdate` catalog prefix.
        assert_eq!(MARK_PRICES_TABLE_FAMILY, "mark_prices");
    }

    #[test]
    fn mark_price_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{MARK_PRICES_TABLE_FAMILY}_mismatch");
        let err = require_registered_source_adapter_for_table_family(
            MARK_PRICES_TRANSFORM_IDENTITY,
            MARK_PRICES_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("mark-price adapter table-family mismatch must fail closed");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    #[test]
    fn funding_rate_source_adapter_registry_exposes_data_family_metadata() {
        let adapter = require_registered_source_adapter_for_table_family(
            FUNDING_RATES_TRANSFORM_IDENTITY,
            FUNDING_RATES_TRANSFORM_VERSION,
            FUNDING_RATES_TABLE_FAMILY,
        )
        .expect("registered funding-rate source adapter");

        assert_eq!(adapter.kind, SourceAdapterKind::FundingRates);
        assert_eq!(adapter.table_family, FUNDING_RATES_TABLE_FAMILY);
        assert_eq!(adapter.normalized_schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(
            adapter.nt_data_type,
            crate::catalog_projection::NT_DATA_TYPE_FUNDING_RATE_UPDATE
        );
        assert_eq!(adapter.nt_data_type, "FundingRateUpdate");
        assert!(REGISTERED_SOURCE_ADAPTERS.contains(&FUNDING_RATES_ADAPTER));
        assert_eq!(FUNDING_RATES_TABLE_FAMILY, "funding_rates");
    }

    #[test]
    fn funding_rate_source_adapter_registry_rejects_table_family_mismatch() {
        let mismatch = format!("{FUNDING_RATES_TABLE_FAMILY}_mismatch");
        let err = require_registered_source_adapter_for_table_family(
            FUNDING_RATES_TRANSFORM_IDENTITY,
            FUNDING_RATES_TRANSFORM_VERSION,
            &mismatch,
        )
        .expect_err("funding-rate adapter table-family mismatch must fail closed");
        assert!(err.to_string().contains("table_family"), "{err}");
    }

    fn accepted_dataset() -> AcceptedDataset {
        let checks = |evidence: &str| RequiredChecks {
            source_access: RequiredCheck::passed(evidence),
            license: RequiredCheck::passed("attestation"),
            schema: RequiredCheck::passed("schema"),
            time_semantics: RequiredCheck::passed("ms_to_nanos"),
            instrument_universe: RequiredCheck::passed("universe"),
            coverage: RequiredCheck::passed(evidence),
            retention_freshness: RequiredCheck::passed("retention"),
            granularity: RequiredCheck::passed("native"),
            completeness: RequiredCheck::passed(evidence),
            nt_mapping: RequiredCheck::passed("TradeTick"),
            cost: RequiredCheck::passed("free"),
            storage: RequiredCheck::passed("artifact_root"),
        };
        let object = IngestManifestObjectRecord {
            s3_uri: "s3://bolt-parquet/.../symbol=BNBUSDC/object=d6af93.csv.gz".to_string(),
            source_url: "https://public.bybit.com/spot/BNBUSDC/BNBUSDC_2026-03-01.csv.gz"
                .to_string(),
            sha256: "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            bytes: 8505,
            archive_date: "2026-03-01".to_string(),
            schema_columns: SAMPLE_SPOT_TICK_TRADES_HEADER
                .iter()
                .map(ToString::to_string)
                .collect(),
        };
        let forbidden_claims = vec!["No execution-quality or queue-position claims.".to_string()];
        let proof = SourceProofReport {
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            contract_version: "backfill-table-contract.v1".to_string(),
            schema_version: "backfill-source-proof.v1".to_string(),
            status: SourceProofStatus::Pending,
            source_binding: "bybit-spot-tick-trades".to_string(),
            venue: "bybit".to_string(),
            product_family: "spot".to_string(),
            product_category: "spot".to_string(),
            table_family: "trades".to_string(),
            evidence_state: EvidenceState::OwnerArchiveBackfillable,
            source_candidate_class: SourceCandidateClass::OfficialFree,
            source_selection_status: SourceSelectionStatus::AcceptedLowerFidelity,
            usage_scope: SourceProofUsageScope::CanonicalBackfillInput,
            official_free_gap_ref: None,
            paid_vendor_gap_ref: None,
            fixture_type: FixtureType::PerpsSpot,
            requested_time_range: TimeRange {
                start_utc: "2025-06-01T00:00:00Z".to_string(),
                end_utc: "2026-06-01T00:00:00Z".to_string(),
            },
            coverage_time_range: TimeRange {
                start_utc: "2026-03-01T00:00:00Z".to_string(),
                end_utc: "2026-03-02T00:00:00Z".to_string(),
            },
            instrument_universe_id: "bybit-spot-instruments-2026-03-01".to_string(),
            raw_sample_uri: object.s3_uri.clone(),
            raw_sample_hash: object.sha256.clone(),
            schema_sample_uri: "s3://bolt-parquet/.../schema.json".to_string(),
            schema_sample_hash: "bf26db".to_string(),
            license_ref: "https://public.bybit.com/ (attestation)".to_string(),
            license_scope: LicenseScope::Public,
            retention_ref: "https://public.bybit.com/".to_string(),
            cost_ref: "cost://free-public-archive".to_string(),
            nt_mapping_status: NtMappingStatus::Accepted,
            fidelity_class: SourceProofFidelityClass::TradeReplay,
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
            required_checks: checks("manifest://fdcc0758"),
            acceptance_mode: None,
            accepted_by: None,
            accepted_at: None,
            supersedes_source_proof_id: None,
        }
        .accept(AcceptanceMode::Manual, "operator", "2026-06-02T00:00:00Z")
        .expect("accept");
        crate::source_proof::select_accepted_dataset(&proof, &object, &object.sha256)
            .expect("select accepted dataset")
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

    fn identity() -> CanonicalInstrumentIdentity {
        CanonicalInstrumentIdentity {
            instrument_id: "BNBUSDC".to_string(),
            venue_symbol: "BNBUSDC".to_string(),
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
        }
    }

    #[test]
    fn arrow_schema_matches_nullable_common_contract_columns() {
        let schema = CanonicalTradesTable::arrow_schema();
        assert!(
            schema
                .field_with_name("availability_time")
                .expect("availability_time field")
                .is_nullable()
        );
        assert!(
            schema
                .field_with_name("nt_instrument_id")
                .expect("nt_instrument_id field")
                .is_nullable()
        );
        assert!(
            schema
                .field_with_name("source_sequence")
                .expect("source_sequence field")
                .is_nullable()
        );
    }

    const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
        1,1772323201665,617.2,0.3,buy,0\n\
        2,1772323312219,617.9,0.1456,sell,0\n\
        3,1772323312236,617.9,0.1544,sell,0\n";

    #[test]
    fn normalizes_native_trades_with_provenance() {
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            SAMPLE_CSV,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        assert_eq!(table.rows.len(), 3);
        assert_eq!(table.schema_version, NORMALIZED_SCHEMA_VERSION);
        assert_eq!(table.partition.dt, "2026-03-01");
        let first = &table.rows[0];
        assert_eq!(first.event_time, 1_772_323_201_665 * NANOS_PER_MILLISECOND);
        assert_eq!(first.capture_time, 42);
        // ingest_run_id is the run identifier, not the source object URL.
        assert_eq!(first.ingest_run_id, "ingest-run-test");
        assert_ne!(first.ingest_run_id, accepted_dataset().object.source_url);
        assert_eq!(first.aggressor_side, "BUYER");
        assert_eq!(first.price, "617.2");
        assert_eq!(first.size, "0.3");
        assert_eq!(first.notional, "185.16");
        assert_eq!(first.canonical_instrument_key, "bybit/spot/BNBUSDC");
        assert_eq!(
            first.payload_hash,
            "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598"
        );
        assert_eq!(first.transform_hash, transform_hash());
    }

    #[test]
    fn normalizes_headerless_native_trades_using_configured_schema_columns() {
        let mut accepted = accepted_dataset();
        accepted.object.schema_columns = [
            "trade_id",
            "price",
            "qty",
            "quote_qty",
            "time",
            "is_buyer_maker",
            "is_best_match",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect();
        let mapping = CsvTradeMappingConfig {
            has_headers: false,
            trade_id_column: "trade_id".to_string(),
            timestamp_column: "time".to_string(),
            timestamp_unit: CsvTimestampUnit::Microseconds,
            price_column: "price".to_string(),
            size_column: "qty".to_string(),
            side_column: "is_buyer_maker".to_string(),
            buyer_side_values: vec!["False".to_string()],
            seller_side_values: vec!["True".to_string()],
        };
        let csv = "101735393,617.34000000,1.61900000,999.47346000,1772323201711256,True,True\n\
            101735394,617.34000000,0.07200000,44.44848000,1772323201815330,False,True\n";

        let table = normalize_csv_native_trades(
            &accepted,
            &identity(),
            &mapping,
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize headerless csv");

        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0].trade_id, "101735393");
        assert_eq!(table.rows[0].event_time, 1_772_323_201_711_256_000);
        assert_eq!(table.rows[0].aggressor_side, "SELLER");
        assert_eq!(table.rows[0].price, "617.34000000");
        assert_eq!(table.rows[0].size, "1.61900000");
        assert_eq!(table.rows[1].aggressor_side, "BUYER");
    }

    #[test]
    fn normalizes_bybit_derivative_trades_with_decimal_second_timestamps() {
        let mut accepted = accepted_dataset();
        accepted.source_binding = "bybit-linear-tick-trades".to_string();
        accepted.product_family = "linear".to_string();
        accepted.product_category = "linear".to_string();
        accepted.object.source_url = "https://public.bybit.com/trading/1000000BABYDOGEUSDT/1000000BABYDOGEUSDT2025-06-01.csv.gz".to_string();
        accepted.object.archive_date = "2025-06-01".to_string();
        accepted.object.schema_columns = [
            "timestamp",
            "symbol",
            "side",
            "size",
            "price",
            "tickDirection",
            "trdMatchID",
            "grossValue",
            "homeNotional",
            "foreignNotional",
            "RPI",
        ]
        .into_iter()
        .map(ToString::to_string)
        .collect();
        let identity = CanonicalInstrumentIdentity {
            instrument_id: "1000000BABYDOGEUSDT".to_string(),
            venue_symbol: "1000000BABYDOGEUSDT".to_string(),
            nt_instrument_id: "1000000BABYDOGEUSDT.BYBIT".to_string(),
        };
        let mapping = CsvTradeMappingConfig {
            has_headers: true,
            trade_id_column: "trdMatchID".to_string(),
            timestamp_column: "timestamp".to_string(),
            timestamp_unit: CsvTimestampUnit::DecimalSeconds,
            price_column: "price".to_string(),
            size_column: "size".to_string(),
            side_column: "side".to_string(),
            buyer_side_values: vec!["Buy".to_string()],
            seller_side_values: vec!["Sell".to_string()],
        };
        let csv = "timestamp,symbol,side,size,price,tickDirection,trdMatchID,grossValue,homeNotional,foreignNotional,RPI\n\
            1748736005.0613,1000000BABYDOGEUSDT,Sell,1000,0.0013924,MinusTick,b765b98b-cd17-575a-85ed-69204ad090f2,1.3924e+08,1000,1.3924,0\n\
            1748736005.061300001,1000000BABYDOGEUSDT,Buy,4100,0.0013919,MinusTick,3bd2f40d-3490-5cae-93c0-cb7aba96b834,5.70679e+08,4100,5.70679,0\n";

        let table =
            normalize_csv_native_trades(&accepted, &identity, &mapping, csv, 42, "ingest-run-test")
                .expect("normalize Bybit derivative csv");

        assert_eq!(table.partition.product_family, "linear");
        assert_eq!(table.partition.dt, "2025-06-01");
        assert_eq!(table.rows.len(), 2);
        assert_eq!(
            table.rows[0].trade_id,
            "b765b98b-cd17-575a-85ed-69204ad090f2"
        );
        assert_eq!(table.rows[0].event_time, 1_748_736_005_061_300_000);
        assert_eq!(table.rows[0].aggressor_side, "SELLER");
        assert_eq!(table.rows[0].size, "1000");
        assert_eq!(table.rows[0].notional, "1.3924");
        assert_eq!(table.rows[1].event_time, 1_748_736_005_061_300_001);
        assert_eq!(table.rows[1].aggressor_side, "BUYER");
        assert_eq!(
            table.rows[0].canonical_instrument_key,
            "bybit/linear/1000000BABYDOGEUSDT"
        );
    }

    #[test]
    fn decimal_second_timestamp_unit_is_config_owned() {
        let mapping: CsvTradeMappingConfig = toml::from_str(
            r#"
has_headers = true
trade_id_column = "trdMatchID"
timestamp_column = "timestamp"
timestamp_unit = "decimal_seconds"
price_column = "price"
size_column = "size"
side_column = "side"
buyer_side_values = ["Buy"]
seller_side_values = ["Sell"]
"#,
        )
        .expect("mapping parses from TOML");

        assert_eq!(mapping.timestamp_unit, CsvTimestampUnit::DecimalSeconds);
    }

    #[test]
    fn parses_quoted_csv_fields_without_shifting_columns() {
        let csv = "id,timestamp,price,volume,side,rpi\n\
            1,1772323201665,617.2,0.3,buy,\"ignored,quoted\"\n";
        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            csv,
            42,
            "ingest-run-test",
        )
        .expect("normalize quoted csv");
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].price, "617.2");
        assert_eq!(table.rows[0].size, "0.3");
        assert_eq!(table.rows[0].aggressor_side, "BUYER");
    }

    #[test]
    fn rejects_header_mismatch() {
        let bad = "id,ts,price,volume,side,rpi\n1,1,1,1,buy,0\n";
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            bad,
            0,
            "ingest-run-test",
        )
        .unwrap_err();
        assert!(err.to_string().contains("header"), "{err}");
    }

    #[test]
    fn rejects_unknown_side() {
        let bad = "id,timestamp,price,volume,side,rpi\n1,1772323201665,617.2,0.3,hold,0\n";
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            bad,
            0,
            "ingest-run-test",
        )
        .unwrap_err();
        assert!(err.to_string().contains("side"), "{err}");
    }

    #[test]
    fn rejects_non_monotonic_event_time() {
        let bad = "id,timestamp,price,volume,side,rpi\n\
            1,1772323312219,617.2,0.3,buy,0\n\
            2,1772323201665,617.9,0.1,sell,0\n";
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            bad,
            0,
            "ingest-run-test",
        )
        .unwrap_err();
        assert!(err.to_string().contains("precedes previous"), "{err}");
    }

    #[test]
    fn rejects_non_positive_price() {
        let bad = "id,timestamp,price,volume,side,rpi\n1,1772323201665,0,0.3,buy,0\n";
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            bad,
            0,
            "ingest-run-test",
        )
        .unwrap_err();
        assert!(err.to_string().contains("price"), "{err}");
    }

    #[test]
    fn rejects_notional_overflow() {
        // price * size that overflows Decimal must fail loud (error), mirroring
        // the checked timestamp arithmetic on the same row — never panic.
        let huge = "79228162514264337593543950335"; // Decimal::MAX
        let csv = format!("id,timestamp,price,volume,side,rpi\n1,1772323201665,{huge},2,buy,0\n");
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            &csv,
            0,
            "ingest-run-test",
        )
        .unwrap_err();
        assert!(err.to_string().contains("notional"), "{err}");
    }

    #[test]
    fn rejects_empty_ingest_run_id() {
        let err = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            SAMPLE_CSV,
            42,
            "  ",
        )
        .unwrap_err();
        assert!(err.to_string().contains("ingest_run_id"), "{err}");
    }

    #[test]
    fn writes_and_reads_back_canonical_parquet() {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

        let table = normalize_sample_spot_tick_trades(
            &accepted_dataset(),
            &identity(),
            SAMPLE_CSV,
            42,
            "ingest-run-test",
        )
        .expect("normalize");
        let dir = tempfile::TempDir::new().expect("temp dir");
        let path = dir.path().join("trades.parquet");
        table.write_parquet(&path).expect("write parquet");

        let file = File::open(&path).expect("open parquet");
        let reader = ParquetRecordBatchReaderBuilder::try_new(file)
            .expect("reader builder")
            .build()
            .expect("reader");
        let mut total_rows = 0;
        for batch in reader {
            total_rows += batch.expect("batch").num_rows();
        }
        assert_eq!(total_rows, table.rows.len());
    }
}
