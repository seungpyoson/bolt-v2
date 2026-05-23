//! Bolt-v3 Phase 7 no-submit readiness report producer.
//!
//! This module owns report modeling, redaction, and sequencing. NT still
//! owns adapter behavior, connection dispatch, cache, lifecycle, order state,
//! reconciliation, and venue wire behavior.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{SystemTime, SystemTimeError, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, resolve_root_relative_path},
    bolt_v3_live_node::{
        BoltV3LiveNodeError, BoltV3LiveNodeRuntime, BoltV3NoSubmitReferenceCacheEvidence,
        BoltV3NoSubmitReferenceQuote, BoltV3NoSubmitReferenceQuoteEvidence,
        build_bolt_v3_no_submit_live_node, controlled_no_submit_readiness,
    },
    bolt_v3_no_submit_readiness_schema::{
        CONTROLLED_CONNECT_STAGE, CONTROLLED_DISCONNECT_STAGE, LIVE_NODE_BUILD_STAGE,
        NO_SUBMIT_READINESS_SCHEMA_VERSION, OPERATOR_APPROVAL_STAGE, REDACTED_DETAIL_MARKER,
        REFERENCE_READINESS_STAGE, REPORT_WRITE_STAGE, SECRET_RESOLUTION_STAGE,
    },
};

const REFERENCE_CACHE_ONLY_LIMITATION_DETAIL: &str = "NT cache only proves required reference instrument IDs are present; no live reference-data freshness or timestamp surface is available";
const NANOS_PER_SECOND: u64 = 1_000_000_000;

trait RedactionValue {
    fn as_redaction_str(&self) -> &str;
}

impl RedactionValue for String {
    fn as_redaction_str(&self) -> &str {
        self.as_str()
    }
}

impl RedactionValue for Zeroizing<String> {
    fn as_redaction_str(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug)]
pub enum BoltV3NoSubmitReadinessError {
    MissingLiveCanaryConfig,
    MissingOperatorApprovalId,
    CurrentExecutablePath {
        source: std::io::Error,
    },
    ExecutableIdentityRead {
        path: PathBuf,
        source: std::io::Error,
    },
    SystemTimeBeforeUnixEpoch {
        source: SystemTimeError,
    },
    ActiveTokioRuntime,
    RuntimeBuild {
        source: std::io::Error,
    },
    LiveNode {
        source: BoltV3LiveNodeError,
    },
    ReportTooLarge {
        path: PathBuf,
        length: u64,
        max_length: u64,
    },
    ReportParentCreate {
        path: PathBuf,
        source: std::io::Error,
    },
    ReportWrite {
        path: PathBuf,
        source: std::io::Error,
    },
    ReportSerialize {
        source: serde_json::Error,
    },
}

impl std::fmt::Display for BoltV3NoSubmitReadinessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingLiveCanaryConfig => {
                write!(f, "bolt-v3 no-submit readiness requires `[live_canary]`")
            }
            Self::MissingOperatorApprovalId => {
                write!(
                    f,
                    "bolt-v3 no-submit readiness operator approval id is empty"
                )
            }
            Self::CurrentExecutablePath { source } => write!(
                f,
                "failed to resolve bolt-v3 no-submit readiness executable path: {source}"
            ),
            Self::ExecutableIdentityRead { path, source } => write!(
                f,
                "failed to read bolt-v3 no-submit readiness executable {}: {source}",
                path.display()
            ),
            Self::SystemTimeBeforeUnixEpoch { source } => write!(
                f,
                "failed to timestamp bolt-v3 no-submit readiness report: {source}"
            ),
            Self::ActiveTokioRuntime => write!(
                f,
                "bolt-v3 no-submit readiness must start from the synchronous startup boundary before entering Tokio runtime"
            ),
            Self::RuntimeBuild { source } => write!(
                f,
                "failed to build Tokio runtime for bolt-v3 no-submit readiness: {source}"
            ),
            Self::LiveNode { source } => write!(
                f,
                "bolt-v3 no-submit readiness live-node operation failed: {source}"
            ),
            Self::ReportTooLarge {
                path,
                length,
                max_length,
            } => write!(
                f,
                "bolt-v3 no-submit readiness report {} is {length} bytes, exceeding configured limit {max_length}",
                path.display()
            ),
            Self::ReportParentCreate { path, source } => write!(
                f,
                "failed to create bolt-v3 no-submit readiness report parent for {}: {source}",
                path.display()
            ),
            Self::ReportWrite { path, source } => write!(
                f,
                "failed to write bolt-v3 no-submit readiness report {}: {source}",
                path.display()
            ),
            Self::ReportSerialize { source } => {
                write!(
                    f,
                    "failed to serialize bolt-v3 no-submit readiness report: {source}"
                )
            }
        }
    }
}

impl std::error::Error for BoltV3NoSubmitReadinessError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RuntimeBuild { source } => Some(source),
            Self::LiveNode { source } => Some(source),
            Self::CurrentExecutablePath { source } => Some(source),
            Self::ExecutableIdentityRead { source, .. } => Some(source),
            Self::SystemTimeBeforeUnixEpoch { source } => Some(source),
            Self::ReportParentCreate { source, .. } | Self::ReportWrite { source, .. } => {
                Some(source)
            }
            Self::ReportSerialize { source } => Some(source),
            Self::MissingLiveCanaryConfig
            | Self::MissingOperatorApprovalId
            | Self::ActiveTokioRuntime
            | Self::ReportTooLarge { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3NoSubmitReadinessStatus {
    Satisfied,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessStage {
    pub stage: &'static str,
    pub status: BoltV3NoSubmitReadinessStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessReportMetadata {
    pub approval_id_hash: String,
    pub executable_identity: String,
    pub config_bundle_checksum: String,
}

impl BoltV3NoSubmitReadinessReportMetadata {
    pub async fn from_loaded(
        loaded: &LoadedBoltV3Config,
    ) -> Result<Self, BoltV3NoSubmitReadinessError> {
        let approval_id_hash = configured_operator_approval_hash(loaded)?;
        Ok(Self {
            approval_id_hash,
            executable_identity: executable_identity().await?,
            config_bundle_checksum: loaded.config_bundle_checksum.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3NoSubmitReadinessReport {
    pub schema_version: &'static str,
    pub approval_id_hash: String,
    pub executable_identity: String,
    pub config_bundle_checksum: String,
    pub generated_at_unix_seconds: u64,
    pub stages: Vec<BoltV3NoSubmitReadinessStage>,
}

impl BoltV3NoSubmitReadinessReport {
    pub fn stage_status(&self, stage: &str) -> Vec<BoltV3NoSubmitReadinessStatus> {
        self.stages
            .iter()
            .filter(|item| item.stage == stage)
            .map(|item| item.status)
            .collect()
    }

    pub fn write_redacted_json_with_max_bytes(
        &self,
        path: &Path,
        max_length: u64,
    ) -> Result<(), BoltV3NoSubmitReadinessError> {
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|source| BoltV3NoSubmitReadinessError::ReportSerialize { source })?;
        let length = bytes.len() as u64;
        if length > max_length {
            return Err(BoltV3NoSubmitReadinessError::ReportTooLarge {
                path: path.to_path_buf(),
                length,
                max_length,
            });
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                BoltV3NoSubmitReadinessError::ReportParentCreate {
                    path: path.to_path_buf(),
                    source,
                }
            })?;
        }
        std::fs::write(path, bytes).map_err(|source| BoltV3NoSubmitReadinessError::ReportWrite {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn write_configured_redacted_json(
        &self,
        loaded: &LoadedBoltV3Config,
    ) -> Result<(), BoltV3NoSubmitReadinessError> {
        let block = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?;
        self.write_redacted_json_with_max_bytes(
            &configured_report_path(loaded, &block.no_submit_readiness_report_path),
            block.max_no_submit_readiness_report_bytes,
        )
    }
}

pub fn run_bolt_v3_no_submit_readiness_from_stage_results(
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    controlled_connect: Result<(), String>,
    reference_readiness: Result<(), String>,
    controlled_disconnect: Result<(), String>,
    redacted_values: &[String],
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    let generated_at_unix_seconds = current_unix_seconds()?;
    Ok(run_bolt_v3_no_submit_readiness_from_stage_results_at(
        metadata,
        controlled_connect,
        reference_readiness,
        controlled_disconnect,
        redacted_values,
        generated_at_unix_seconds,
    ))
}

pub fn run_bolt_v3_no_submit_readiness_from_stage_results_at(
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    controlled_connect: Result<(), String>,
    reference_readiness: Result<(), String>,
    controlled_disconnect: Result<(), String>,
    redacted_values: &[String],
    generated_at_unix_seconds: u64,
) -> BoltV3NoSubmitReadinessReport {
    run_bolt_v3_no_submit_readiness_from_stage_results_at_impl(
        metadata,
        controlled_connect,
        reference_readiness,
        controlled_disconnect,
        redacted_values,
        generated_at_unix_seconds,
    )
}

fn run_bolt_v3_no_submit_readiness_from_stage_results_at_impl<T: RedactionValue>(
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    controlled_connect: Result<(), String>,
    reference_readiness: Result<(), String>,
    controlled_disconnect: Result<(), String>,
    redacted_values: &[T],
    generated_at_unix_seconds: u64,
) -> BoltV3NoSubmitReadinessReport {
    let mut stages = Vec::new();
    push_satisfied_stage(&mut stages, OPERATOR_APPROVAL_STAGE);
    push_satisfied_stage(&mut stages, SECRET_RESOLUTION_STAGE);
    push_satisfied_stage(&mut stages, LIVE_NODE_BUILD_STAGE);
    let connected = push_result_stage(
        &mut stages,
        CONTROLLED_CONNECT_STAGE,
        controlled_connect,
        redacted_values,
    );
    if connected {
        push_result_stage(
            &mut stages,
            REFERENCE_READINESS_STAGE,
            reference_readiness,
            redacted_values,
        );
    } else {
        stages.push(BoltV3NoSubmitReadinessStage {
            stage: REFERENCE_READINESS_STAGE,
            status: BoltV3NoSubmitReadinessStatus::Skipped,
            detail: Some("controlled connect failed".to_string()),
        });
    }
    push_result_stage(
        &mut stages,
        CONTROLLED_DISCONNECT_STAGE,
        controlled_disconnect,
        redacted_values,
    );
    push_satisfied_stage(&mut stages, REPORT_WRITE_STAGE);
    BoltV3NoSubmitReadinessReport {
        schema_version: NO_SUBMIT_READINESS_SCHEMA_VERSION,
        approval_id_hash: metadata.approval_id_hash,
        executable_identity: metadata.executable_identity,
        config_bundle_checksum: metadata.config_bundle_checksum,
        generated_at_unix_seconds,
        stages,
    }
}

/// Audits required reference instrument cache membership as diagnostic evidence.
/// Always returns `Err`: complete cache membership is still not live
/// reference-data freshness proof.
pub fn reference_readiness_from_cached_instrument_ids<I, S>(
    loaded: &LoadedBoltV3Config,
    cached_instrument_ids: I,
) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let cached = cached_instrument_ids
        .into_iter()
        .map(|instrument_id| instrument_id.as_ref().trim().to_string())
        .filter(|instrument_id| !instrument_id.is_empty())
        .collect::<BTreeSet<_>>();
    let missing = loaded
        .strategies
        .iter()
        .flat_map(|strategy| {
            strategy
                .config
                .reference_data
                .iter()
                .filter_map(|(role, reference)| {
                    let instrument_id = reference.instrument_id.to_string();
                    (!cached.contains(instrument_id.as_str())).then(|| {
                        format!(
                            "{} reference_data.{role} instrument_id `{instrument_id}`",
                            strategy.relative_path
                        )
                    })
                })
        })
        .collect::<Vec<_>>();

    if missing.is_empty() {
        Err(REFERENCE_CACHE_ONLY_LIMITATION_DETAIL.to_string())
    } else {
        Err(format!(
            "missing required reference instruments in NT cache: {}",
            missing.join(", ")
        ))
    }
}

pub fn reference_readiness_from_cache_evidence(
    loaded: &LoadedBoltV3Config,
    evidence: &BoltV3NoSubmitReferenceCacheEvidence,
) -> Result<(), String> {
    reference_readiness_from_cached_instrument_ids(loaded, evidence.cached_instrument_ids())
}

pub fn reference_readiness_from_quote_evidence(
    loaded: &LoadedBoltV3Config,
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
    observed_at_unix_nanos: u64,
) -> Result<(), String> {
    let max_age_seconds = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| "reference quote freshness requires `[live_canary]`".to_string())?
        .reference_quote_max_age_seconds;
    if max_age_seconds == 0 {
        return Err(
            "[live_canary].reference_quote_max_age_seconds must be a positive integer".to_string(),
        );
    }
    let max_age_nanos = max_age_seconds
        .checked_mul(NANOS_PER_SECOND)
        .ok_or_else(|| {
            "[live_canary].reference_quote_max_age_seconds overflows nanoseconds".to_string()
        })?;

    let latest_quotes = latest_reference_quotes_by_key(evidence);
    let mut failures = Vec::new();
    let mut required_count = 0usize;
    for strategy in &loaded.strategies {
        for (role, reference) in &strategy.config.reference_data {
            required_count += 1;
            let data_client_id = reference.data_client_id.to_string();
            let instrument_id = reference.instrument_id.to_string();
            let label = format!(
                "{} reference_data.{role} data_client_id `{data_client_id}` instrument_id `{instrument_id}`",
                strategy.relative_path
            );
            let Some(quote) = latest_quotes.get(&(data_client_id, instrument_id)) else {
                failures.push(format!("missing live quote evidence for {label}"));
                continue;
            };
            let Some(age_nanos) = observed_at_unix_nanos.checked_sub(quote.ts_event_unix_nanos)
            else {
                failures.push(format!(
                    "live quote evidence for {label} has future ts_event_unix_nanos {} > observed_at_unix_nanos {observed_at_unix_nanos}",
                    quote.ts_event_unix_nanos
                ));
                continue;
            };
            if age_nanos > max_age_nanos {
                failures.push(format!(
                    "live quote evidence for {label} is stale: age_nanos={age_nanos} > max_age_nanos={max_age_nanos} ([live_canary].reference_quote_max_age_seconds={max_age_seconds})"
                ));
            }
        }
    }

    if required_count == 0 {
        failures.push("no configured reference_data requirements found".to_string());
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn latest_reference_quotes_by_key(
    evidence: &BoltV3NoSubmitReferenceQuoteEvidence,
) -> BTreeMap<(String, String), &BoltV3NoSubmitReferenceQuote> {
    let mut latest: BTreeMap<(String, String), &BoltV3NoSubmitReferenceQuote> = BTreeMap::new();
    for quote in &evidence.quotes {
        let key = (quote.data_client_id.clone(), quote.instrument_id.clone());
        match latest.get(&key) {
            Some(existing) if existing.ts_event_unix_nanos >= quote.ts_event_unix_nanos => {}
            _ => {
                latest.insert(key, quote);
            }
        }
    }
    latest
}

pub async fn run_bolt_v3_no_submit_readiness_on_runtime(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    metadata: BoltV3NoSubmitReadinessReportMetadata,
    redacted_values: &[Zeroizing<String>],
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    let (connect, reference, disconnect) =
        controlled_no_submit_readiness(runtime, loaded, |_runtime, quote_evidence| {
            let observed_at_unix_nanos = quote_evidence
                .observed_at_unix_nanos()
                .ok_or_else(|| "no live reference quote evidence was captured".to_string())?;
            reference_readiness_from_quote_evidence(loaded, quote_evidence, observed_at_unix_nanos)
        })
        .await;
    let generated_at_unix_seconds = current_unix_seconds()?;
    Ok(run_bolt_v3_no_submit_readiness_from_stage_results_at_impl(
        metadata,
        connect.map_err(|error| error.to_string()),
        reference,
        disconnect.map_err(|error| error.to_string()),
        redacted_values,
        generated_at_unix_seconds,
    ))
}

pub fn run_bolt_v3_no_submit_readiness(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReadinessReport, BoltV3NoSubmitReadinessError> {
    if tokio::runtime::Handle::try_current().is_ok() {
        return Err(BoltV3NoSubmitReadinessError::ActiveTokioRuntime);
    }
    configured_operator_approval_hash(loaded)?;

    let mut runtime = build_bolt_v3_no_submit_live_node(loaded)
        .map_err(|source| BoltV3NoSubmitReadinessError::LiveNode { source })?;
    let redacted_values = runtime.redaction_values().to_vec();

    let readiness_runtime = no_submit_readiness_tokio_runtime()?;
    let metadata =
        readiness_runtime.block_on(BoltV3NoSubmitReadinessReportMetadata::from_loaded(loaded))?;
    let local = tokio::task::LocalSet::new();
    readiness_runtime.block_on(local.run_until(run_bolt_v3_no_submit_readiness_on_runtime(
        &mut runtime,
        loaded,
        metadata,
        &redacted_values,
    )))
}

fn no_submit_readiness_tokio_runtime()
-> Result<tokio::runtime::Runtime, BoltV3NoSubmitReadinessError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| BoltV3NoSubmitReadinessError::RuntimeBuild { source })
}

fn configured_operator_approval_hash(
    loaded: &LoadedBoltV3Config,
) -> Result<String, BoltV3NoSubmitReadinessError> {
    let configured = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or(BoltV3NoSubmitReadinessError::MissingLiveCanaryConfig)?
        .approval_id
        .trim();
    if configured.is_empty() {
        return Err(BoltV3NoSubmitReadinessError::MissingOperatorApprovalId);
    }
    Ok(sha256_hex(configured.as_bytes()))
}

fn configured_report_path(loaded: &LoadedBoltV3Config, configured: &str) -> PathBuf {
    resolve_root_relative_path(&loaded.root_path, configured)
}

fn current_unix_seconds() -> Result<u64, BoltV3NoSubmitReadinessError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| BoltV3NoSubmitReadinessError::SystemTimeBeforeUnixEpoch { source })
}

async fn executable_identity() -> Result<String, BoltV3NoSubmitReadinessError> {
    let path = std::env::current_exe()
        .map_err(|source| BoltV3NoSubmitReadinessError::CurrentExecutablePath { source })?;
    let bytes = tokio::fs::read(&path).await.map_err(|source| {
        BoltV3NoSubmitReadinessError::ExecutableIdentityRead {
            path: path.clone(),
            source,
        }
    })?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn push_satisfied_stage(stages: &mut Vec<BoltV3NoSubmitReadinessStage>, stage: &'static str) {
    stages.push(BoltV3NoSubmitReadinessStage {
        stage,
        status: BoltV3NoSubmitReadinessStatus::Satisfied,
        detail: None,
    });
}

fn push_result_stage<T: RedactionValue>(
    stages: &mut Vec<BoltV3NoSubmitReadinessStage>,
    stage: &'static str,
    result: Result<(), String>,
    redacted_values: &[T],
) -> bool {
    match result {
        Ok(()) => {
            stages.push(BoltV3NoSubmitReadinessStage {
                stage,
                status: BoltV3NoSubmitReadinessStatus::Satisfied,
                detail: None,
            });
            true
        }
        Err(detail) => {
            stages.push(BoltV3NoSubmitReadinessStage {
                stage,
                status: BoltV3NoSubmitReadinessStatus::Failed,
                detail: Some(redact_detail(&detail, redacted_values)),
            });
            false
        }
    }
}

fn redact_detail<T: RedactionValue>(detail: &str, redacted_values: &[T]) -> String {
    let mut values = redacted_values
        .iter()
        .map(RedactionValue::as_redaction_str)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    values.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    values.dedup();
    if values.is_empty() {
        return detail.to_string();
    }

    let mut occupied = vec![false; detail.len()];
    let mut ranges = Vec::new();
    for value in values {
        for (start, _) in detail.match_indices(value) {
            let end = start + value.len();
            if occupied[start..end].iter().any(|taken| *taken) {
                continue;
            }
            occupied[start..end].fill(true);
            ranges.push((start, end));
        }
    }
    if ranges.is_empty() {
        return detail.to_string();
    }

    ranges.sort_unstable_by_key(|(start, _)| *start);
    let mut redacted = String::with_capacity(detail.len());
    let mut cursor = 0;
    for (start, end) in ranges {
        redacted.push_str(&detail[cursor..start]);
        redacted.push_str(REDACTED_DETAIL_MARKER);
        cursor = end;
    }
    redacted.push_str(&detail[cursor..]);
    redacted
}
