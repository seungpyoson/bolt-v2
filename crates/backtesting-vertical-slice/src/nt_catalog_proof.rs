//! Bounded proof for NautilusTrader multi-instrument catalog I/O.
//!
//! This module proves pinned NautilusTrader can write, read, query, and run a
//! `BacktestNode` over a configured multi-instrument `ParquetDataCatalog`
//! location. It intentionally does not open the production run-manifest
//! `instrument_ids` surface.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use futures::StreamExt;
use nautilus_backtest::{
    config::{BacktestDataConfig, BacktestRunConfig, BacktestVenueConfig, NautilusDataType},
    node::BacktestNode,
};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::TradeTick,
    enums::{AccountType, AggressorSide, BookType, OmsType},
    identifiers::{InstrumentId, Symbol, TradeId, Venue},
    instruments::{CurrencyPair, Instrument, InstrumentAny},
    types::{Currency, Price, Quantity},
};
use nautilus_persistence::{
    backend::catalog::ParquetDataCatalog, parquet::create_object_store_from_path,
};
use object_store::path::Path as ObjectPath;
use serde::{Deserialize, Serialize};
use ustr::Ustr;

use crate::run_manifest::{ManifestArtifactStore, artifact_store_storage_options_for_uri};

pub const NT_CATALOG_PROOF_SCHEMA_VERSION: &str = "nt-catalog-proof.v1";
pub const NT_CATALOG_PROOF_REPORT_FILE: &str = "nt-catalog-proof-report.json";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogProofSpec {
    pub proof_id: String,
    pub catalog_uri: String,
    pub output_dir: PathBuf,
    pub artifact_store: ManifestArtifactStore,
    pub instruments: Vec<NtCatalogProofInstrumentSpec>,
    pub ticks_per_instrument: usize,
    pub base_timestamp_nanos: u64,
    pub trade_interval_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogProofInstrumentSpec {
    pub symbol: String,
    pub venue: String,
    pub base_currency: String,
    pub quote_currency: String,
    pub price_precision: u8,
    pub size_precision: u8,
    pub price_increment: String,
    pub size_increment: String,
    pub quantity: String,
    pub price_start: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtCatalogProofReport {
    pub schema_version: String,
    pub proof_id: String,
    pub catalog_uri: String,
    pub catalog_protocol: String,
    pub storage_option_keys: Vec<String>,
    pub instrument_ids: Vec<String>,
    pub expected_instrument_count: usize,
    pub nt_instrument_count: usize,
    pub expected_trade_ticks: usize,
    pub nt_trade_ticks: usize,
    pub nt_backtest_iterations: usize,
    pub direct_s3_catalog_write_proven: bool,
    pub direct_s3_catalog_query_proven: bool,
    pub direct_s3_backtest_node_proven: bool,
    pub direct_s3_catalog_access_proven: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtCatalogProofArtifact {
    pub report_path: PathBuf,
    pub content_hash: String,
    pub report_bytes: u64,
    pub catalog_uri: String,
}

pub fn run_nt_catalog_proof_from_spec_file_with_resolver<F>(
    spec_path: &Path,
    resolver: &mut F,
) -> Result<NtCatalogProofArtifact>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    let bytes = fs::read(spec_path)
        .with_context(|| format!("read NT catalog proof spec {}", spec_path.display()))?;
    let spec: NtCatalogProofSpec = toml::from_slice(&bytes)
        .with_context(|| format!("parse NT catalog proof spec TOML {}", spec_path.display()))?;
    run_nt_catalog_proof_with_resolver(&spec, resolver)
}

pub fn run_nt_catalog_proof_with_resolver<F>(
    spec: &NtCatalogProofSpec,
    resolver: &mut F,
) -> Result<NtCatalogProofArtifact>
where
    F: FnMut(&str, &str) -> Result<String, String>,
{
    validate_spec(spec)?;
    if let Some(local_root) = local_catalog_root(&spec.catalog_uri) {
        ensure_local_catalog_root_empty(&local_root)?;
    }

    let storage_options =
        artifact_store_storage_options_for_uri(&spec.catalog_uri, &spec.artifact_store, resolver)
            .map_err(|error| anyhow::anyhow!("resolve catalog artifact-store options: {error}"))?;
    if local_catalog_root(&spec.catalog_uri).is_none() {
        ensure_remote_catalog_prefix_empty(&spec.catalog_uri, storage_options.as_ref())?;
    }

    let instruments = build_instruments(&spec.instruments)?;
    let instrument_ids: Vec<InstrumentId> = instruments.iter().map(Instrument::id).collect();
    let instrument_id_strings: Vec<String> =
        instrument_ids.iter().map(ToString::to_string).collect();
    let ticks = build_trade_ticks(spec, &instrument_ids)?;
    let expected_trade_ticks = ticks.len();

    let mut catalog = ParquetDataCatalog::from_uri(
        &spec.catalog_uri,
        storage_options
            .clone()
            .map(|options| options.into_iter().collect()),
        None,
        None,
        None,
    )
    .with_context(|| format!("open NT ParquetDataCatalog {}", spec.catalog_uri))?;
    catalog
        .write_instruments(
            instruments
                .into_iter()
                .map(InstrumentAny::CurrencyPair)
                .collect(),
        )
        .context("write configured instruments through NT catalog")?;
    catalog
        .write_to_parquet(ticks, None, None, None)
        .context("write configured TradeTick data through NT catalog")?;

    let loaded_instruments = catalog
        .query_instruments(Some(&instrument_id_strings))
        .context("query configured instruments through NT catalog")?;
    let loaded_ticks: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(
            Some(instrument_id_strings.clone()),
            None,
            None,
            None,
            None,
            true,
        )
        .context("query configured TradeTick data through NT catalog")?;

    let result = run_backtest_node(spec, &instrument_ids, storage_options.clone())?;
    let is_s3 = catalog_protocol(&spec.catalog_uri) == "s3";
    let report = NtCatalogProofReport {
        schema_version: NT_CATALOG_PROOF_SCHEMA_VERSION.to_string(),
        proof_id: spec.proof_id.clone(),
        catalog_uri: spec.catalog_uri.clone(),
        catalog_protocol: catalog_protocol(&spec.catalog_uri).to_string(),
        storage_option_keys: storage_options
            .as_ref()
            .map(|options| options.keys().cloned().collect())
            .unwrap_or_default(),
        instrument_ids: instrument_ids.iter().map(ToString::to_string).collect(),
        expected_instrument_count: spec.instruments.len(),
        nt_instrument_count: loaded_instruments.len(),
        expected_trade_ticks,
        nt_trade_ticks: loaded_ticks.len(),
        nt_backtest_iterations: result.iterations,
        direct_s3_catalog_write_proven: is_s3,
        direct_s3_catalog_query_proven: is_s3,
        direct_s3_backtest_node_proven: is_s3,
        direct_s3_catalog_access_proven: is_s3,
    };
    ensure!(
        report.nt_instrument_count == report.expected_instrument_count,
        "NT catalog returned {} instruments, expected {}",
        report.nt_instrument_count,
        report.expected_instrument_count
    );
    ensure!(
        report.nt_trade_ticks == report.expected_trade_ticks,
        "NT catalog returned {} ticks, expected {}",
        report.nt_trade_ticks,
        report.expected_trade_ticks
    );
    ensure!(
        report.nt_backtest_iterations == report.expected_trade_ticks,
        "NT BacktestNode iterated {} times, expected {}",
        report.nt_backtest_iterations,
        report.expected_trade_ticks
    );

    write_report(&spec.output_dir, &report).map(|(report_path, content_hash, report_bytes)| {
        NtCatalogProofArtifact {
            report_path,
            content_hash,
            report_bytes,
            catalog_uri: spec.catalog_uri.clone(),
        }
    })
}

fn validate_spec(spec: &NtCatalogProofSpec) -> Result<()> {
    ensure!(
        !spec.proof_id.trim().is_empty(),
        "proof_id must not be empty"
    );
    ensure!(
        !spec.catalog_uri.trim().is_empty(),
        "catalog_uri must not be empty"
    );
    ensure!(
        matches!(catalog_protocol(&spec.catalog_uri), "file" | "s3"),
        "catalog_uri protocol must be file or s3"
    );
    ensure!(
        spec.instruments.len() >= 2,
        "NT catalog proof requires at least two instruments"
    );
    ensure!(
        spec.ticks_per_instrument > 0,
        "ticks_per_instrument must be positive"
    );
    ensure!(
        spec.trade_interval_nanos > 0,
        "trade_interval_nanos must be positive"
    );
    let mut instrument_ids = BTreeSet::new();
    for instrument in &spec.instruments {
        for (field, value) in [
            ("symbol", instrument.symbol.as_str()),
            ("venue", instrument.venue.as_str()),
            ("base_currency", instrument.base_currency.as_str()),
            ("quote_currency", instrument.quote_currency.as_str()),
            ("price_increment", instrument.price_increment.as_str()),
            ("size_increment", instrument.size_increment.as_str()),
            ("quantity", instrument.quantity.as_str()),
            ("price_start", instrument.price_start.as_str()),
        ] {
            ensure!(
                !value.trim().is_empty(),
                "instrument {field} must not be empty"
            );
        }
        ensure_positive_f64("price_increment", &instrument.price_increment)?;
        ensure_positive_f64("size_increment", &instrument.size_increment)?;
        ensure_positive_f64("quantity", &instrument.quantity)?;
        ensure_positive_f64("price_start", &instrument.price_start)?;
        let id = format!("{}.{}", instrument.symbol, instrument.venue);
        ensure!(
            instrument_ids.insert(id.clone()),
            "duplicate instrument {id}"
        );
    }
    Ok(())
}

fn ensure_positive_f64(field: &str, value: &str) -> Result<()> {
    let parsed = value
        .parse::<f64>()
        .with_context(|| format!("parse {field} as decimal"))?;
    ensure!(parsed > 0.0, "{field} must be positive");
    Ok(())
}

fn build_instruments(specs: &[NtCatalogProofInstrumentSpec]) -> Result<Vec<CurrencyPair>> {
    specs.iter().map(build_instrument).collect()
}

fn build_instrument(spec: &NtCatalogProofInstrumentSpec) -> Result<CurrencyPair> {
    let symbol = Symbol::from(spec.symbol.as_str());
    let venue = Venue::from(spec.venue.as_str());
    let instrument_id = InstrumentId::new(symbol, venue);
    Ok(CurrencyPair::new(
        instrument_id,
        symbol,
        Currency::from(spec.base_currency.as_str()),
        Currency::from(spec.quote_currency.as_str()),
        spec.price_precision,
        spec.size_precision,
        Price::new(
            parse_f64("price_increment", &spec.price_increment)?,
            spec.price_precision,
        ),
        Quantity::new(
            parse_f64("size_increment", &spec.size_increment)?,
            spec.size_precision,
        ),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None, // tick_scheme (NT bump): not populated by bolt
        None,
        UnixNanos::default(),
        UnixNanos::default(),
    ))
}

fn build_trade_ticks(
    spec: &NtCatalogProofSpec,
    instrument_ids: &[InstrumentId],
) -> Result<Vec<TradeTick>> {
    let mut ticks = Vec::with_capacity(spec.ticks_per_instrument * instrument_ids.len());
    for (instrument_index, (instrument, instrument_id)) in spec
        .instruments
        .iter()
        .zip(instrument_ids.iter().copied())
        .enumerate()
    {
        let price_start = parse_f64("price_start", &instrument.price_start)?;
        let quantity = parse_f64("quantity", &instrument.quantity)?;
        for tick_index in 0..spec.ticks_per_instrument {
            let sequence = instrument_index * spec.ticks_per_instrument + tick_index;
            let ts = UnixNanos::from(
                spec.base_timestamp_nanos + (sequence as u64) * spec.trade_interval_nanos,
            );
            let aggressor = if sequence.is_multiple_of(2) {
                AggressorSide::Buyer
            } else {
                AggressorSide::Seller
            };
            ticks.push(TradeTick::new(
                instrument_id,
                Price::new(price_start + tick_index as f64, instrument.price_precision),
                Quantity::new(quantity, instrument.size_precision),
                aggressor,
                TradeId::from(format!("t{sequence}").as_str()),
                ts,
                ts,
            ));
        }
    }
    Ok(ticks)
}

fn parse_f64(field: &str, value: &str) -> Result<f64> {
    value
        .parse::<f64>()
        .with_context(|| format!("parse {field} as decimal"))
}

fn run_backtest_node(
    spec: &NtCatalogProofSpec,
    instrument_ids: &[InstrumentId],
    storage_options: Option<BTreeMap<String, String>>,
) -> Result<nautilus_backtest::result::BacktestResult> {
    let (catalog_path, catalog_fs_protocol) = nt_data_config_catalog_location(&spec.catalog_uri)?;
    let data_config = BacktestDataConfig::builder()
        .data_type(NautilusDataType::TradeTick)
        .catalog_path(catalog_path)
        .maybe_catalog_fs_protocol(catalog_fs_protocol)
        .maybe_catalog_fs_rust_storage_options(
            storage_options.map(|options| options.into_iter().collect()),
        )
        .instrument_ids(instrument_ids.to_vec())
        .build();

    let run_config = BacktestRunConfig::builder()
        .id(spec.proof_id.clone())
        .venues(build_venue_configs(&spec.instruments))
        .data(vec![data_config])
        .build();
    let mut node = BacktestNode::new(vec![run_config]).context("construct NT BacktestNode")?;
    node.build().context("build NT BacktestNode")?;
    let mut results = node.run().context("run NT BacktestNode")?;
    ensure!(
        results.len() == 1,
        "expected exactly one NT backtest result, got {}",
        results.len()
    );
    Ok(results.remove(0))
}

fn build_venue_configs(instruments: &[NtCatalogProofInstrumentSpec]) -> Vec<BacktestVenueConfig> {
    let mut quote_currencies_by_venue: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for instrument in instruments {
        quote_currencies_by_venue
            .entry(instrument.venue.as_str())
            .or_default()
            .insert(instrument.quote_currency.as_str());
    }
    quote_currencies_by_venue
        .into_iter()
        .map(|(venue, quote_currencies)| {
            BacktestVenueConfig::builder()
                .name(Ustr::from(venue))
                .oms_type(OmsType::Netting)
                .account_type(AccountType::Cash)
                .book_type(BookType::L1_MBP)
                .starting_balances(
                    quote_currencies
                        .into_iter()
                        .map(|currency| format!("1_000_000 {currency}"))
                        .collect(),
                )
                .build()
        })
        .collect()
}

fn nt_data_config_catalog_location(catalog_uri: &str) -> Result<(String, Option<String>)> {
    if let Some(path) = catalog_uri.strip_prefix("file://") {
        ensure!(!path.is_empty(), "file catalog URI must include a path");
        return Ok((path.to_string(), None));
    }
    if let Some(path) = catalog_uri.strip_prefix("s3://") {
        ensure!(
            !path.is_empty(),
            "s3 catalog URI must include bucket and prefix"
        );
        return Ok((
            path.trim_end_matches('/').to_string(),
            Some("s3".to_string()),
        ));
    }
    bail!("unsupported catalog URI protocol")
}

fn catalog_protocol(catalog_uri: &str) -> &str {
    catalog_uri
        .split_once("://")
        .map(|(protocol, _)| protocol)
        .unwrap_or("file")
}

fn local_catalog_root(catalog_uri: &str) -> Option<PathBuf> {
    catalog_uri
        .strip_prefix("file://")
        .map(|path| PathBuf::from(path.to_string()))
}

fn ensure_local_catalog_root_empty(catalog_root: &Path) -> Result<()> {
    if !catalog_root.exists() {
        fs::create_dir_all(catalog_root)
            .with_context(|| format!("create catalog root {}", catalog_root.display()))?;
        return Ok(());
    }
    let mut entries = fs::read_dir(catalog_root)
        .with_context(|| format!("read catalog root {}", catalog_root.display()))?;
    ensure!(
        entries.next().transpose()?.is_none(),
        "catalog root is not empty: {}",
        catalog_root.display()
    );
    Ok(())
}

fn ensure_remote_catalog_prefix_empty(
    catalog_uri: &str,
    storage_options: Option<&BTreeMap<String, String>>,
) -> Result<()> {
    let (object_store, base_path, _) = create_object_store_from_path(
        catalog_uri,
        storage_options
            .cloned()
            .map(|options| options.into_iter().collect()),
    )
    .with_context(|| format!("open object store for catalog prefix {catalog_uri}"))?;
    let prefix = if base_path.trim_matches('/').is_empty() {
        None
    } else {
        Some(ObjectPath::from(format!(
            "{}/",
            base_path.trim_matches('/')
        )))
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build Tokio runtime for catalog prefix check")?;
    let first_object = runtime
        .block_on(async {
            let mut stream = object_store.list(prefix.as_ref());
            match stream.next().await {
                Some(Ok(metadata)) => Ok::<_, object_store::Error>(Some(metadata.location)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            }
        })
        .with_context(|| format!("list catalog prefix {catalog_uri}"))?;
    ensure!(
        first_object.is_none(),
        "catalog root is not empty: {catalog_uri}"
    );
    Ok(())
}

fn write_report(
    output_dir: &Path,
    report: &NtCatalogProofReport,
) -> Result<(PathBuf, String, u64)> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "create NT catalog proof output dir {}",
            output_dir.display()
        )
    })?;
    let path = output_dir.join(NT_CATALOG_PROOF_REPORT_FILE);
    let written = crate::reference_artifact::write_reference_artifact_with_len(
        &path,
        NT_CATALOG_PROOF_REPORT_FILE,
        report,
        crate::reference_artifact::ReferenceArtifactRewrite::FailOnDirty,
    )
    .with_context(|| format!("write NT catalog proof report {}", path.display()))?;
    Ok((path, written.pin.sha256, written.bytes))
}
