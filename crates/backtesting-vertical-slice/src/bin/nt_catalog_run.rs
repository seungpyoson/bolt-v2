//! Driver: convert staged venue data into a NautilusTrader `ParquetDataCatalog`.
//!
//! The catalog target is any `object_store` URI — an `s3://bucket/prefix` lake
//! or a local path — resolved through NautilusTrader's own
//! [`ParquetDataCatalog::from_uri`], so the same converters that round-trip in
//! the test suite write straight into a cloud catalog with no extra plumbing.
//!
//! Two subcommands:
//! * `smoke` proves the catalog's object-store write + read-back path works end
//!   to end against the real target before a full run. It is a connectivity /
//!   credentials probe, not a data conversion.
//! * `convert` lists staged objects under a source URI, converts every object of
//!   a wired `(venue, family)` into NautilusTrader-native data with the crate's
//!   own converters, and appends it into the catalog. Source listing and reads
//!   go through `object_store` (the same backend NautilusTrader uses), so a
//!   `file://` staging tree and an `s3://` lake share one code path.

use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use ahash::AHashMap;
use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use futures::StreamExt;
use object_store::{ObjectStore, ObjectStoreExt, path::Path as ObjectPath};
use url::Url;

use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::AggressorSide,
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

use backtesting_vertical_slice::canonical_binance::{
    append_binance_futures_agg_trades_archive, append_binance_futures_mark_price_klines_archive,
    kline_bar_spec_from_object_key,
};
use backtesting_vertical_slice::canonical_book::{
    append_polymarket_book_archive, append_polymarket_trades_archive,
};
use backtesting_vertical_slice::canonical_bybit::{
    append_bybit_deriv_tick_trades_archive, append_bybit_mark_price_kline_1m_archive,
    append_bybit_mark_price_kline_1m_batch,
};
use backtesting_vertical_slice::canonical_chainlink::append_chainlink_index_prices_archive;
use backtesting_vertical_slice::canonical_deribit::{
    append_deribit_bars_archive, append_deribit_trades_archive,
};
use backtesting_vertical_slice::canonical_hyperliquid_core::append_hyperliquid_core_fills_archive;
use backtesting_vertical_slice::canonical_hyperliquid_hip3::append_hyperliquid_hip3_bars_archive;
use backtesting_vertical_slice::canonical_hyperliquid_hip4::{
    append_hip4_bars_archive, append_hip4_snapshots_archive, append_hip4_trades_archive,
    hip4_canonical_naming,
};
use backtesting_vertical_slice::canonical_okx::{
    append_okx_book_archive, append_okx_candlesticks_archive, append_okx_trades_archive,
    extract_csv_from_zip,
};

/// Default region for the `bolt-parquet` data lake (a bucket with no location
/// constraint resolves to `us-east-1`).
const DEFAULT_REGION: &str = "us-east-1";

/// Synthetic instrument id used only by the `smoke` connectivity probe.
const SMOKE_INSTRUMENT_ID: &str = "SMOKE.TEST";

/// A `(venue, family)` the bulk converter knows how to project, and how its
/// objects are found under the bucket root.
struct FamilyBinding {
    /// Dispatch venue key — the `--venue` filter value, e.g. `okx`.
    venue: &'static str,
    /// Dispatch family key — the `--family` filter value, e.g. `trades`.
    family: &'static str,
    /// Key prefix under the bucket root to list recursively. A `{date}`
    /// placeholder is filled from `--staging-date` (Polymarket's prefix carries
    /// no date and is left untouched).
    root_template: &'static str,
    /// Required key segments — an object converts only if its key contains
    /// every entry (empty = no filter). Used where one root holds several
    /// families or product types: Deribit interleaves `trades` and `bars_1m`
    /// across many `run=` partitions that cannot be enumerated statically, and
    /// mixes option / future / perpetual products the option-only converter must
    /// not be fed.
    key_filters: &'static [&'static str],
    /// Suffix the family's archive objects carry, e.g. `.zip`; everything else
    /// under the prefix (manifests, checksums, sidecars) is skipped.
    extension: &'static str,
}

/// The wired conversion bindings, resolved against the live `bolt-parquet`
/// layout. New venues/families are added here as their append paths land; the
/// dispatch in [`convert_object`] must gain a matching arm. Deribit
/// `tardis_options_chain` (NautilusTrader Tardis loader) and the second targeted
/// HL-core fills folder are intentionally not wired here.
const FAMILY_BINDINGS: &[FamilyBinding] = &[
    FamilyBinding {
        venue: "okx",
        family: "trades",
        root_template: "backfill-staging/{date}/okx/raw/v1/family=trades/",
        key_filters: &[],
        extension: ".zip",
    },
    FamilyBinding {
        venue: "okx",
        family: "candlesticks",
        root_template: "backfill-staging/{date}/okx/raw/v1/family=candlesticks/",
        key_filters: &[],
        extension: ".zip",
    },
    FamilyBinding {
        venue: "okx",
        family: "order_book_400",
        root_template: "backfill-staging/{date}/okx/raw/v1/family=order_book_400/",
        key_filters: &[],
        extension: ".gz",
    },
    FamilyBinding {
        venue: "binance",
        family: "aggTrades",
        root_template: "backfill-staging/{date}/binance/raw/v1/source=data.binance.vision/product=futures_um/",
        key_filters: &["/family=aggTrades/"],
        extension: ".zip",
    },
    FamilyBinding {
        venue: "binance",
        family: "markPriceKlines",
        root_template: "backfill-staging/{date}/binance/raw/v1/source=data.binance.vision/product=futures_um/",
        key_filters: &["/family=markPriceKlines/"],
        extension: ".zip",
    },
    // Bybit tick trades: the converter parses the derivatives archive header
    // (linear + inverse share it); spot uses a different header and is not wired.
    // One binding per derivative category keeps the prefix the scope.
    FamilyBinding {
        venue: "bybit",
        family: "tick_trades",
        root_template: "backfill-staging/{date}/bybit/raw/v1/source=public_archive/family=tick_trades/category=linear/",
        key_filters: &[],
        extension: ".csv.gz",
    },
    FamilyBinding {
        venue: "bybit",
        family: "tick_trades",
        root_template: "backfill-staging/{date}/bybit/raw/v1/source=public_archive/family=tick_trades/category=inverse/",
        key_filters: &[],
        extension: ".csv.gz",
    },
    FamilyBinding {
        venue: "bybit",
        family: "mark_price_kline_1m",
        root_template: "backfill-staging/{date}/bybit/raw/v1/source=rest/family=mark_price_kline_1m/",
        key_filters: &[],
        extension: ".json",
    },
    // Deribit `trades` is intentionally NOT wired: the staged `/family=trades/`
    // objects in this layout are raw Deribit REST envelopes
    // (`{"result":{"trades":[],"has_more":false}}`) and are uniformly empty (≈135
    // bytes), while `append_deribit_trades_archive` reads the RiveChen merged-
    // trades Parquet — a different source not staged in this snapshot. The
    // converter + its round-trip test are kept (and the `convert_object` arm) for
    // when that Parquet source is staged; only the binding is omitted so a full
    // run does not list+read thousands of empty REST objects with no matching
    // converter.
    FamilyBinding {
        venue: "deribit",
        family: "bars_1m",
        root_template: "backfill-staging/{date}/deribit/raw/v1/",
        // Options only (see the trades binding): futures/perpetuals are excluded.
        key_filters: &["/family=bars_1m/", "/product_family=option/"],
        extension: ".json",
    },
    FamilyBinding {
        venue: "chainlink",
        family: "btc",
        root_template: "backfill-staging/{date}/chainlink/btc-5m-cycles/",
        key_filters: &[],
        extension: ".parquet",
    },
    FamilyBinding {
        venue: "chainlink",
        family: "eth",
        root_template: "backfill-staging/{date}/chainlink/eth-5m-cycles/",
        key_filters: &[],
        extension: ".parquet",
    },
    FamilyBinding {
        venue: "chainlink",
        family: "sol",
        root_template: "backfill-staging/{date}/chainlink/sol-5m-cycles/",
        key_filters: &[],
        extension: ".parquet",
    },
    FamilyBinding {
        venue: "chainlink",
        family: "xrp",
        root_template: "backfill-staging/{date}/chainlink/xrp-5m-cycles/",
        key_filters: &[],
        extension: ".parquet",
    },
    FamilyBinding {
        venue: "hyperliquid-core",
        family: "node_fills_by_block",
        root_template: "backfill-staging/{date}/hyperliquid-core/raw/v1/source_family=node_fills_by_block/",
        key_filters: &[],
        extension: ".lz4",
    },
    FamilyBinding {
        venue: "hyperliquid-hip3",
        family: "bars",
        root_template: "backfill-staging/{date}/hyperliquid-hip3/staged/v1/table=bars/",
        key_filters: &[],
        extension: ".jsonl",
    },
    FamilyBinding {
        venue: "hyperliquid-hip4",
        family: "trades_recent",
        root_template: "backfill-staging/{date}/hyperliquid-hip4/staged/v1/table=trades_recent/",
        key_filters: &[],
        extension: ".jsonl",
    },
    FamilyBinding {
        venue: "hyperliquid-hip4",
        family: "bars",
        root_template: "backfill-staging/{date}/hyperliquid-hip4/staged/v1/table=bars/",
        key_filters: &[],
        extension: ".jsonl",
    },
    FamilyBinding {
        venue: "hyperliquid-hip4",
        family: "order_book_snapshots_fixed_depth",
        root_template: "backfill-staging/{date}/hyperliquid-hip4/staged/v1/table=order_book_snapshots_fixed_depth/",
        key_filters: &[],
        extension: ".jsonl",
    },
    // Polymarket CLOB: one unified `order_book_snapshots_fixed_depth` stream per
    // staged prefix (book snapshots + price_change deltas + last_trade_price
    // prints, discriminated by `event_type`). The accepted backfill spans two
    // prefixes (`-streaming` and `-page1`); both share this schema. The book
    // converter emits BOTH OrderBookDelta and TradeTick from the unified stream,
    // so no separate `trades` binding is wired (no separate trades archive
    // exists). `{date}` is the staging snapshot date; each object's own `dt=`
    // partition is listed recursively under the family prefix.
    FamilyBinding {
        venue: "polymarket",
        family: "book",
        root_template: "backfill-staging/{date}/polymarket-pmxt-v2-streaming/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/",
        key_filters: &[],
        extension: ".parquet",
    },
    FamilyBinding {
        venue: "polymarket",
        family: "book",
        root_template: "backfill-staging/{date}/polymarket-pmxt-v2-page1/raw/v1/source_binding=polymarket-parquet-archive-index/fixture=prediction-market/family=order_book_snapshots_fixed_depth/",
        key_filters: &[],
        extension: ".parquet",
    },
];

/// One instrument's write outcome, venue-agnostic so the coverage report does
/// not depend on any single venue's summary type.
struct ConvertedInstrument {
    nt_instrument_id: String,
    record_count: usize,
}

#[derive(Parser)]
#[command(about = "Convert staged venue data into a NautilusTrader catalog (S3 or local).")]
struct Cli {
    /// Catalog base URI: `s3://bucket/prefix` (cloud) or a local filesystem path.
    #[arg(long)]
    catalog_uri: String,
    /// Object-store region for an S3 target. Defaults to `AWS_REGION`, then
    /// `us-east-1`.
    #[arg(long)]
    region: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a handful of synthetic `TradeTick`s and read them back, proving the
    /// catalog's object-store write/read path works end to end.
    Smoke,
    /// Convert staged objects under a source URI into the catalog.
    Convert {
        /// Staging root URI: the bucket root `s3://bucket/` (or a `file:///abs/`
        /// mirror root). Family prefixes are resolved relative to it.
        #[arg(long)]
        staging_uri: String,
        /// Backfill snapshot date that fills the `{date}` segment of the staging
        /// layout (for example `2026-06-01`). A runtime value, never hardcoded.
        #[arg(long)]
        staging_date: String,
        /// Identifier stamped as this conversion run's provenance on venues that
        /// record it (for example Binance). One value per convert invocation.
        #[arg(long)]
        ingest_run_id: String,
        /// Optional venue filter (for example `okx`); absent converts all wired
        /// venues found under the staging root.
        #[arg(long)]
        venue: Option<String>,
        /// Optional family filter (for example `trades`).
        #[arg(long)]
        family: Option<String>,
    },
}

/// Whether a URI targets a remote object store (so credentials/region options
/// apply). A plain path or a `file://` URI is local; those backends take no
/// storage options.
fn is_remote(uri: &str) -> bool {
    match uri.split_once("://") {
        Some((scheme, _)) => !scheme.eq_ignore_ascii_case("file"),
        None => false,
    }
}

/// Build object-store storage options from `--region` and the standard AWS
/// environment.
///
/// Region resolves from `--region`, then `AWS_REGION`, then [`DEFAULT_REGION`].
/// Static credentials are read from the standard AWS environment variables when
/// present and forwarded to the object store; the values are never logged. On an
/// EC2 instance role no static credentials are set, and `object_store` resolves
/// them via the instance metadata service.
fn storage_options(region: Option<String>) -> AHashMap<String, String> {
    let mut opts = AHashMap::new();
    let region = region
        .or_else(|| std::env::var("AWS_REGION").ok())
        .unwrap_or_else(|| DEFAULT_REGION.to_string());
    opts.insert("region".to_string(), region);
    if let Ok(key) = std::env::var("AWS_ACCESS_KEY_ID") {
        opts.insert("access_key_id".to_string(), key);
    }
    if let Ok(secret) = std::env::var("AWS_SECRET_ACCESS_KEY") {
        opts.insert("secret_access_key".to_string(), secret);
    }
    if let Ok(token) = std::env::var("AWS_SESSION_TOKEN") {
        opts.insert("session_token".to_string(), token);
    }
    opts
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let opts = storage_options(cli.region);
    match cli.command {
        Command::Smoke => smoke(&cli.catalog_uri, opts),
        Command::Convert {
            staging_uri,
            staging_date,
            ingest_run_id,
            venue,
            family,
        } => convert(
            &cli.catalog_uri,
            opts,
            &staging_uri,
            &staging_date,
            &ingest_run_id,
            venue.as_deref(),
            family.as_deref(),
        ),
    }
}

/// Prove the catalog's object-store write + read-back path end to end.
fn smoke(catalog_uri: &str, opts: AHashMap<String, String>) -> Result<()> {
    let instrument_id = InstrumentId::from(SMOKE_INSTRUMENT_ID);
    let base_ts: u64 = 1_700_000_000_000_000_000;
    let ticks: Vec<TradeTick> = (0..5u64)
        .map(|i| {
            TradeTick::new(
                instrument_id,
                Price::from_str("100.00").expect("valid price"),
                Quantity::from_str("1").expect("valid size"),
                AggressorSide::Buyer,
                TradeId::from(format!("smoke-{i}").as_str()),
                UnixNanos::from(base_ts + i),
                UnixNanos::from(base_ts + i),
            )
        })
        .collect();
    let expected = ticks.len();

    let catalog_opts = is_remote(catalog_uri).then(|| opts.clone());
    let mut catalog = ParquetDataCatalog::from_uri(catalog_uri, catalog_opts, None, None, None)
        .context("construct catalog from uri")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write trade ticks to catalog")?;

    let read = catalog
        .query_typed_data::<TradeTick>(
            Some(vec![SMOKE_INSTRUMENT_ID.to_string()]),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query trade ticks back")?;

    println!(
        "smoke: wrote {expected} ticks, read back {} from {catalog_uri}",
        read.len()
    );
    ensure!(
        read.len() == expected,
        "catalog round trip mismatch: wrote {expected}, read {}",
        read.len()
    );
    println!("SMOKE PASS");
    Ok(())
}

/// Convert staged objects under `staging_uri` into the catalog at `catalog_uri`.
///
/// Lists each wired family's objects through `object_store`, then for every
/// object reads its bytes and appends it to the catalog with the venue's own
/// converter. Async IO (list/get) is driven on a runtime and completes before
/// each synchronous catalog write, so NautilusTrader's own blocking write never
/// nests inside the runtime.
#[allow(clippy::too_many_arguments)]
fn convert(
    catalog_uri: &str,
    opts: AHashMap<String, String>,
    staging_uri: &str,
    staging_date: &str,
    ingest_run_id: &str,
    venue: Option<&str>,
    family: Option<&str>,
) -> Result<()> {
    let catalog_opts = is_remote(catalog_uri).then(|| opts.clone());
    let mut catalog = ParquetDataCatalog::from_uri(catalog_uri, catalog_opts, None, None, None)
        .context("open catalog from uri")?;

    let base_url = Url::parse(staging_uri).with_context(|| {
        format!("parse staging uri {staging_uri:?} (need a file:// or s3:// URI)")
    })?;
    let source_opts = if is_remote(staging_uri) {
        opts
    } else {
        AHashMap::new()
    };
    let (store, base_path) = object_store::parse_url_opts(&base_url, source_opts)
        .with_context(|| format!("open object store for {staging_uri:?}"))?;
    let store: Arc<dyn ObjectStore> = Arc::from(store);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for object-store IO")?;

    let mut total_objects = 0usize;
    let mut total_records = 0usize;
    let mut total_instruments = 0usize;
    let mut matched_bindings = 0usize;

    for binding in FAMILY_BINDINGS {
        if venue.is_some_and(|v| v != binding.venue) {
            continue;
        }
        if family.is_some_and(|f| f != binding.family) {
            continue;
        }
        matched_bindings += 1;

        let resolved = binding.root_template.replace("{date}", staging_date);
        let base = base_path.as_ref().trim_end_matches('/');
        let joined = if base.is_empty() {
            resolved.trim_start_matches('/').to_string()
        } else {
            format!("{}/{}", base, resolved.trim_matches('/'))
        };
        let prefix = ObjectPath::from(joined);
        let keys = runtime
            .block_on(list_objects(
                store.clone(),
                prefix.clone(),
                binding.extension,
                binding.key_filters,
            ))
            .with_context(|| format!("list {prefix}"))?;

        let mut objects = 0usize;
        let mut records = 0usize;
        let mut instruments: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        if binding.venue == "bybit" && binding.family == "mark_price_kline_1m" {
            // Bybit mark-price REST pages overlap in time per instrument, so they
            // must be deduplicated across objects before writing — one disjoint
            // catalog write per instrument, not one (conflicting) file per page.
            // Collect the whole binding, then batch-convert.
            let mut batch = Vec::with_capacity(keys.len());
            for key in &keys {
                let bytes = runtime
                    .block_on(get_bytes(store.clone(), key.clone()))
                    .with_context(|| format!("read {key}"))?;
                batch.push((key.to_string(), bytes));
            }
            objects = batch.len();
            let converted = append_bybit_mark_price_kline_1m_batch(&batch, &mut catalog)
                .with_context(|| format!("convert bybit mark-price batch ({objects} objects)"))?;
            for instrument in converted {
                records += instrument.record_count;
                instruments.insert(instrument.nt_instrument_id);
            }
        } else {
            for key in &keys {
                let bytes = runtime
                    .block_on(get_bytes(store.clone(), key.clone()))
                    .with_context(|| format!("read {key}"))?;
                let converted =
                    convert_object(binding, key.as_ref(), &bytes, ingest_run_id, &mut catalog)
                        .with_context(|| format!("convert {key}"))?;
                objects += 1;
                for instrument in converted {
                    records += instrument.record_count;
                    instruments.insert(instrument.nt_instrument_id);
                }
            }
        }

        println!(
            "[{}/{}] objects_seen={} objects_converted={} instruments={} records_written={}",
            binding.venue,
            binding.family,
            keys.len(),
            objects,
            instruments.len(),
            records,
        );

        total_objects += objects;
        total_records += records;
        total_instruments += instruments.len();
    }

    ensure!(
        matched_bindings > 0,
        "no wired (venue, family) binding matched the requested filters (venue={venue:?}, family={family:?})"
    );

    println!(
        "CONVERT DONE: bindings={matched_bindings} objects={total_objects} instruments={total_instruments} records={total_records} -> {catalog_uri}"
    );
    Ok(())
}

/// A staged object's bytes materialised to a temporary local file, removed when
/// dropped. Polymarket's converters read Parquet through the synchronous Arrow
/// file reader (a filesystem path), so an `s3://` object is staged locally for
/// the duration of one append.
struct TempObject {
    path: PathBuf,
}

impl TempObject {
    fn new(object_key: &str, bytes: &[u8]) -> Result<Self> {
        // Hash the full object key into a short, deterministic, collision-free
        // local name. Flattening the key (replacing separators) can exceed the
        // 255-byte single-component filesystem limit for deep accepted-archive
        // keys (ENAMETOOLONG); the Arrow reader opens by file handle, so no real
        // extension is required.
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        object_key.hash(&mut hasher);
        let path =
            std::env::temp_dir().join(format!("nt-convert-{:016x}.parquet", hasher.finish()));
        fs::write(&path, bytes)
            .with_context(|| format!("stage object to temp file {}", path.display()))?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempObject {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Decode an object's bytes as UTF-8 text for the converters that take `&str`.
fn as_text(bytes: &[u8], object_key: &str) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .with_context(|| format!("object {object_key} is not valid UTF-8"))
}

/// Build the venue-agnostic coverage row from a summary's id + record count.
fn instrument(nt_instrument_id: String, record_count: usize) -> ConvertedInstrument {
    ConvertedInstrument {
        nt_instrument_id,
        record_count,
    }
}

/// Dispatch one staged object to its venue/family converter, appending into the
/// shared catalog. Each arm adapts the object bytes to the converter's input
/// form (raw bytes, decompressed CSV text, UTF-8 JSONL, or a temp file) and maps
/// the venue summary onto the agnostic coverage row. New `(venue, family)` arms
/// are added as their append paths land.
fn convert_object(
    binding: &FamilyBinding,
    object_key: &str,
    bytes: &[u8],
    ingest_run_id: &str,
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<ConvertedInstrument>> {
    let rows = match (binding.venue, binding.family) {
        ("okx", "trades") => append_okx_trades_archive(bytes, catalog)?
            .into_iter()
            .map(|s| instrument(s.nt_instrument_id, s.record_count))
            .collect(),
        ("okx", "candlesticks") => append_okx_candlesticks_archive(bytes, catalog)?
            .into_iter()
            .map(|s| instrument(s.nt_instrument_id, s.record_count))
            .collect(),
        ("okx", "order_book_400") => append_okx_book_archive(bytes, catalog)?
            .into_iter()
            .map(|s| instrument(s.nt_instrument_id, s.record_count))
            .collect(),
        ("binance", "aggTrades") => {
            let csv = extract_csv_from_zip(bytes)?;
            let s = append_binance_futures_agg_trades_archive(
                &csv,
                object_key,
                ingest_run_id,
                catalog,
            )?;
            vec![instrument(s.nt_instrument_id, s.record_count)]
        }
        ("binance", "markPriceKlines") => {
            let csv = extract_csv_from_zip(bytes)?;
            let bar_spec = kline_bar_spec_from_object_key(object_key)?;
            let s = append_binance_futures_mark_price_klines_archive(
                &csv,
                object_key,
                ingest_run_id,
                bar_spec,
                catalog,
            )?;
            vec![instrument(s.nt_instrument_id, s.record_count)]
        }
        ("bybit", "tick_trades") => {
            append_bybit_deriv_tick_trades_archive(bytes, object_key, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.record_count))
                .collect()
        }
        ("bybit", "mark_price_kline_1m") => {
            append_bybit_mark_price_kline_1m_archive(bytes, object_key, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.record_count))
                .collect()
        }
        ("deribit", "trades") => append_deribit_trades_archive(bytes, catalog)?
            .into_iter()
            .map(|s| instrument(s.nt_instrument_id, s.record_count))
            .collect(),
        ("deribit", "bars_1m") => {
            let s = append_deribit_bars_archive(bytes, object_key, catalog)?;
            vec![instrument(s.nt_instrument_id, s.record_count)]
        }
        ("chainlink", _) => {
            let s = append_chainlink_index_prices_archive(bytes, catalog)?;
            vec![instrument(s.nt_instrument_id, s.record_count)]
        }
        ("hyperliquid-core", "node_fills_by_block") => {
            append_hyperliquid_core_fills_archive(bytes, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.record_count))
                .collect()
        }
        ("hyperliquid-hip3", "bars") => {
            append_hyperliquid_hip3_bars_archive(bytes, object_key, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.record_count))
                .collect()
        }
        ("hyperliquid-hip4", "trades_recent") => append_hip4_trades_archive(
            &as_text(bytes, object_key)?,
            &hip4_canonical_naming(),
            catalog,
        )?
        .into_iter()
        .map(|s| instrument(s.nt_identifier, s.record_count))
        .collect(),
        ("hyperliquid-hip4", "bars") => append_hip4_bars_archive(
            &as_text(bytes, object_key)?,
            &hip4_canonical_naming(),
            catalog,
        )?
        .into_iter()
        .map(|s| instrument(s.nt_identifier, s.record_count))
        .collect(),
        ("hyperliquid-hip4", "order_book_snapshots_fixed_depth") => append_hip4_snapshots_archive(
            &as_text(bytes, object_key)?,
            &hip4_canonical_naming(),
            catalog,
        )?
        .into_iter()
        .map(|s| instrument(s.nt_identifier, s.record_count))
        .collect(),
        ("polymarket", "book") => {
            let tmp = TempObject::new(object_key, bytes)?;
            append_polymarket_book_archive(tmp.path(), object_key, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.delta_count + s.trade_count))
                .collect()
        }
        ("polymarket", "trades") => {
            let tmp = TempObject::new(object_key, bytes)?;
            append_polymarket_trades_archive(tmp.path(), object_key, catalog)?
                .into_iter()
                .map(|s| instrument(s.nt_instrument_id, s.delta_count + s.trade_count))
                .collect()
        }
        (venue, family) => bail!("no converter wired for venue={venue} family={family}"),
    };
    Ok(rows)
}

/// List every object under `prefix` whose key ends with `extension` and contains
/// every entry in `key_filters` (empty = no segment filter), in deterministic
/// key order. Manifests, checksums, and other sidecars under the same prefix are
/// skipped by the extension test.
async fn list_objects(
    store: Arc<dyn ObjectStore>,
    prefix: ObjectPath,
    extension: &str,
    key_filters: &[&str],
) -> Result<Vec<ObjectPath>> {
    let mut stream = store.list(Some(&prefix));
    let mut keys = Vec::new();
    while let Some(meta) = stream.next().await {
        let meta = meta.context("list object metadata")?;
        let location = meta.location;
        let key = location.as_ref();
        if !key.ends_with(extension) {
            continue;
        }
        if !key_filters.iter().all(|seg| key.contains(seg)) {
            continue;
        }
        keys.push(location);
    }
    keys.sort();
    Ok(keys)
}

/// Read one object's bytes.
async fn get_bytes(store: Arc<dyn ObjectStore>, key: ObjectPath) -> Result<Vec<u8>> {
    let result = store.get(&key).await.context("get object")?;
    let bytes = result.bytes().await.context("read object bytes")?;
    Ok(bytes.to_vec())
}
