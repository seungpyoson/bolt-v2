//! Driver: convert staged venue data into a NautilusTrader `ParquetDataCatalog`.
//!
//! The catalog target is any `object_store` URI — an `s3://bucket/prefix` lake
//! or a local path — resolved through NautilusTrader's own
//! [`ParquetDataCatalog::from_uri`], so the same converters that round-trip in
//! the test suite write straight into a cloud catalog with no extra plumbing.
//!
//! `smoke` proves the catalog's object-store write + read-back path works end to
//! end against the real target before the full multi-venue conversion run. It is
//! a connectivity/credentials probe, not a data conversion.

use ahash::AHashMap;
use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use std::str::FromStr;

use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::AggressorSide,
    identifiers::{InstrumentId, TradeId},
    types::{Price, Quantity},
};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;

/// Default region for the `bolt-parquet` data lake (a bucket with no location
/// constraint resolves to `us-east-1`).
const DEFAULT_REGION: &str = "us-east-1";

/// Synthetic instrument id used only by the `smoke` connectivity probe.
const SMOKE_INSTRUMENT_ID: &str = "SMOKE.TEST";

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
}

/// Build object-store storage options from `--region`/`--catalog-uri` and the
/// standard AWS environment.
///
/// Region resolves from `--region`, then `AWS_REGION`, then [`DEFAULT_REGION`].
/// Static credentials are read from the standard AWS environment variables when
/// present and forwarded to the object store; the values are never logged. When
/// the target is a local path the options are harmless (ignored by the local
/// backend).
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

    let mut catalog = ParquetDataCatalog::from_uri(catalog_uri, Some(opts), None, None, None)
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
