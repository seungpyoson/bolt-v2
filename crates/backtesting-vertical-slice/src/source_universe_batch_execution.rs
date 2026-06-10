//! Batch execution for source-universe single-object operator runs.
//!
//! Source-universe execution packs already materialize one run-spec and
//! execution plan per accepted object. This module adds the missing operator
//! loop: fetch the pinned object, verify bytes/hash, run the existing
//! single-object operator path, and summarize the completed records.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use object_store::{ObjectStoreExt, path::Path as ObjectPath};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    operator::{RunSpec, run_from_run_spec},
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackRecord,
        SourceUniverseExecutionPackStatus,
    },
};

pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION: &str =
    "source-universe-batch-execution-report.v1";
pub const SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE: &str =
    "source-universe-batch-execution-report.json";

pub trait SourceUniverseObjectFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>>;
}

pub trait SourceUniverseOperatorRunner {
    fn run(
        &mut self,
        record: &SourceUniverseExecutionPackRecord,
        object_bytes: &[u8],
        run_spec_path: &Path,
        execution_plan_path: &Path,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionRunOutput {
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionConfig {
    pub start_sequence: Option<u64>,
    pub record_limit: Option<u64>,
    pub continue_on_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUniverseBatchExecutionReportStatus {
    Completed,
    CompletedWithFailures,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionRecord {
    pub sequence: u64,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub selected_object_sha256: String,
    pub selected_object_bytes: u64,
    pub canonical_rows: u64,
    pub nt_catalog_rows: u64,
    pub catalog_hash: String,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionFailureRecord {
    pub sequence: u64,
    pub operator_run_id: String,
    pub source_binding: String,
    pub category: String,
    pub symbol: String,
    pub archive_date: String,
    pub selected_object_sha256: String,
    pub selected_object_bytes: u64,
    pub failure_stage: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceUniverseBatchExecutionReport {
    pub schema_version: String,
    pub batch_id: String,
    pub status: SourceUniverseBatchExecutionReportStatus,
    pub pack_id: String,
    pub universe_id: String,
    pub venue: String,
    pub selected_record_count: u64,
    pub completed_record_count: u64,
    pub failed_record_count: u64,
    pub total_canonical_rows: u64,
    pub total_nt_catalog_rows: u64,
    pub records: Vec<SourceUniverseBatchExecutionRecord>,
    pub failures: Vec<SourceUniverseBatchExecutionFailureRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUniverseBatchExecutionReportArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
    pub completed_record_count: u64,
}

pub struct HttpSourceUniverseObjectFetcher {
    runtime: tokio::runtime::Runtime,
}

impl HttpSourceUniverseObjectFetcher {
    pub fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("create HTTP fetch runtime")?;
        Ok(Self { runtime })
    }
}

impl SourceUniverseObjectFetcher for HttpSourceUniverseObjectFetcher {
    fn fetch(&mut self, record: &SourceUniverseExecutionPackRecord) -> Result<Vec<u8>> {
        let (base_url, object_path) = http_base_url_and_object_path(&record.source_url)?;
        let store = object_store::http::HttpBuilder::new()
            .with_url(base_url)
            .build()
            .with_context(|| format!("open HTTP object store for {}", record.source_url))?;
        let bytes = self.runtime.block_on(async {
            let result = store
                .get(&ObjectPath::from(object_path))
                .await
                .with_context(|| format!("GET {}", record.source_url))?;
            result
                .bytes()
                .await
                .with_context(|| format!("read response body {}", record.source_url))
        })?;
        Ok(bytes.to_vec())
    }
}

#[derive(Default)]
pub struct LocalSourceUniverseOperatorRunner;

impl SourceUniverseOperatorRunner for LocalSourceUniverseOperatorRunner {
    fn run(
        &mut self,
        _record: &SourceUniverseExecutionPackRecord,
        object_bytes: &[u8],
        run_spec_path: &Path,
        _execution_plan_path: &Path,
        output_dir: &Path,
    ) -> Result<SourceUniverseBatchExecutionRunOutput> {
        let run_spec_bytes = fs::read(run_spec_path)
            .with_context(|| format!("read run-spec {}", run_spec_path.display()))?;
        let run_spec_text = std::str::from_utf8(&run_spec_bytes)
            .with_context(|| format!("decode run-spec {} as UTF-8", run_spec_path.display()))?;
        let run_spec: RunSpec = toml::from_str(run_spec_text)
            .with_context(|| format!("parse run-spec TOML {}", run_spec_path.display()))?;
        let artifacts = run_from_run_spec(&run_spec, object_bytes, output_dir)?;
        Ok(SourceUniverseBatchExecutionRunOutput {
            canonical_rows: artifacts.output.canonical_table.rows.len() as u64,
            nt_catalog_rows: artifacts.output.read_back_count as u64,
            catalog_hash: artifacts.output.projection.catalog_hash,
        })
    }
}

pub fn execute_source_universe_batch<F, R>(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    record_limit: Option<u64>,
    fetcher: &mut F,
    runner: &mut R,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    execute_source_universe_batch_with_config(
        batch_id,
        execution_pack_path,
        output_dir,
        SourceUniverseBatchExecutionConfig {
            record_limit,
            ..SourceUniverseBatchExecutionConfig::default()
        },
        fetcher,
        runner,
    )
}

pub fn execute_source_universe_batch_with_config<F, R>(
    batch_id: &str,
    execution_pack_path: &Path,
    output_dir: &Path,
    config: SourceUniverseBatchExecutionConfig,
    fetcher: &mut F,
    runner: &mut R,
) -> Result<SourceUniverseBatchExecutionReport>
where
    F: SourceUniverseObjectFetcher,
    R: SourceUniverseOperatorRunner,
{
    ensure!(!batch_id.trim().is_empty(), "batch_id must not be empty");
    if let Some(limit) = config.record_limit {
        ensure!(limit > 0, "record_limit must be positive when set");
    }

    let pack_bytes = fs::read(execution_pack_path)
        .with_context(|| format!("read execution pack {}", execution_pack_path.display()))?;
    let pack: SourceUniverseExecutionPack = serde_json::from_slice(&pack_bytes)
        .with_context(|| format!("parse execution pack {}", execution_pack_path.display()))?;
    ensure!(
        matches!(
            pack.status,
            SourceUniverseExecutionPackStatus::Ready
                | SourceUniverseExecutionPackStatus::PartiallyReady
        ),
        "execution pack {} is not executable",
        pack.pack_id
    );

    let pack_base_dir = execution_pack_path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch output dir {}", output_dir.display()))?;

    let record_limit = config
        .record_limit
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);
    let selected_records = pack
        .records
        .iter()
        .filter(|record| {
            config
                .start_sequence
                .is_none_or(|start_sequence| record.sequence >= start_sequence)
        })
        .take(record_limit)
        .collect::<Vec<_>>();
    let mut records = Vec::with_capacity(selected_records.len());
    let mut failures = Vec::new();
    let mut total_canonical_rows = 0_u64;
    let mut total_nt_catalog_rows = 0_u64;

    for record in selected_records {
        let object_bytes = match fetcher
            .fetch(record)
            .with_context(|| format!("fetch source object for {}", record.operator_run_id))
        {
            Ok(object_bytes) => object_bytes,
            Err(error) if config.continue_on_error => {
                failures.push(failure_record(record, "fetch", &error));
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Err(error) = verify_object(record, &object_bytes) {
            let error = error.context(format!(
                "verify source object for {}",
                record.operator_run_id
            ));
            if config.continue_on_error {
                failures.push(failure_record(record, "verify_object", &error));
                continue;
            }
            return Err(error);
        }

        let run_spec_path = resolve_existing_path(pack_base_dir, &record.run_spec_path);
        let execution_plan_path = resolve_existing_path(pack_base_dir, &record.execution_plan_path);
        let record_output_dir = output_dir.join(&record.operator_run_id);
        let run_output = match runner
            .run(
                record,
                &object_bytes,
                &run_spec_path,
                &execution_plan_path,
                &record_output_dir,
            )
            .with_context(|| format!("run operator {}", record.operator_run_id))
        {
            Ok(run_output) => run_output,
            Err(error) if config.continue_on_error => {
                failures.push(failure_record(record, "run_operator", &error));
                continue;
            }
            Err(error) => return Err(error),
        };

        total_canonical_rows = total_canonical_rows.saturating_add(run_output.canonical_rows);
        total_nt_catalog_rows = total_nt_catalog_rows.saturating_add(run_output.nt_catalog_rows);
        records.push(SourceUniverseBatchExecutionRecord {
            sequence: record.sequence,
            operator_run_id: record.operator_run_id.clone(),
            source_binding: record.source_binding.clone(),
            category: record.category.clone(),
            symbol: record.symbol.clone(),
            archive_date: record.archive_date.clone(),
            selected_object_sha256: record.selected_object_sha256.clone(),
            selected_object_bytes: record.selected_object_bytes,
            canonical_rows: run_output.canonical_rows,
            nt_catalog_rows: run_output.nt_catalog_rows,
            catalog_hash: run_output.catalog_hash,
            output_dir: record_output_dir,
        });
    }
    let status = if failures.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Completed
    } else if records.is_empty() {
        SourceUniverseBatchExecutionReportStatus::Failed
    } else {
        SourceUniverseBatchExecutionReportStatus::CompletedWithFailures
    };

    Ok(SourceUniverseBatchExecutionReport {
        schema_version: SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_SCHEMA_VERSION.to_string(),
        batch_id: batch_id.to_string(),
        status,
        pack_id: pack.pack_id,
        universe_id: pack.universe_id,
        venue: pack.venue,
        selected_record_count: records.len().saturating_add(failures.len()) as u64,
        completed_record_count: records.len() as u64,
        failed_record_count: failures.len() as u64,
        total_canonical_rows,
        total_nt_catalog_rows,
        records,
        failures,
    })
}

pub fn write_source_universe_batch_execution_report(
    output_dir: &Path,
    report: &SourceUniverseBatchExecutionReport,
) -> Result<SourceUniverseBatchExecutionReportArtifact> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create batch execution report dir {}", output_dir.display()))?;
    let bytes = serde_json::to_vec_pretty(report).context("serialize batch execution report")?;
    let path = output_dir.join(SOURCE_UNIVERSE_BATCH_EXECUTION_REPORT_FILE);
    if path.exists() {
        let existing = fs::read(&path)
            .with_context(|| format!("read existing batch execution report {}", path.display()))?;
        ensure!(
            existing == bytes,
            "dirty batch execution report {}: existing file content differs",
            path.display()
        );
    } else {
        fs::write(&path, &bytes)
            .with_context(|| format!("write batch execution report {}", path.display()))?;
    }
    Ok(SourceUniverseBatchExecutionReportArtifact {
        path,
        content_hash: hex::encode(Sha256::digest(&bytes)),
        bytes: bytes.len() as u64,
        completed_record_count: report.completed_record_count,
    })
}

fn verify_object(record: &SourceUniverseExecutionPackRecord, object_bytes: &[u8]) -> Result<()> {
    ensure!(
        object_bytes.len() as u64 == record.selected_object_bytes,
        "object byte length for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_bytes,
        object_bytes.len()
    );
    let actual_sha256 = hex::encode(Sha256::digest(object_bytes));
    ensure!(
        actual_sha256 == record.selected_object_sha256,
        "object sha256 for {} does not match execution pack: expected {}, got {}",
        record.operator_run_id,
        record.selected_object_sha256,
        actual_sha256
    );
    Ok(())
}

fn failure_record(
    record: &SourceUniverseExecutionPackRecord,
    failure_stage: &str,
    error: &anyhow::Error,
) -> SourceUniverseBatchExecutionFailureRecord {
    SourceUniverseBatchExecutionFailureRecord {
        sequence: record.sequence,
        operator_run_id: record.operator_run_id.clone(),
        source_binding: record.source_binding.clone(),
        category: record.category.clone(),
        symbol: record.symbol.clone(),
        archive_date: record.archive_date.clone(),
        selected_object_sha256: record.selected_object_sha256.clone(),
        selected_object_bytes: record.selected_object_bytes,
        failure_stage: failure_stage.to_string(),
        error: format!("{error:#}"),
    }
}

fn resolve_existing_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() && path.exists() {
        return path.to_path_buf();
    }
    if path.exists() {
        return path.to_path_buf();
    }
    let base_relative = base_dir.join(path);
    if base_relative.exists() {
        return base_relative;
    }
    let repo_relative = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path);
    if repo_relative.exists() {
        return repo_relative;
    }
    path.to_path_buf()
}

fn http_base_url_and_object_path(source_url: &str) -> Result<(String, String)> {
    ensure!(
        source_url.starts_with("https://"),
        "source_url must be HTTPS for batch execution: {source_url}"
    );
    let without_scheme = source_url
        .strip_prefix("https://")
        .expect("checked HTTPS prefix");
    let (host, path) = without_scheme
        .split_once('/')
        .with_context(|| format!("source_url missing object path: {source_url}"))?;
    ensure!(!host.trim().is_empty(), "source_url host must not be empty");
    ensure!(!path.trim().is_empty(), "source_url path must not be empty");
    ensure!(
        !path.contains('?') && !path.contains('#'),
        "source_url query and fragment components are not supported: {source_url}"
    );
    Ok((format!("https://{host}"), path.to_string()))
}
