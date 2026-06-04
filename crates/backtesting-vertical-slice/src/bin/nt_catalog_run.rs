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

use backtesting_vertical_slice::canonical_okx::append_okx_trades_archive;

/// Default region for the `bolt-parquet` data lake (a bucket with no location
/// constraint resolves to `us-east-1`).
const DEFAULT_REGION: &str = "us-east-1";

/// Synthetic instrument id used only by the `smoke` connectivity probe.
const SMOKE_INSTRUMENT_ID: &str = "SMOKE.TEST";

/// A `(venue, family)` the bulk converter knows how to project, and the staging
/// sub-prefix (relative to the source root) its objects live under.
struct FamilyBinding {
    venue: &'static str,
    family: &'static str,
    /// Path under the staging root, for example
    /// `okx/raw/v1/family=trades/`. Objects below it are the family's archives.
    sub_prefix: &'static str,
}

/// The wired conversion bindings. New venues/families are added here as their
/// append paths land; the dispatch in [`convert_object`] must gain a matching
/// arm.
const FAMILY_BINDINGS: &[FamilyBinding] = &[FamilyBinding {
    venue: "okx",
    family: "trades",
    sub_prefix: "okx/raw/v1/family=trades/",
}];

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
        /// Staging root URI: `s3://bucket/prefix/` or `file:///abs/path/`.
        #[arg(long)]
        staging_uri: String,
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
            venue,
            family,
        } => convert(
            &cli.catalog_uri,
            opts,
            &staging_uri,
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
fn convert(
    catalog_uri: &str,
    opts: AHashMap<String, String>,
    staging_uri: &str,
    venue: Option<&str>,
    family: Option<&str>,
) -> Result<()> {
    let catalog_opts = is_remote(catalog_uri).then(|| opts.clone());
    let mut catalog = ParquetDataCatalog::from_uri(catalog_uri, catalog_opts, None, None, None)
        .context("open catalog from uri")?;

    let base_url = Url::parse(staging_uri)
        .with_context(|| format!("parse staging uri {staging_uri:?} (need a file:// or s3:// URI)"))?;
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

        let prefix = ObjectPath::from(format!(
            "{}/{}",
            base_path.as_ref().trim_end_matches('/'),
            binding.sub_prefix.trim_matches('/')
        ));
        let keys = runtime
            .block_on(list_zip_keys(store.clone(), prefix.clone()))
            .with_context(|| format!("list {prefix}"))?;

        let mut objects = 0usize;
        let mut records = 0usize;
        let mut instruments: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

        for key in &keys {
            let bytes = runtime
                .block_on(get_bytes(store.clone(), key.clone()))
                .with_context(|| format!("read {key}"))?;
            let converted = convert_object(binding, &bytes, &mut catalog)
                .with_context(|| format!("convert {key}"))?;
            objects += 1;
            for instrument in converted {
                records += instrument.record_count;
                instruments.insert(instrument.nt_instrument_id);
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

/// Dispatch one staged object to its venue/family converter, appending into the
/// shared catalog. New `(venue, family)` arms are added as their append paths
/// land.
fn convert_object(
    binding: &FamilyBinding,
    bytes: &[u8],
    catalog: &mut ParquetDataCatalog,
) -> Result<Vec<ConvertedInstrument>> {
    match (binding.venue, binding.family) {
        ("okx", "trades") => Ok(append_okx_trades_archive(bytes, catalog)?
            .into_iter()
            .map(|summary| ConvertedInstrument {
                nt_instrument_id: summary.nt_instrument_id,
                record_count: summary.record_count,
            })
            .collect()),
        (venue, family) => bail!("no converter wired for venue={venue} family={family}"),
    }
}

/// List every `.zip` object under `prefix`, in deterministic key order.
async fn list_zip_keys(store: Arc<dyn ObjectStore>, prefix: ObjectPath) -> Result<Vec<ObjectPath>> {
    let mut stream = store.list(Some(&prefix));
    let mut keys = Vec::new();
    while let Some(meta) = stream.next().await {
        let meta = meta.context("list object metadata")?;
        if meta.location.as_ref().ends_with(".zip") {
            keys.push(meta.location);
        }
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
