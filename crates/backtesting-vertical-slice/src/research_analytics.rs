//! Pure Research Analytics contract helpers.
//!
//! This module deliberately does not own a backtest runner, mutate source-proof
//! or BTE artifacts, touch SSM, or write runtime config. It validates that an
//! RA-owned verdicts live on `experiment-results` artifacts and materialize
//! sweep inputs for the existing BTE operator path.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde::{Deserialize, Serialize};

use crate::{
    artifact_index::LifecycleState,
    artifact_store::validate_artifact_root,
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    operator::{
        RESULT_CONTRACT_FILE, RunSpec, run_operator_from_run_spec,
        validate_local_run_spec_authority,
    },
    pinned_regular_file::{open_pinned_regular_file, read_exact_pinned_file},
    reference_artifact::{ReferenceArtifactRewrite, write_reference_artifact_with_len},
    result_contract::BacktestResultContract,
    retired_backfill_evidence::{
        read_active_backfill_runtime_input, resolve_active_backfill_runtime_input,
    },
    source_proof::SourceProofFidelityClass,
};

const RUN_POINTER_INDEX_REFERENCE_ARTIFACT_ROLE: &str = "run-pointer-index.v1";
const RESEARCH_ANALYTICS_KIND_PATH: &str = "research-analytics";
const RESEARCH_ANALYTICS_SCHEMA_VERSION: &str = "v1";
const RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY: &str = "experiment-results";
const RUN_POINTER_BACKTESTS_SUBPATH: &str = "backtests";
const RUN_POINTER_INDEX_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone)]
pub struct BacktestSweepSourcePair {
    pub run_spec_path: PathBuf,
    pub object_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct BacktestSweepPublicationPlan {
    pub input_dir: PathBuf,
    pub run_spec_dir: PathBuf,
    pub run_output_dir: PathBuf,
    pub artifact_root: String,
    pub index_path: PathBuf,
    pub sources: Vec<BacktestSweepSourcePair>,
}

#[derive(Debug, Clone)]
pub struct BacktestSweepIndexArtifact {
    pub path: PathBuf,
    pub content_hash: String,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct BacktestSweepPublicationReport {
    pub sweep_report: BacktestSweepReport,
    pub index: RunPointerIndex,
    pub index_artifact: BacktestSweepIndexArtifact,
}

#[derive(Debug, Clone)]
pub struct BacktestSweepPlan {
    pub run_spec_dir: PathBuf,
    pub run_output_dir: PathBuf,
    pub runs: Vec<BacktestSweepRun>,
}

#[derive(Debug, Clone)]
pub struct BacktestSweepRun {
    pub run_spec_file_name: String,
    pub output_dir_name: String,
    pub run_spec: RunSpec,
    pub accepted_object_bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BacktestSweepReport {
    pub runs: Vec<BacktestSweepRunReport>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BacktestSweepRunReport {
    pub run_id: String,
    pub run_spec_path: PathBuf,
    pub output_dir: PathBuf,
    pub result_contract_path: PathBuf,
    pub contract: BacktestResultContract,
}

pub trait BacktestRunCatalogList {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>>;
}

impl BacktestRunCatalogList for ParquetDataCatalog {
    fn list_backtest_runs(&self) -> anyhow::Result<Vec<String>> {
        ParquetDataCatalog::list_backtest_runs(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPointerResult {
    pub result_contract_uri: String,
    pub result_contract_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPointerIndexRecord {
    pub run_id: String,
    pub params: BTreeMap<String, serde_json::Value>,
    pub result: RunPointerResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPointerIndex {
    pub schema_version: u64,
    pub artifact_root: String,
    pub content_hash: String,
    pub runs: Vec<RunPointerIndexRecord>,
}

#[derive(Serialize)]
struct RunPointerIndexHashPayload<'a> {
    schema_version: u64,
    artifact_root: &'a str,
    runs: &'a [RunPointerIndexRecord],
}

impl RunPointerIndex {
    /// # Errors
    ///
    /// Returns an error when the index uses an unsupported schema version,
    /// contains invalid pointers, or has a stale `content_hash`.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == RUN_POINTER_INDEX_SCHEMA_VERSION,
            "run-pointer index schema_version must be {RUN_POINTER_INDEX_SCHEMA_VERSION}"
        );
        let artifact_root = validate_run_pointer_artifact_root(&self.artifact_root)?;
        ensure!(
            self.artifact_root == artifact_root,
            "run-pointer index artifact_root must be normalized without a trailing slash"
        );
        ensure!(
            is_lowercase_sha256_hex(&self.content_hash),
            "run-pointer index content_hash must be lowercase sha256 hex"
        );
        ensure!(
            !self.runs.is_empty(),
            "run-pointer index must include at least one catalog-listed run"
        );

        let mut seen = BTreeSet::new();
        for record in &self.runs {
            record.validate(&artifact_root)?;
            ensure!(
                seen.insert(record.run_id.clone()),
                "run-pointer index contains duplicate run_id {:?}",
                record.run_id
            );
        }
        ensure!(
            self.runs
                .windows(2)
                .all(|pair| pair[0].run_id.as_str() < pair[1].run_id.as_str()),
            "run-pointer index runs must be sorted by run_id"
        );
        ensure!(
            self.content_hash == self.expected_content_hash()?,
            "run-pointer index content_hash does not match payload"
        );
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the index cannot be serialized as structured JSON.
    pub fn expected_content_hash(&self) -> Result<String> {
        let artifact_root = validate_run_pointer_artifact_root(&self.artifact_root)?;
        let payload = RunPointerIndexHashPayload {
            schema_version: self.schema_version,
            artifact_root: &artifact_root,
            runs: &self.runs,
        };
        Ok(sha256_hex(&serde_json::to_vec(&payload)?))
    }
}

impl RunPointerIndexRecord {
    fn validate(&self, artifact_root: &str) -> Result<()> {
        ensure!(!self.run_id.trim().is_empty(), "run_id must not be empty");
        ensure!(
            !self.params.is_empty(),
            "run-pointer params for run_id {:?} must not be empty",
            self.run_id
        );
        for key in self.params.keys() {
            ensure!(
                !key.trim().is_empty(),
                "run-pointer params for run_id {:?} must not contain an empty key",
                self.run_id
            );
        }
        ensure!(
            !self.params.contains_key("lifecycle_state"),
            "run-pointer params must not carry lifecycle_state"
        );
        ensure!(
            !self.params.contains_key("promotion_config"),
            "run-pointer params must not carry promotion_config"
        );
        self.result.validate(artifact_root)
    }
}

impl RunPointerResult {
    fn validate(&self, artifact_root: &str) -> Result<()> {
        ensure!(
            self.result_contract_uri
                .starts_with(&format!("{artifact_root}/")),
            "result_contract_uri must live under artifact_root {artifact_root:?}"
        );
        ensure!(
            is_lowercase_sha256_hex(&self.result_contract_hash),
            "result_contract_hash must be lowercase sha256 hex"
        );
        Ok(())
    }
}

/// # Errors
///
/// Returns an error if the catalog cannot list backtest runs, the provided
/// records do not exactly cover that list, or any result pointer is invalid.
pub fn build_run_pointer_index_from_catalog<C: BacktestRunCatalogList>(
    catalog: &C,
    artifact_root: &str,
    records: Vec<RunPointerIndexRecord>,
) -> Result<RunPointerIndex> {
    build_run_pointer_index_from_catalog_list(catalog.list_backtest_runs()?, artifact_root, records)
}

/// # Errors
///
/// Returns an error if the listed run IDs and pointer records differ, contain
/// duplicates, or point outside the single artifact root.
pub fn build_run_pointer_index_from_catalog_list<I>(
    catalog_run_ids: I,
    artifact_root: &str,
    records: Vec<RunPointerIndexRecord>,
) -> Result<RunPointerIndex>
where
    I: IntoIterator,
    I::Item: Into<String>,
{
    let artifact_root = validate_run_pointer_artifact_root(artifact_root)?;
    let listed = exact_run_id_set(
        "catalog.list_backtest_runs",
        catalog_run_ids.into_iter().map(Into::into),
    )?;
    ensure!(
        !listed.is_empty(),
        "catalog.list_backtest_runs must include at least one run"
    );

    let indexed = exact_run_id_set(
        "run pointer records",
        records.iter().map(|record| record.run_id.clone()),
    )?;
    ensure!(
        listed == indexed,
        "run pointer records must exactly match catalog.list_backtest_runs"
    );

    let mut runs = records;
    runs.sort_by(|left, right| left.run_id.cmp(&right.run_id));
    for record in &runs {
        record.validate(&artifact_root)?;
    }

    let mut index = RunPointerIndex {
        schema_version: RUN_POINTER_INDEX_SCHEMA_VERSION,
        artifact_root,
        content_hash: String::new(),
        runs,
    };
    index.content_hash = index.expected_content_hash()?;
    index.validate()?;
    Ok(index)
}

/// # Errors
///
/// Returns an error if a run-spec cannot be materialized, the existing BTE
/// executor fails, or the persisted result contract is missing/invalid.
pub fn run_backtest_sweep(plan: &BacktestSweepPlan) -> Result<BacktestSweepReport> {
    run_backtest_sweep_with_executor(plan, |spec, accepted_object_bytes, output_dir| {
        run_operator_from_run_spec(spec, accepted_object_bytes, output_dir).map(|_| ())
    })
}

/// # Errors
///
/// Returns an error if a source pair cannot be loaded and verified, any sweep run
/// fails, the result contracts cannot form a valid run-pointer index, or the
/// index reference artifact cannot be written with `FailOnDirty` semantics.
pub fn run_backtest_sweep_publication(
    plan: &BacktestSweepPublicationPlan,
) -> Result<BacktestSweepPublicationReport> {
    run_backtest_sweep_publication_with_executor(plan, |spec, accepted_object_bytes, output_dir| {
        run_operator_from_run_spec(spec, accepted_object_bytes, output_dir).map(|_| ())
    })
}

/// # Errors
///
/// Returns an error if a source pair cannot be loaded and verified, the injected
/// executor fails, the result contracts cannot form a valid run-pointer index,
/// or the index reference artifact cannot be written with `FailOnDirty`
/// semantics.
pub fn run_backtest_sweep_publication_with_executor<F>(
    plan: &BacktestSweepPublicationPlan,
    executor: F,
) -> Result<BacktestSweepPublicationReport>
where
    F: FnMut(&RunSpec, &[u8], &Path) -> Result<()>,
{
    validate_run_pointer_artifact_root(&plan.artifact_root)?;
    let loaded_runs = load_backtest_sweep_source_pairs(plan)?;
    let (runs, loaded_sources): (Vec<_>, Vec<_>) = loaded_runs
        .into_iter()
        .map(|loaded| (loaded.run, loaded.source))
        .unzip();
    let sweep_plan = BacktestSweepPlan {
        run_spec_dir: plan.run_spec_dir.clone(),
        run_output_dir: plan.run_output_dir.clone(),
        runs,
    };
    let sweep_report = run_backtest_sweep_with_executor(&sweep_plan, executor)?;
    let records = run_pointer_records_from_sweep(&sweep_report, &loaded_sources)?;
    let catalog_run_ids = sweep_report
        .runs
        .iter()
        .map(|run| run.run_id.clone())
        .collect::<Vec<_>>();
    let index =
        build_run_pointer_index_from_catalog_list(catalog_run_ids, &plan.artifact_root, records)?;
    let index_artifact_write = write_reference_artifact_with_len(
        &plan.index_path,
        RUN_POINTER_INDEX_REFERENCE_ARTIFACT_ROLE,
        &index,
        ReferenceArtifactRewrite::FailOnDirty,
    )?;
    Ok(BacktestSweepPublicationReport {
        sweep_report,
        index,
        index_artifact: BacktestSweepIndexArtifact {
            path: index_artifact_write.pin.path,
            content_hash: index_artifact_write.pin.sha256,
            bytes: index_artifact_write.bytes,
        },
    })
}

#[derive(Debug, Clone, Copy)]
struct BacktestSweepRunPreflight<'a> {
    run_spec_file_name: &'a str,
    output_dir_name: &'a str,
    run_spec: &'a RunSpec,
}

#[derive(Debug, Clone)]
struct BacktestSweepMaterializationTargets {
    run_spec_path: PathBuf,
    output_dir: PathBuf,
}

fn validate_backtest_sweep_runs(
    run_spec_dir: &Path,
    run_output_dir: &Path,
    runs: &[BacktestSweepRunPreflight<'_>],
) -> Result<Vec<BacktestSweepMaterializationTargets>> {
    ensure!(!runs.is_empty(), "sweep must include at least one run");
    ensure!(
        run_spec_dir.is_absolute() == run_output_dir.is_absolute(),
        "run-spec and output target roots must use the same absolute or relative anchoring"
    );

    // Prove the authority boundary for the complete plan before inspecting
    // any materialization target owned by the caller.
    for run in runs {
        validate_local_run_spec_authority(run.run_spec).with_context(|| {
            format!(
                "validate local sweep authority for {}",
                run.run_spec.manifest.run_id
            )
        })?;
    }

    let mut seen_run_spec_file_names = BTreeSet::new();
    let mut seen_output_dir_names = BTreeSet::new();
    let mut materialization_targets = Vec::with_capacity(runs.len());
    for run in runs {
        validate_run_spec_file_name(run.run_spec_file_name)?;
        validate_leaf_path("output_dir_name", run.output_dir_name)?;
        ensure!(
            run.run_spec.accepted_object.bytes > 0,
            "accepted_object.bytes for run {} must be positive",
            run.run_spec.manifest.run_id
        );
        ensure!(
            is_lowercase_sha256_hex(&run.run_spec.accepted_object.sha256),
            "accepted_object.sha256 for run {} must be exactly 64 lowercase hexadecimal characters",
            run.run_spec.manifest.run_id
        );
        ensure_object_read_within_raw_payload_limit(run.run_spec)?;
        ensure!(
            seen_run_spec_file_names.insert(run.run_spec_file_name.to_string()),
            "duplicate run_spec_file_name {:?}",
            run.run_spec_file_name
        );
        ensure!(
            seen_output_dir_names.insert(run.output_dir_name.to_string()),
            "duplicate output_dir_name {:?}",
            run.output_dir_name
        );

        let run_spec_path = run_spec_dir.join(run.run_spec_file_name);
        let output_dir = run_output_dir.join(run.output_dir_name);
        validate_planned_target_path("run-spec target", &run_spec_path)?;
        validate_planned_target_path("output target", &output_dir)?;
        materialization_targets.push(BacktestSweepMaterializationTargets {
            run_spec_path,
            output_dir,
        });
    }
    ensure_materialization_targets_disjoint(&materialization_targets)?;
    Ok(materialization_targets)
}

fn validate_planned_target_path(label: &'static str, path: &Path) -> Result<()> {
    ensure!(!path.as_os_str().is_empty(), "{label} must not be empty");
    ensure!(
        path.components().all(|component| matches!(
            component,
            Component::Prefix(_) | Component::RootDir | Component::Normal(_)
        )),
        "{label} must be lexically normalized without current or parent components: {}",
        path.display()
    );
    Ok(())
}

fn planned_paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn ensure_materialization_targets_disjoint(
    targets: &[BacktestSweepMaterializationTargets],
) -> Result<()> {
    let flattened = targets
        .iter()
        .flat_map(|target| {
            [
                ("run-spec target", target.run_spec_path.as_path()),
                ("output target", target.output_dir.as_path()),
            ]
        })
        .collect::<Vec<_>>();
    for (index, (left_label, left_path)) in flattened.iter().enumerate() {
        for (right_label, right_path) in flattened.iter().skip(index + 1) {
            ensure!(
                !planned_paths_overlap(left_path, right_path),
                "planned filesystem targets overlap: {left_label} {} and {right_label} {}",
                left_path.display(),
                right_path.display()
            );
        }
    }
    Ok(())
}

fn ensure_index_target_disjoint_and_absent(
    index_path: &Path,
    targets: &[BacktestSweepMaterializationTargets],
) -> Result<()> {
    validate_planned_target_path("index target", index_path)?;
    if let Some(first_target) = targets.first() {
        ensure!(
            index_path.is_absolute() == first_target.run_spec_path.is_absolute(),
            "index and materialization targets must use the same absolute or relative anchoring"
        );
    }
    for target in targets {
        for (label, path) in [
            ("run-spec target", target.run_spec_path.as_path()),
            ("output target", target.output_dir.as_path()),
        ] {
            ensure!(
                !planned_paths_overlap(index_path, path),
                "planned filesystem targets overlap: index target {} and {label} {}",
                index_path.display(),
                path.display()
            );
        }
    }
    ensure!(
        !index_path
            .try_exists()
            .with_context(|| format!("check index path {}", index_path.display()))?,
        "index path already exists: {}",
        index_path.display()
    );
    Ok(())
}

fn ensure_backtest_sweep_targets_absent(
    targets: &[BacktestSweepMaterializationTargets],
) -> Result<()> {
    for target in targets {
        ensure!(
            !target.run_spec_path.try_exists().with_context(|| {
                format!("check run-spec path {}", target.run_spec_path.display())
            })?,
            "run-spec path already exists: {}",
            target.run_spec_path.display()
        );
        ensure!(
            !target
                .output_dir
                .try_exists()
                .with_context(|| format!("check output_dir {}", target.output_dir.display()))?,
            "output_dir already exists: {}",
            target.output_dir.display()
        );
    }
    Ok(())
}

fn validate_backtest_sweep_payloads(runs: &[BacktestSweepRun]) -> Result<()> {
    for run in runs {
        ensure!(
            !run.accepted_object_bytes.is_empty(),
            "accepted_object_bytes for run {} must not be empty",
            run.run_spec.manifest.run_id
        );
        let actual_bytes = u64::try_from(run.accepted_object_bytes.len())
            .context("accepted object byte length exceeds u64")?;
        ensure!(
            actual_bytes == run.run_spec.accepted_object.bytes,
            "accepted object byte length {actual_bytes} does not match run-spec {} for run {}",
            run.run_spec.accepted_object.bytes,
            run.run_spec.manifest.run_id
        );
        let actual_sha256 = sha256_hex(&run.accepted_object_bytes);
        ensure!(
            actual_sha256 == run.run_spec.accepted_object.sha256,
            "accepted object SHA-256 {actual_sha256} does not match run-spec {} for run {}",
            run.run_spec.accepted_object.sha256,
            run.run_spec.manifest.run_id
        );
    }
    Ok(())
}

/// # Errors
///
/// Returns an error if a run-spec cannot be materialized, the provided BTE
/// executor fails, or the persisted result contract is missing/invalid.
pub fn run_backtest_sweep_with_executor<F>(
    plan: &BacktestSweepPlan,
    mut executor: F,
) -> Result<BacktestSweepReport>
where
    F: FnMut(&RunSpec, &[u8], &Path) -> Result<()>,
{
    let preflight_runs = plan
        .runs
        .iter()
        .map(|run| BacktestSweepRunPreflight {
            run_spec_file_name: &run.run_spec_file_name,
            output_dir_name: &run.output_dir_name,
            run_spec: &run.run_spec,
        })
        .collect::<Vec<_>>();
    let materialization_targets =
        validate_backtest_sweep_runs(&plan.run_spec_dir, &plan.run_output_dir, &preflight_runs)?;
    ensure_backtest_sweep_targets_absent(&materialization_targets)?;
    validate_backtest_sweep_payloads(&plan.runs)?;

    fs::create_dir_all(&plan.run_spec_dir)
        .with_context(|| format!("create run-spec dir {}", plan.run_spec_dir.display()))?;
    fs::create_dir_all(&plan.run_output_dir)
        .with_context(|| format!("create run-output dir {}", plan.run_output_dir.display()))?;

    let mut reports = Vec::with_capacity(plan.runs.len());
    for run in &plan.runs {
        let run_spec_path = plan.run_spec_dir.join(&run.run_spec_file_name);
        let output_dir = plan.run_output_dir.join(&run.output_dir_name);
        fs::create_dir(&output_dir)
            .with_context(|| format!("create run output dir {}", output_dir.display()))?;

        let run_spec_toml =
            toml::to_string_pretty(&run.run_spec).context("serialize typed run-spec TOML")?;
        let mut run_spec_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&run_spec_path)
            .with_context(|| format!("create run-spec {}", run_spec_path.display()))?;
        run_spec_file
            .write_all(run_spec_toml.as_bytes())
            .with_context(|| format!("write run-spec {}", run_spec_path.display()))?;

        executor(&run.run_spec, &run.accepted_object_bytes, &output_dir)
            .with_context(|| format!("execute BTE run {}", run.run_spec.manifest.run_id))?;

        let result_contract_path = output_dir.join(RESULT_CONTRACT_FILE);
        let contract = read_result_contract(&result_contract_path)?;
        contract
            .validate()
            .with_context(|| format!("validate {}", result_contract_path.display()))?;
        ensure!(
            contract.run_id == run.run_spec.manifest.run_id,
            "result contract run_id {:?} does not match run-spec run_id {:?}",
            contract.run_id,
            run.run_spec.manifest.run_id
        );
        validate_result_contract_matches_run(&contract, run, &result_contract_path)?;

        reports.push(BacktestSweepRunReport {
            run_id: run.run_spec.manifest.run_id.clone(),
            run_spec_path,
            output_dir,
            result_contract_path,
            contract,
        });
    }

    Ok(BacktestSweepReport { runs: reports })
}

#[derive(Debug, Clone)]
struct LoadedBacktestSweepRun {
    run: BacktestSweepRun,
    source: LoadedBacktestSweepSource,
}

#[derive(Debug, Clone)]
struct LoadedBacktestSweepSource {
    run_id: String,
    params: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
struct PreflightedBacktestSweepSource {
    source_run_spec_path: String,
    source_object_path: PathBuf,
    source_run_spec_sha256: String,
    run_spec_file_name: String,
    run_spec: RunSpec,
}

#[derive(Debug, Clone)]
struct ResolvedBacktestSweepSource {
    preflighted: PreflightedBacktestSweepSource,
    object_path: PathBuf,
    params: BTreeMap<String, serde_json::Value>,
}

/// # Errors
///
/// Returns an error if the run-spec TOML cannot be read or parsed.
pub fn read_run_spec_with_hash(path: &Path) -> Result<(RunSpec, String)> {
    let bytes = read_active_backfill_runtime_input(None, path)?;
    let hash = sha256_hex(&bytes);
    let text = std::str::from_utf8(&bytes).context("run-spec TOML is not UTF-8")?;
    let mut spec: RunSpec = toml::from_str(text).context("parse run-spec TOML")?;
    if spec.source_bindings_path.is_relative() {
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let sibling_relative = base_dir.join(&spec.source_bindings_path);
        if sibling_relative.exists() {
            spec.source_bindings_path = sibling_relative;
        }
    }
    Ok((spec, hash))
}

/// # Errors
///
/// Returns an error if the object file cannot be read, does not match the
/// run-spec byte count, exceeds the raw payload read limit, or has the wrong
/// SHA-256.
pub fn read_accepted_object_for_run_spec(path: &Path, spec: &RunSpec) -> Result<Vec<u8>> {
    ensure!(
        spec.accepted_object.bytes > 0,
        "accepted_object.bytes must be positive"
    );
    ensure!(
        is_lowercase_sha256_hex(&spec.accepted_object.sha256),
        "accepted_object.sha256 must be exactly 64 lowercase hexadecimal characters"
    );
    ensure_object_read_within_raw_payload_limit(spec)?;
    let resolved_path = resolve_active_backfill_runtime_input(None, path)?;
    let (mut file, identity) = open_pinned_regular_file(&resolved_path)
        .with_context(|| format!("open accepted object {}", path.display()))?;
    ensure!(
        identity.byte_len == spec.accepted_object.bytes,
        "object byte length {} does not match run-spec {}",
        identity.byte_len,
        spec.accepted_object.bytes
    );
    identity.revalidate(&resolved_path, &file)?;
    let bytes = read_exact_pinned_file(&mut file, &resolved_path, spec.accepted_object.bytes)?;
    let actual_sha256 = sha256_hex(&bytes);
    ensure!(
        actual_sha256 == spec.accepted_object.sha256,
        "object SHA-256 {actual_sha256} does not match run-spec {}",
        spec.accepted_object.sha256
    );
    identity.revalidate(&resolved_path, &file)?;
    Ok(bytes)
}

fn preflight_accepted_object_for_run_spec(path: &Path, spec: &RunSpec) -> Result<PathBuf> {
    ensure_object_read_within_raw_payload_limit(spec)?;
    let resolved_path = resolve_active_backfill_runtime_input(None, path)?;
    let metadata =
        fs::metadata(&resolved_path).with_context(|| format!("stat object {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "object path must be a regular file: {}",
        path.display()
    );
    let actual_bytes = metadata.len();
    ensure!(
        actual_bytes == spec.accepted_object.bytes,
        "object byte length {actual_bytes} does not match run-spec {}",
        spec.accepted_object.bytes
    );
    Ok(resolved_path)
}

/// # Errors
///
/// Returns an error when the run-spec accepted object byte count exceeds the
/// configured raw-payload read limit.
pub fn ensure_object_read_within_raw_payload_limit(spec: &RunSpec) -> Result<()> {
    ensure!(
        spec.accepted_object.bytes <= spec.converter.raw_payload.max_object_bytes,
        "accepted_object.bytes {} exceeds converter.raw_payload.max_object_bytes {}",
        spec.accepted_object.bytes,
        spec.converter.raw_payload.max_object_bytes
    );
    Ok(())
}

fn load_backtest_sweep_source_pairs(
    plan: &BacktestSweepPublicationPlan,
) -> Result<Vec<LoadedBacktestSweepRun>> {
    ensure_publication_input_dir(&plan.input_dir)?;
    let mut input_entries = fs::read_dir(&plan.input_dir)
        .with_context(|| format!("read input_dir {}", plan.input_dir.display()))?;
    ensure!(
        input_entries
            .next()
            .transpose()
            .with_context(|| format!("read input_dir {}", plan.input_dir.display()))?
            .is_some(),
        "input_dir must not be empty: {}",
        plan.input_dir.display()
    );
    ensure!(
        !plan.sources.is_empty(),
        "sweep source pairs must not be empty"
    );

    let mut source_run_spec_paths = BTreeSet::new();
    let mut source_object_paths = BTreeSet::new();
    for source in &plan.sources {
        let run_spec_path = input_relative_source_path("run-spec", &source.run_spec_path)?;
        let object_path = input_relative_source_path("object", &source.object_path)?;
        ensure!(
            source_run_spec_paths.insert(run_spec_path.clone()),
            "duplicate source run-spec path {run_spec_path:?}"
        );
        source_object_paths.insert(object_path);
    }
    for shared_path in source_run_spec_paths.intersection(&source_object_paths) {
        ensure!(
            false,
            "source path {shared_path:?} cannot serve as both a run-spec control and accepted-object payload"
        );
    }

    let mut preflighted_sources = Vec::with_capacity(plan.sources.len());
    for source in &plan.sources {
        let (run_spec_path, source_run_spec_path) =
            resolve_source_path("run-spec", &plan.input_dir, &source.run_spec_path)?;
        let (run_spec, source_run_spec_sha256) = read_run_spec_with_hash(&run_spec_path)?;
        let run_spec_file_name = source_file_name("run-spec", &run_spec_path)?;
        preflighted_sources.push(PreflightedBacktestSweepSource {
            source_run_spec_path,
            source_object_path: source.object_path.clone(),
            source_run_spec_sha256,
            run_spec_file_name,
            run_spec,
        });
    }

    let sweep_preflight = preflighted_sources
        .iter()
        .map(|source| BacktestSweepRunPreflight {
            run_spec_file_name: &source.run_spec_file_name,
            output_dir_name: &source.run_spec.manifest.run_id,
            run_spec: &source.run_spec,
        })
        .collect::<Vec<_>>();
    let materialization_targets =
        validate_backtest_sweep_runs(&plan.run_spec_dir, &plan.run_output_dir, &sweep_preflight)?;

    let mut seen_output_prefixes = BTreeSet::new();
    for source in &preflighted_sources {
        let output_prefix =
            validate_publication_run_spec_artifact_scope(&plan.artifact_root, &source.run_spec)?;
        // The current run-id-scoped prefix rule makes this redundant, but keep
        // the remote-prefix uniqueness invariant explicit if that scope changes.
        ensure!(
            seen_output_prefixes.insert(output_prefix.clone()),
            "duplicate manifest.output_prefix {output_prefix:?}"
        );
    }
    ensure_index_target_disjoint_and_absent(&plan.index_path, &materialization_targets)?;
    ensure_backtest_sweep_targets_absent(&materialization_targets)?;

    // Resolve and stat every object before reading any payload. A later
    // missing, aliased, non-regular, oversized, or wrong-length object cannot
    // waste an earlier large read. The read phase repeats these checks to
    // detect changes at the available pathname boundary.
    let mut resolved_sources = Vec::with_capacity(preflighted_sources.len());
    for preflighted in preflighted_sources {
        let (object_path, source_object_path) =
            resolve_source_path("object", &plan.input_dir, &preflighted.source_object_path)?;
        let params = run_pointer_params(
            &preflighted.run_spec,
            preflighted.source_run_spec_path.clone(),
            source_object_path,
            preflighted.source_run_spec_sha256.clone(),
        )?;
        preflight_accepted_object_for_run_spec(&object_path, &preflighted.run_spec)?;
        resolved_sources.push(ResolvedBacktestSweepSource {
            preflighted,
            object_path,
            params,
        });
    }

    let mut loaded_runs = Vec::with_capacity(resolved_sources.len());
    for resolved in resolved_sources {
        let accepted_object_bytes = read_accepted_object_for_run_spec(
            &resolved.object_path,
            &resolved.preflighted.run_spec,
        )?;
        let preflighted = resolved.preflighted;
        let run_id = preflighted.run_spec.manifest.run_id.clone();
        loaded_runs.push(LoadedBacktestSweepRun {
            run: BacktestSweepRun {
                run_spec_file_name: preflighted.run_spec_file_name,
                output_dir_name: run_id.clone(),
                run_spec: preflighted.run_spec,
                accepted_object_bytes,
            },
            source: LoadedBacktestSweepSource {
                run_id,
                params: resolved.params,
            },
        });
    }
    Ok(loaded_runs)
}

fn ensure_publication_input_dir(input_dir: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(input_dir)
        .with_context(|| format!("stat input_dir {}", input_dir.display()))?;
    ensure!(
        !metadata.file_type().is_symlink(),
        "input_dir must not be a symlink: {}",
        input_dir.display()
    );
    ensure!(
        metadata.file_type().is_dir(),
        "input_dir must be an existing directory: {}",
        input_dir.display()
    );
    Ok(())
}

fn resolve_source_path(
    label: &'static str,
    input_dir: &Path,
    path: &Path,
) -> Result<(PathBuf, String)> {
    let input_relative_path = input_relative_source_path(label, path)?;
    ensure_source_path_has_no_symlink_components(label, input_dir, path)?;
    Ok((input_dir.join(path), input_relative_path))
}

fn ensure_source_path_has_no_symlink_components(
    label: &'static str,
    input_dir: &Path,
    path: &Path,
) -> Result<()> {
    let mut current = input_dir.to_path_buf();
    let mut final_metadata = None;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                current.push(part);
                let metadata = fs::symlink_metadata(&current)
                    .with_context(|| format!("stat {label} source path {}", current.display()))?;
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "{label} source path must not contain symlinks: {}",
                    path.display()
                );
                final_metadata = Some(metadata);
            }
            _ => {
                ensure!(
                    false,
                    "{label} source path must be relative to input_dir without parent or prefix components: {}",
                    path.display()
                );
            }
        }
    }
    let metadata =
        final_metadata.with_context(|| format!("{label} source path must not be empty"))?;
    ensure!(
        metadata.file_type().is_file(),
        "{label} source path must be a regular file: {}",
        path.display()
    );
    Ok(())
}

fn input_relative_source_path(label: &'static str, path: &Path) -> Result<String> {
    ensure!(
        !path.as_os_str().is_empty(),
        "{label} source path must not be empty"
    );
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_str().with_context(|| {
                    format!("{label} source path must be UTF-8: {}", path.display())
                })?;
                parts.push(part.to_string());
            }
            _ => {
                ensure!(
                    false,
                    "{label} source path must be relative to input_dir without parent or prefix components: {}",
                    path.display()
                );
            }
        }
    }
    ensure!(
        !parts.is_empty(),
        "{label} source path must include at least one path component"
    );
    Ok(parts.join("/"))
}

fn validate_publication_run_spec_artifact_scope(
    artifact_root: &str,
    run_spec: &RunSpec,
) -> Result<String> {
    let artifact_root = validate_run_pointer_artifact_root(artifact_root)?;
    let manifest_root = validate_run_pointer_artifact_root(&run_spec.manifest.artifact_root)
        .with_context(|| {
            format!(
                "run spec {} manifest.artifact_root",
                run_spec.manifest.run_id
            )
        })?;
    ensure!(
        manifest_root == artifact_root,
        "run spec {} manifest.artifact_root {:?} must match publication artifact_root {:?}",
        run_spec.manifest.run_id,
        run_spec.manifest.artifact_root,
        artifact_root
    );
    validate_leaf_path("manifest.run_id", &run_spec.manifest.run_id)?;
    ensure!(
        run_spec.manifest.output_prefix == run_spec.manifest.output_prefix.trim(),
        "run spec {} manifest.output_prefix must not contain leading or trailing whitespace",
        run_spec.manifest.run_id
    );
    let output_prefix = run_spec.manifest.output_prefix.trim_end_matches('/');
    ensure!(
        output_prefix == run_spec.manifest.output_prefix,
        "run spec {} manifest.output_prefix {:?} must be normalized without a trailing slash",
        run_spec.manifest.run_id,
        run_spec.manifest.output_prefix
    );
    let expected_prefix = format!(
        "{artifact_root}/{RUN_POINTER_BACKTESTS_SUBPATH}/{}",
        run_spec.manifest.run_id
    );
    ensure!(
        output_prefix == expected_prefix,
        "run spec {} manifest.output_prefix {:?} must equal {:?}",
        run_spec.manifest.run_id,
        run_spec.manifest.output_prefix,
        expected_prefix
    );
    Ok(output_prefix.to_string())
}

fn source_file_name(label: &'static str, path: &Path) -> Result<String> {
    let file_name = path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .with_context(|| {
            format!(
                "{label} path must have a UTF-8 file name: {}",
                path.display()
            )
        })?;
    Ok(file_name.to_string())
}

fn run_pointer_records_from_sweep(
    sweep_report: &BacktestSweepReport,
    loaded_sources: &[LoadedBacktestSweepSource],
) -> Result<Vec<RunPointerIndexRecord>> {
    let mut reports_by_run_id = BTreeMap::new();
    for run in &sweep_report.runs {
        ensure!(
            reports_by_run_id.insert(run.run_id.clone(), run).is_none(),
            "sweep report contains duplicate run_id {:?}",
            run.run_id
        );
    }

    loaded_sources
        .iter()
        .map(|loaded| {
            let report = reports_by_run_id
                .get(&loaded.run_id)
                .with_context(|| format!("sweep report missing run_id {:?}", loaded.run_id))?;
            let result_contract_bytes =
                fs::read(&report.result_contract_path).with_context(|| {
                    format!(
                        "read result contract {}",
                        report.result_contract_path.display()
                    )
                })?;
            Ok(RunPointerIndexRecord {
                run_id: loaded.run_id.clone(),
                params: loaded.params.clone(),
                result: RunPointerResult {
                    result_contract_uri: report.contract.artifact_uris.result_contract_uri.clone(),
                    result_contract_hash: sha256_hex(&result_contract_bytes),
                },
            })
        })
        .collect()
}

fn run_pointer_params(
    run_spec: &RunSpec,
    source_run_spec_path: String,
    source_object_path: String,
    source_run_spec_sha256: String,
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut params = BTreeMap::new();
    params.insert(
        "source_run_spec_sha256".to_string(),
        serde_json::Value::String(source_run_spec_sha256),
    );
    params.insert(
        "accepted_object_sha256".to_string(),
        serde_json::Value::String(run_spec.accepted_object.sha256.clone()),
    );
    params.insert(
        "strategy_config_hash".to_string(),
        serde_json::Value::String(run_spec.manifest.strategy_config_hash.clone()),
    );
    params.insert(
        "converter_config_hash".to_string(),
        serde_json::Value::String(
            run_spec
                .converter
                .content_hash()
                .context("hash run-spec converter config")?,
        ),
    );
    params.insert(
        "source_run_spec_path".to_string(),
        serde_json::Value::String(source_run_spec_path),
    );
    params.insert(
        "source_object_path".to_string(),
        serde_json::Value::String(source_object_path),
    );
    Ok(params)
}

fn read_result_contract(path: &Path) -> Result<BacktestResultContract> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

fn validate_result_contract_matches_run(
    contract: &BacktestResultContract,
    run: &BacktestSweepRun,
    result_contract_path: &Path,
) -> Result<()> {
    let expected_manifest_hash = run.run_spec.manifest.manifest_hash();
    ensure!(
        contract.manifest_hash == expected_manifest_hash,
        "{} manifest_hash {:?} does not match run-spec manifest hash {:?}",
        result_contract_path.display(),
        contract.manifest_hash,
        expected_manifest_hash
    );

    let expected_accepted_object_sha256 = &run.run_spec.accepted_object.sha256;
    ensure!(
        contract.accepted_object_sha256.as_str() == expected_accepted_object_sha256.as_str(),
        "{} accepted_object_sha256 {:?} does not match accepted object bytes {:?}",
        result_contract_path.display(),
        contract.accepted_object_sha256,
        expected_accepted_object_sha256
    );

    ensure!(
        contract.strategy_config_hash == run.run_spec.manifest.strategy_config_hash,
        "{} strategy_config_hash {:?} does not match run-spec strategy_config_hash {:?}",
        result_contract_path.display(),
        contract.strategy_config_hash,
        run.run_spec.manifest.strategy_config_hash
    );

    let expected_converter_config_hash = run
        .run_spec
        .converter
        .content_hash()
        .context("hash run-spec converter config")?;
    ensure!(
        contract.converter_config_hash == expected_converter_config_hash,
        "{} converter_config_hash {:?} does not match run-spec converter config hash {:?}",
        result_contract_path.display(),
        contract.converter_config_hash,
        expected_converter_config_hash
    );

    ensure!(
        contract.source_proof_id == run.run_spec.manifest.source_proof_id,
        "{} source_proof_id {:?} does not match manifest source_proof_id {:?}",
        result_contract_path.display(),
        contract.source_proof_id,
        run.run_spec.manifest.source_proof_id
    );
    ensure!(
        contract.source_proof_version == run.run_spec.manifest.source_proof_version,
        "{} source_proof_version {} does not match manifest source_proof_version {}",
        result_contract_path.display(),
        contract.source_proof_version,
        run.run_spec.manifest.source_proof_version
    );
    ensure!(
        contract.nt_version == run.run_spec.manifest.resolved_nt_version,
        "{} nt_version {:?} does not match manifest resolved_nt_version {:?}",
        result_contract_path.display(),
        contract.nt_version,
        run.run_spec.manifest.resolved_nt_version
    );

    let expected_result_contract_uri = format!(
        "{}/{}",
        run.run_spec.manifest.output_prefix.trim_end_matches('/'),
        RESULT_CONTRACT_FILE
    );
    ensure!(
        contract.artifact_uris.result_contract_uri == expected_result_contract_uri,
        "{} result_contract_uri {:?} does not match expected {:?}",
        result_contract_path.display(),
        contract.artifact_uris.result_contract_uri,
        expected_result_contract_uri
    );

    Ok(())
}

fn validate_run_pointer_artifact_root(artifact_root: &str) -> Result<String> {
    ensure!(
        !artifact_root.trim().is_empty(),
        "artifact_root must not be empty"
    );
    ensure!(
        artifact_root == artifact_root.trim(),
        "artifact_root must not contain leading or trailing whitespace"
    );
    ensure!(
        artifact_root == artifact_root.trim_end_matches('/'),
        "artifact_root must be normalized without a trailing slash"
    );
    validate_artifact_root(artifact_root)
}

fn exact_run_id_set(
    source: &'static str,
    run_ids: impl Iterator<Item = String>,
) -> Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    for run_id in run_ids {
        ensure!(
            !run_id.trim().is_empty(),
            "{source} must not include an empty run_id"
        );
        ensure!(
            set.insert(run_id.clone()),
            "{source} includes duplicate run_id {run_id:?}"
        );
    }
    Ok(set)
}

fn validate_run_spec_file_name(value: &str) -> Result<()> {
    validate_leaf_path("run_spec_file_name", value)?;
    ensure!(
        Path::new(value)
            .extension()
            .is_some_and(|ext| ext == "toml"),
        "run_spec_file_name {value:?} must use .toml"
    );
    Ok(())
}

fn validate_leaf_path(field: &'static str, value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{field} must not be empty");
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "{field} must be relative");
    let mut components = path.components();
    ensure!(
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none(),
        "{field} must be a single relative path segment"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RaVerdictKind {
    Go,
    NoGo,
    ConditionalGo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForbiddenPromotionAction {
    AutoMerge,
    AutoEnableStrategy,
    ScheduleLiveTrading,
    TouchSsmCredentials,
    MutateProductionRuntimeConfig,
}

impl ForbiddenPromotionAction {
    const fn description(self) -> &'static str {
        match self {
            Self::AutoMerge => "auto-merge",
            Self::AutoEnableStrategy => "auto-enable strategy",
            Self::ScheduleLiveTrading => "schedule live trading",
            Self::TouchSsmCredentials => "touch SSM credentials",
            Self::MutateProductionRuntimeConfig => "mutate production runtime config",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProofEvidenceRef {
    pub source_proof_id: String,
    pub source_proof_version: Option<u64>,
    pub source_proof_report_uri: String,
    pub source_proof_report_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub accepted: bool,
}

impl SourceProofEvidenceRef {
    fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_non_empty("source_proof_id", &self.source_proof_id)?;
        validate_non_empty("source_proof_report_uri", &self.source_proof_report_uri)?;
        validate_sha256("source_proof_report_hash", &self.source_proof_report_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestEvidenceRef {
    pub result_contract_id: String,
    pub result_contract_uri: String,
    pub result_contract_hash: String,
    pub objective: bool,
}

impl BacktestEvidenceRef {
    fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_non_empty("result_contract_id", &self.result_contract_id)?;
        validate_non_empty("result_contract_uri", &self.result_contract_uri)?;
        validate_sha256("result_contract_hash", &self.result_contract_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPointerRef {
    pub uri: String,
    pub sha256: String,
}

impl ArtifactPointerRef {
    fn validate(&self, field: &'static str) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_non_empty(field, &self.uri)?;
        validate_sha256(field, &self.sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaVerdict {
    pub verdict: RaVerdictKind,
    pub scope: String,
    pub source_proof_refs: Vec<SourceProofEvidenceRef>,
    pub backtest_result_refs: Vec<BacktestEvidenceRef>,
    pub evidence_report_refs: Vec<ArtifactPointerRef>,
    pub requested_claim_fidelity: SourceProofFidelityClass,
    pub preserved_claim_limits: Vec<String>,
    pub remeasurement_cadence: String,
    pub recorded_at: String,
    pub recorded_by: String,
}

impl RaVerdict {
    fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_non_empty("verdict.scope", &self.scope)?;
        validate_non_empty("verdict.remeasurement_cadence", &self.remeasurement_cadence)?;
        validate_non_empty("verdict.recorded_at", &self.recorded_at)?;
        validate_non_empty("verdict.recorded_by", &self.recorded_by)?;
        ensure_non_empty("verdict.source_proof_refs", &self.source_proof_refs)?;
        ensure_non_empty("verdict.backtest_result_refs", &self.backtest_result_refs)?;
        ensure_non_empty("verdict.evidence_report_refs", &self.evidence_report_refs)?;
        ensure_non_empty(
            "verdict.preserved_claim_limits",
            &self.preserved_claim_limits,
        )?;
        for source_ref in &self.source_proof_refs {
            source_ref.validate()?;
            if !source_fidelity_supports_claim(
                source_ref.fidelity_class,
                self.requested_claim_fidelity,
            ) {
                return Err(ResearchAnalyticsArtifactError::IncompatibleClaimFidelity {
                    source_fidelity: source_ref.fidelity_class,
                    requested_fidelity: self.requested_claim_fidelity,
                });
            }
        }
        for backtest_ref in &self.backtest_result_refs {
            backtest_ref.validate()?;
        }
        for evidence_ref in &self.evidence_report_refs {
            evidence_ref.validate("verdict.evidence_report_refs")?;
        }
        for claim_limit in &self.preserved_claim_limits {
            validate_non_empty("verdict.preserved_claim_limits", claim_limit)?;
        }
        if self.verdict == RaVerdictKind::Go && !self.is_real_go_finding() {
            return Err(ResearchAnalyticsArtifactError::PromotionConfigRequiresGo);
        }
        Ok(())
    }

    fn is_real_go_finding(&self) -> bool {
        self.verdict == RaVerdictKind::Go
            && self
                .source_proof_refs
                .iter()
                .all(|source_ref| source_ref.accepted)
            && self
                .backtest_result_refs
                .iter()
                .all(|backtest_ref| backtest_ref.objective)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromotionConfigRef {
    pub typed_config_uri: String,
    pub typed_config_hash: String,
    pub reviewer_policy_refs: Vec<String>,
    pub non_live_boundary: bool,
}

impl PromotionConfigRef {
    fn validate(&self, artifact_root: &str) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_experiment_results_uri(
            "promotion_config.typed_config_uri",
            artifact_root,
            &self.typed_config_uri,
        )?;
        validate_sha256(
            "promotion_config.typed_config_hash",
            &self.typed_config_hash,
        )?;
        ensure_non_empty(
            "promotion_config.reviewer_policy_refs",
            &self.reviewer_policy_refs,
        )?;
        for reviewer_ref in &self.reviewer_policy_refs {
            validate_non_empty("promotion_config.reviewer_policy_refs", reviewer_ref)?;
        }
        if !self.non_live_boundary {
            return Err(ResearchAnalyticsArtifactError::PromotionConfigMissing {
                missing: "explicit non-live boundary",
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentResultArtifact {
    pub artifact_schema_version: u64,
    pub artifact_id: String,
    pub artifact_root: String,
    pub artifact_uri: String,
    pub owner: String,
    pub source_refs: Vec<String>,
    pub source_hashes: Vec<String>,
    pub content_hash: String,
    pub lifecycle_state: LifecycleState,
    pub verdict: RaVerdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_config: Option<PromotionConfigRef>,
    pub dashboard_field_refs: Vec<String>,
    pub notebook_runtime_code_refs: Vec<String>,
    pub accepts_source_proofs: bool,
    pub mutates_source_proofs: bool,
    pub mutates_backtest_result_contracts: bool,
    pub weakens_forbidden_claims: bool,
    pub post_verdict_actions: Vec<ForbiddenPromotionAction>,
}

#[derive(Serialize)]
struct ExperimentResultArtifactHashPayload<'a> {
    artifact_schema_version: u64,
    artifact_id: &'a str,
    artifact_root: &'a str,
    artifact_uri: &'a str,
    owner: &'a str,
    source_refs: &'a [String],
    source_hashes: &'a [String],
    lifecycle_state: &'a LifecycleState,
    verdict: &'a RaVerdict,
    promotion_config: Option<&'a PromotionConfigRef>,
    dashboard_field_refs: &'a [String],
    notebook_runtime_code_refs: &'a [String],
    accepts_source_proofs: bool,
    mutates_source_proofs: bool,
    mutates_backtest_result_contracts: bool,
    weakens_forbidden_claims: bool,
    post_verdict_actions: &'a [ForbiddenPromotionAction],
}

impl ExperimentResultArtifact {
    #[must_use]
    pub fn expected_content_hash(&self) -> String {
        let payload = ExperimentResultArtifactHashPayload {
            artifact_schema_version: self.artifact_schema_version,
            artifact_id: &self.artifact_id,
            artifact_root: &self.artifact_root,
            artifact_uri: &self.artifact_uri,
            owner: &self.owner,
            source_refs: &self.source_refs,
            source_hashes: &self.source_hashes,
            lifecycle_state: &self.lifecycle_state,
            verdict: &self.verdict,
            promotion_config: self.promotion_config.as_ref(),
            dashboard_field_refs: &self.dashboard_field_refs,
            notebook_runtime_code_refs: &self.notebook_runtime_code_refs,
            accepts_source_proofs: self.accepts_source_proofs,
            mutates_source_proofs: self.mutates_source_proofs,
            mutates_backtest_result_contracts: self.mutates_backtest_result_contracts,
            weakens_forbidden_claims: self.weakens_forbidden_claims,
            post_verdict_actions: &self.post_verdict_actions,
        };
        sha256_hex(
            &serde_json::to_vec(&payload)
                .expect("experiment-results hash payload serialization cannot fail"),
        )
    }

    pub fn validate(&self) -> Result<(), ResearchAnalyticsArtifactError> {
        validate_experiment_result_identity(self)?;
        validate_experiment_results_uri("artifact_uri", &self.artifact_root, &self.artifact_uri)?;
        ensure_non_empty("source_refs", &self.source_refs)?;
        ensure_non_empty("source_hashes", &self.source_hashes)?;
        if self.source_refs.len() != self.source_hashes.len() {
            return Err(ResearchAnalyticsArtifactError::SourceRefHashCountMismatch {
                source_refs: self.source_refs.len(),
                source_hashes: self.source_hashes.len(),
            });
        }
        validate_sha256("content_hash", &self.content_hash)?;
        for source_ref in &self.source_refs {
            validate_non_empty("source_refs", source_ref)?;
        }
        for source_hash in &self.source_hashes {
            validate_sha256("source_hashes", source_hash)?;
        }
        for dashboard_ref in &self.dashboard_field_refs {
            validate_non_empty("dashboard_field_refs", dashboard_ref)?;
        }
        for notebook_ref in &self.notebook_runtime_code_refs {
            validate_non_empty("notebook_runtime_code_refs", notebook_ref)?;
        }
        self.verdict.validate()?;
        let forbidden = self.forbidden_behavior_violations();
        if !forbidden.is_empty() {
            return Err(ResearchAnalyticsArtifactError::ForbiddenPromotionBehavior {
                violations: forbidden,
            });
        }
        if let Some(promotion_config) = &self.promotion_config {
            if !self.verdict.is_real_go_finding() {
                return Err(ResearchAnalyticsArtifactError::PromotionConfigRequiresGo);
            }
            promotion_config.validate(&self.artifact_root)?;
        }
        let expected_content_hash = self.expected_content_hash();
        if self.content_hash != expected_content_hash {
            return Err(ResearchAnalyticsArtifactError::ContentHashMismatch {
                expected: expected_content_hash,
                actual: self.content_hash.clone(),
            });
        }
        Ok(())
    }

    fn forbidden_behavior_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.accepts_source_proofs {
            violations.push("unauthorized proof acceptance".to_string());
        }
        if self.mutates_source_proofs {
            violations.push("source proof mutation".to_string());
        }
        if self.mutates_backtest_result_contracts {
            violations.push("backtest result contract mutation".to_string());
        }
        if self.weakens_forbidden_claims {
            violations.push("forbidden-claim weakening".to_string());
        }
        if !self.notebook_runtime_code_refs.is_empty() {
            violations.push("notebook runtime code".to_string());
        }
        violations.extend(
            self.post_verdict_actions
                .iter()
                .map(|action| action.description().to_string()),
        );
        violations
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchAnalyticsArtifactError {
    EmptyField {
        field: &'static str,
    },
    EmptyList {
        field: &'static str,
    },
    InvalidArtifactVersion,
    InvalidSha256 {
        field: &'static str,
        value: String,
    },
    UnsupportedArtifactRoot {
        artifact_root: String,
    },
    ArtifactOutsideExperimentResults {
        field: &'static str,
        artifact_root: String,
        uri: String,
        expected_prefix: String,
    },
    PromotionConfigMissing {
        missing: &'static str,
    },
    PromotionConfigRequiresGo,
    SourceRefHashCountMismatch {
        source_refs: usize,
        source_hashes: usize,
    },
    IncompatibleClaimFidelity {
        source_fidelity: SourceProofFidelityClass,
        requested_fidelity: SourceProofFidelityClass,
    },
    ForbiddenPromotionBehavior {
        violations: Vec<String>,
    },
    ContentHashMismatch {
        expected: String,
        actual: String,
    },
}

impl fmt::Display for ResearchAnalyticsArtifactError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(formatter, "{field} must not be empty"),
            Self::EmptyList { field } => write!(formatter, "{field} must not be empty"),
            Self::InvalidArtifactVersion => {
                write!(
                    formatter,
                    "artifact_schema_version must be greater than zero"
                )
            }
            Self::InvalidSha256 { field, value } => {
                write!(
                    formatter,
                    "{field} must be lowercase sha256 hex, got {value:?}"
                )
            }
            Self::UnsupportedArtifactRoot { artifact_root } => {
                write!(
                    formatter,
                    "artifact_root must be an s3:// URI, got {artifact_root:?}"
                )
            }
            Self::ArtifactOutsideExperimentResults {
                field,
                artifact_root,
                uri,
                expected_prefix,
            } => write!(
                formatter,
                "{field} {uri:?} is outside RA experiment-results family for artifact_root {artifact_root:?}; expected prefix {expected_prefix:?}"
            ),
            Self::PromotionConfigMissing { missing } => write!(
                formatter,
                "promotion_config missing required evidence/boundary: {missing}"
            ),
            Self::PromotionConfigRequiresGo => write!(
                formatter,
                "promotion_config is allowed only on a real GO finding"
            ),
            Self::SourceRefHashCountMismatch {
                source_refs,
                source_hashes,
            } => write!(
                formatter,
                "source_refs count {source_refs} must match source_hashes count {source_hashes}"
            ),
            Self::IncompatibleClaimFidelity {
                source_fidelity,
                requested_fidelity,
            } => write!(
                formatter,
                "verdict requested fidelity {requested_fidelity:?} is not supported by source fidelity {source_fidelity:?}"
            ),
            Self::ForbiddenPromotionBehavior { violations } => write!(
                formatter,
                "experiment-results artifact contains forbidden promotion behavior: {}",
                violations.join(", ")
            ),
            Self::ContentHashMismatch { expected, actual } => write!(
                formatter,
                "experiment-results content_hash {actual:?} does not match expected payload hash {expected:?}"
            ),
        }
    }
}

impl Error for ResearchAnalyticsArtifactError {}

fn validate_experiment_result_identity(
    artifact: &ExperimentResultArtifact,
) -> Result<(), ResearchAnalyticsArtifactError> {
    if artifact.artifact_schema_version == 0 {
        return Err(ResearchAnalyticsArtifactError::InvalidArtifactVersion);
    }
    validate_non_empty("artifact_id", &artifact.artifact_id)?;
    validate_non_empty("artifact_root", &artifact.artifact_root)?;
    validate_non_empty("artifact_uri", &artifact.artifact_uri)?;
    validate_non_empty("owner", &artifact.owner)?;
    if !artifact.artifact_root.starts_with("s3://") {
        return Err(ResearchAnalyticsArtifactError::UnsupportedArtifactRoot {
            artifact_root: artifact.artifact_root.clone(),
        });
    }
    Ok(())
}

fn validate_experiment_results_uri(
    field: &'static str,
    artifact_root: &str,
    uri: &str,
) -> Result<(), ResearchAnalyticsArtifactError> {
    validate_non_empty(field, uri)?;
    let expected_prefix = experiment_results_prefix(artifact_root);
    if uri.starts_with(&expected_prefix) {
        Ok(())
    } else {
        Err(
            ResearchAnalyticsArtifactError::ArtifactOutsideExperimentResults {
                field,
                artifact_root: artifact_root.to_string(),
                uri: uri.to_string(),
                expected_prefix,
            },
        )
    }
}

fn experiment_results_prefix(artifact_root: &str) -> String {
    format!(
        "{}/{}/{}/{}/",
        artifact_root.trim_end_matches('/'),
        RESEARCH_ANALYTICS_KIND_PATH,
        RESEARCH_ANALYTICS_SCHEMA_VERSION,
        RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY
    )
}

fn validate_non_empty(
    field: &'static str,
    value: &str,
) -> Result<(), ResearchAnalyticsArtifactError> {
    if value.trim().is_empty() {
        Err(ResearchAnalyticsArtifactError::EmptyField { field })
    } else {
        Ok(())
    }
}

fn ensure_non_empty<T>(
    field: &'static str,
    values: &[T],
) -> Result<(), ResearchAnalyticsArtifactError> {
    if values.is_empty() {
        Err(ResearchAnalyticsArtifactError::EmptyList { field })
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), ResearchAnalyticsArtifactError> {
    if is_lowercase_sha256_hex(value) {
        Ok(())
    } else {
        Err(ResearchAnalyticsArtifactError::InvalidSha256 {
            field,
            value: value.to_string(),
        })
    }
}

fn source_fidelity_supports_claim(
    source: SourceProofFidelityClass,
    requested: SourceProofFidelityClass,
) -> bool {
    match source {
        SourceProofFidelityClass::L2Replay => true,
        SourceProofFidelityClass::SnapshotReplay => matches!(
            requested,
            SourceProofFidelityClass::SnapshotReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::TradeReplay => matches!(
            requested,
            SourceProofFidelityClass::TradeReplay
                | SourceProofFidelityClass::TradeBarReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::TradeBarReplay => matches!(
            requested,
            SourceProofFidelityClass::TradeBarReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::QuoteReplay => matches!(
            requested,
            SourceProofFidelityClass::QuoteReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::IndexReplay => matches!(
            requested,
            SourceProofFidelityClass::IndexReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::MarkReplay => matches!(
            requested,
            SourceProofFidelityClass::MarkReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::FundingReplay => matches!(
            requested,
            SourceProofFidelityClass::FundingReplay
                | SourceProofFidelityClass::SignalOnly
                | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::SignalOnly => matches!(
            requested,
            SourceProofFidelityClass::SignalOnly | SourceProofFidelityClass::MetadataOnly
        ),
        SourceProofFidelityClass::MetadataOnly => {
            matches!(requested, SourceProofFidelityClass::MetadataOnly)
        }
        SourceProofFidelityClass::ForwardCapturePending => {
            matches!(requested, SourceProofFidelityClass::ForwardCapturePending)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use crate::{
        result_contract::{
            BacktestResultContract, NautilusResultPointer, RESULT_CONTRACT_VERSION,
            ResultArtifactUris,
        },
        source_proof::AcceptanceMode,
    };
    use tempfile::TempDir;

    const COMMITTED_RUN_SPEC: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
    );
    const TEST_ARTIFACT_ROOT: &str = "s3://example-bucket/nt-research-analytics";

    fn test_run_spec(run_id: &str, accepted_object_bytes: &[u8]) -> RunSpec {
        let mut spec: RunSpec =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
        spec.manifest.run_id = run_id.to_string();
        spec.manifest.artifact_root = TEST_ARTIFACT_ROOT.to_string();
        spec.manifest.output_prefix = format!("{TEST_ARTIFACT_ROOT}/backtests/{run_id}");
        spec.artifact_store = None;
        spec.accepted_object.sha256 = sha256_hex(accepted_object_bytes);
        spec.accepted_object.bytes = accepted_object_bytes.len() as u64;
        spec
    }

    fn durable_test_run_spec(run_id: &str, accepted_object_bytes: &[u8]) -> RunSpec {
        let committed: RunSpec =
            toml::from_str(COMMITTED_RUN_SPEC).expect("committed durable run-spec parses");
        let mut spec = test_run_spec(run_id, accepted_object_bytes);
        spec.artifact_store = committed.artifact_store;
        spec.source_bindings_path = PathBuf::from("must-not-read/source-bindings.toml");
        spec
    }

    fn rewrite_source_run_spec<F>(input_dir: &Path, source: &BacktestSweepSourcePair, mutate: F)
    where
        F: FnOnce(&mut RunSpec),
    {
        let path = input_dir.join(&source.run_spec_path);
        let mut spec: RunSpec =
            toml::from_str(&fs::read_to_string(&path).expect("read source run spec for rewrite"))
                .expect("parse source run spec for rewrite");
        mutate(&mut spec);
        fs::write(
            &path,
            toml::to_string_pretty(&spec).expect("serialize rewritten run spec"),
        )
        .expect("rewrite source run spec");
    }

    fn test_contract(
        spec: &RunSpec,
        object_bytes: &[u8],
        result_contract_uri: &str,
    ) -> BacktestResultContract {
        BacktestResultContract {
            contract_version: RESULT_CONTRACT_VERSION.to_string(),
            run_id: spec.manifest.run_id.clone(),
            nt_version: spec.manifest.resolved_nt_version.clone(),
            source_proof_id: spec.manifest.source_proof_id.clone(),
            source_proof_version: spec.manifest.source_proof_version,
            manifest_hash: spec.manifest.manifest_hash(),
            acceptance_mode: AcceptanceMode::Manual,
            accepted_by: "research-analytics-test".to_string(),
            accepted_at: "2026-06-14T00:00:00Z".to_string(),
            accepted_object_sha256: sha256_hex(object_bytes),
            converter_identity: "converter".to_string(),
            converter_version: "converter.v1".to_string(),
            converter_config_hash: spec.converter.content_hash().expect("converter hash"),
            conversion_manifest_hash:
                "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc".to_string(),
            conversion_checkpoint_hash:
                "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_string(),
            catalog_hash: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
                .to_string(),
            catalog_metadata_hash:
                "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string(),
            event_count_ledger_hash: None,
            selected_asset_ids_hash: None,
            strategy_config_hash: spec.manifest.strategy_config_hash.clone(),
            execution_model: "nt_backtest_node".to_string(),
            venue_queue_position: Some(false),
            catalog_data_types: vec!["TradeTick".to_string()],
            run_purpose: "normal".to_string(),
            market_structure_fixture: "binary option".to_string(),
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            claim_limits: vec!["trade replay only".to_string()],
            warnings: Vec::new(),
            mechanical_blockers: Vec::new(),
            config_override_report: None,
            run_guard_report: None,
            feed_labels: vec![],
            nt_result: NautilusResultPointer {
                trader_id: "TRADER-001".to_string(),
                machine_id: "machine".to_string(),
                instance_id: "instance".to_string(),
                run_config_id: Some("run-config".to_string()),
                backtest_start: Some(1),
                backtest_end: Some(2),
                elapsed_time_secs: 0.1,
                iterations: 3,
                total_events: 4,
                total_orders: 5,
                total_positions: 6,
                stats_pnls: Default::default(),
                stats_returns: Default::default(),
            },
            artifact_uris: ResultArtifactUris {
                source_proof_uri: "s3://example-bucket/source-proof.json".to_string(),
                canonical_table_uri: "s3://example-bucket/canonical.parquet".to_string(),
                nt_catalog_uri: "s3://example-bucket/nt-catalog/".to_string(),
                nt_catalog_manifest_uri: None,
                catalog_metadata_uri: "s3://example-bucket/catalog-metadata.json".to_string(),
                result_contract_uri: result_contract_uri.to_string(),
            },
            created_at: "2026-06-14T00:00:01Z".to_string(),
        }
    }

    fn write_source_pair(
        input_dir: &Path,
        run_spec_name: &str,
        object_name: &str,
        run_id: &str,
        object_bytes: &[u8],
    ) -> BacktestSweepSourcePair {
        let spec = test_run_spec(run_id, object_bytes);
        fs::write(
            input_dir.join(run_spec_name),
            toml::to_string_pretty(&spec).expect("serialize run spec"),
        )
        .expect("write run spec");
        fs::write(input_dir.join(object_name), object_bytes).expect("write object");
        BacktestSweepSourcePair {
            run_spec_path: PathBuf::from(run_spec_name),
            object_path: PathBuf::from(object_name),
        }
    }

    fn write_test_contract(
        output_dir: &Path,
        spec: &RunSpec,
        object_bytes: &[u8],
        artifact_root: &str,
    ) {
        let uri = format!(
            "{artifact_root}/backtests/{}/{}",
            spec.manifest.run_id, RESULT_CONTRACT_FILE
        );
        let contract = test_contract(spec, object_bytes, &uri);
        fs::write(
            output_dir.join(RESULT_CONTRACT_FILE),
            serde_json::to_vec_pretty(&contract).expect("serialize contract"),
        )
        .expect("write result contract");
    }

    fn publication_plan(
        temp: &TempDir,
        input_dir: PathBuf,
        artifact_root: &str,
        sources: Vec<BacktestSweepSourcePair>,
    ) -> BacktestSweepPublicationPlan {
        BacktestSweepPublicationPlan {
            input_dir,
            run_spec_dir: temp.path().join("materialized-run-specs"),
            run_output_dir: temp.path().join("run-output"),
            artifact_root: artifact_root.to_string(),
            index_path: temp.path().join("run-pointer-index.json"),
            sources,
        }
    }

    #[test]
    fn default_and_injected_sweeps_reject_durable_run_spec_before_materializing_output() {
        let temp = TempDir::new().expect("temp dir");
        let object_bytes = b"must-not-execute".to_vec();
        let spec = durable_test_run_spec("ra-durable-run", &object_bytes);
        let plan = BacktestSweepPlan {
            run_spec_dir: temp.path().join("materialized-run-specs"),
            run_output_dir: temp.path().join("run-output"),
            runs: vec![BacktestSweepRun {
                run_spec_file_name: "durable.toml".to_string(),
                output_dir_name: "ra-durable-run".to_string(),
                run_spec: spec,
                accepted_object_bytes: object_bytes,
            }],
        };

        let injected_error = run_backtest_sweep_with_executor(&plan, |_, _, _| {
            panic!("durable authority rejection must precede injected executor")
        })
        .expect_err("injected local sweep must reject a durable RunSpec");
        let error = run_backtest_sweep(&plan)
            .expect_err("default local sweep must reject a durable RunSpec");

        for error in [&injected_error, &error] {
            let error_chain = format!("{error:#}");
            assert!(
                error_chain.contains("must use source_universe_batch_execution"),
                "{error:#}"
            );
        }
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
    }

    #[test]
    fn default_and_injected_publications_reject_durable_before_object_read_or_output() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let run_spec_path = input_dir.join("durable.toml");
        let spec = durable_test_run_spec("ra-durable-run", b"object-not-present");
        fs::write(
            &run_spec_path,
            toml::to_string_pretty(&spec).expect("serialize durable run-spec"),
        )
        .expect("write durable run-spec");
        let source = BacktestSweepSourcePair {
            run_spec_path: PathBuf::from("durable.toml"),
            object_path: PathBuf::from("must-not-read.object"),
        };
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);

        let injected_error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("durable authority rejection must precede injected executor")
        })
        .expect_err("injected local publication must reject a durable RunSpec");
        let error = run_backtest_sweep_publication(&plan)
            .expect_err("default local publication must reject a durable RunSpec");

        for error in [&injected_error, &error] {
            let error_chain = format!("{error:#}");
            assert!(
                error_chain.contains("must use source_universe_batch_execution"),
                "{error:#}"
            );
            assert!(!error_chain.contains("must-not-read.object"));
        }
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert!(!plan.index_path.exists());
    }

    #[test]
    fn publication_preflights_all_run_spec_authority_before_any_object_path() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let local_spec = test_run_spec("ra-local-first", b"missing-local-object");
        let durable_spec = durable_test_run_spec("ra-durable-second", b"must-not-read");
        fs::write(
            input_dir.join("local.toml"),
            toml::to_string_pretty(&local_spec).expect("serialize local run-spec"),
        )
        .expect("write local run-spec");
        fs::write(
            input_dir.join("durable.toml"),
            toml::to_string_pretty(&durable_spec).expect("serialize durable run-spec"),
        )
        .expect("write durable run-spec");
        let plan = publication_plan(
            &temp,
            input_dir,
            TEST_ARTIFACT_ROOT,
            vec![
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("local.toml"),
                    object_path: PathBuf::from("missing-local.object"),
                },
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("durable.toml"),
                    object_path: PathBuf::from("must-not-read.object"),
                },
            ],
        );

        let error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("full authority preflight must precede object reads and execution")
        })
        .expect_err("later durable RunSpec must reject the whole publication plan");
        let error_chain = format!("{error:#}");

        assert!(
            error_chain.contains("must use source_universe_batch_execution"),
            "{error:#}"
        );
        assert!(!error_chain.contains("missing-local.object"));
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert!(!plan.index_path.exists());
    }

    #[test]
    fn publication_preflights_all_object_metadata_before_hashing_any_payload() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let first = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-first",
            b"expected",
        );
        fs::write(input_dir.join(&first.object_path), b"tampered")
            .expect("replace first object with same-length corrupt bytes");
        let second = write_source_pair(
            &input_dir,
            "second.toml",
            "second.object",
            "ra-second",
            b"second",
        );
        let plan = publication_plan(
            &temp,
            input_dir,
            TEST_ARTIFACT_ROOT,
            vec![
                first,
                BacktestSweepSourcePair {
                    run_spec_path: second.run_spec_path,
                    object_path: PathBuf::from("missing-second.object"),
                },
            ],
        );

        let error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("object metadata preflight must precede payload hashing and execution")
        })
        .expect_err("later missing object must reject before hashing the earlier payload");
        let error_chain = format!("{error:#}");

        assert!(error_chain.contains("missing-second.object"), "{error:#}");
        assert!(!error_chain.contains("SHA-256"), "{error:#}");
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert!(!plan.index_path.exists());
    }

    #[test]
    fn publication_rejects_cross_pair_source_role_collision_before_parsing_run_spec() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        fs::write(input_dir.join("first.toml"), b"not valid TOML")
            .expect("write deliberately invalid first source");
        fs::write(input_dir.join("second.toml"), b"also not valid TOML")
            .expect("write deliberately invalid second source");
        let plan = publication_plan(
            &temp,
            input_dir,
            TEST_ARTIFACT_ROOT,
            vec![
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("first.toml"),
                    object_path: PathBuf::from("second.toml"),
                },
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("second.toml"),
                    object_path: PathBuf::from("missing.object"),
                },
            ],
        );

        let error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("source role collision must reject before parsing or execution")
        })
        .expect_err("one source file cannot hold both control and payload roles");
        let error_chain = format!("{error:#}");

        assert!(
            error_chain.contains("both a run-spec control and accepted-object payload"),
            "{error:#}"
        );
        assert!(!error_chain.contains("parse run-spec TOML"), "{error:#}");
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert!(!plan.index_path.exists());
    }

    #[test]
    fn publication_rejects_index_inside_output_target_before_hashing_payload() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"expected",
        );
        fs::write(input_dir.join(&source.object_path), b"tampered")
            .expect("replace object with same-length corrupt bytes");
        let mut plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        plan.index_path = plan.run_output_dir.join("ra-run-a/index.json");

        let error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("index overlap must reject before payload hashing or execution")
        })
        .expect_err("index target inside an output target must fail closed");
        let error_chain = format!("{error:#}");

        assert!(
            error_chain.contains("planned filesystem targets overlap"),
            "{error:#}"
        );
        assert!(!error_chain.contains("SHA-256"), "{error:#}");
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert!(!plan.index_path.exists());
    }

    #[test]
    fn publication_rejects_occupied_index_before_hashing_payload() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"expected",
        );
        fs::write(input_dir.join(&source.object_path), b"tampered")
            .expect("replace object with same-length corrupt bytes");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        fs::write(&plan.index_path, b"occupied").expect("occupy index target");

        let error = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            panic!("occupied index must reject before payload hashing or execution")
        })
        .expect_err("occupied index target must fail closed");
        let error_chain = format!("{error:#}");

        assert!(
            error_chain.contains("index path already exists"),
            "{error:#}"
        );
        assert!(!error_chain.contains("SHA-256"), "{error:#}");
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
        assert_eq!(
            fs::read(&plan.index_path).expect("read occupied index"),
            b"occupied"
        );
    }

    #[test]
    fn sweep_publication_writes_index_from_source_pairs_after_all_runs_succeed() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let first = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let second = write_source_pair(
            &input_dir,
            "second.toml",
            "second.object",
            "ra-run-b",
            b"second",
        );
        let artifact_root = TEST_ARTIFACT_ROOT;
        let plan = publication_plan(&temp, input_dir, artifact_root, vec![first, second]);

        let publication = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                write_test_contract(output_dir, spec, object_bytes, artifact_root);
                Ok(())
            },
        )
        .expect("sweep publication succeeds");

        publication.index.validate().expect("index validates");
        assert_eq!(publication.index.runs.len(), 2);
        assert!(plan.index_path.exists(), "index artifact written");
        assert_eq!(
            publication
                .index
                .runs
                .iter()
                .map(|record| record.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["ra-run-a", "ra-run-b"]
        );
        for record in &publication.index.runs {
            assert!(
                record
                    .result
                    .result_contract_uri
                    .starts_with(&format!("{artifact_root}/"))
            );
            assert!(record.params.contains_key("source_run_spec_sha256"));
            assert!(record.params.contains_key("accepted_object_sha256"));
        }
        let first_params = &publication.index.runs[0].params;
        assert_eq!(
            first_params
                .get("source_run_spec_path")
                .and_then(serde_json::Value::as_str),
            Some("first.toml")
        );
        assert_eq!(
            first_params
                .get("source_object_path")
                .and_then(serde_json::Value::as_str),
            Some("first.object")
        );
        assert_eq!(publication.index_artifact.path, plan.index_path);
    }

    #[test]
    fn sweep_publication_leaves_no_index_when_result_contract_missing() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let artifact_root = TEST_ARTIFACT_ROOT;
        let plan = publication_plan(&temp, input_dir, artifact_root, vec![source]);

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| Ok(()))
            .expect_err("missing result contract must fail publication");

        assert!(err.to_string().contains("result-contract.json"), "{err}");
        assert!(
            !plan.index_path.exists(),
            "index artifact must not be left behind"
        );
    }

    #[test]
    fn sweep_publication_rejects_result_contract_uri_mismatch_without_index() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let artifact_root = TEST_ARTIFACT_ROOT;
        let plan = publication_plan(&temp, input_dir, artifact_root, vec![source]);
        let wrong_result_contract_uri =
            format!("{artifact_root}/backtests/ra-run-b/{RESULT_CONTRACT_FILE}");

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                let contract = test_contract(spec, object_bytes, &wrong_result_contract_uri);
                fs::write(
                    output_dir.join(RESULT_CONTRACT_FILE),
                    serde_json::to_vec_pretty(&contract).expect("serialize contract"),
                )
                .expect("write result contract");
                Ok(())
            },
        )
        .expect_err("result contract URI mismatch must fail publication");

        assert!(err.to_string().contains("result_contract_uri"), "{err}");
        assert!(
            !plan.index_path.exists(),
            "index artifact must not be left behind"
        );
    }

    #[test]
    fn sweep_publication_refuses_dirty_index_without_clobbering_existing_bytes() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let artifact_root = TEST_ARTIFACT_ROOT;
        let plan = publication_plan(&temp, input_dir, artifact_root, vec![source]);
        fs::write(&plan.index_path, b"stale index bytes").expect("write stale index");

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                write_test_contract(output_dir, spec, object_bytes, artifact_root);
                Ok(())
            },
        )
        .expect_err("dirty index must fail with FailOnDirty");

        assert!(
            err.to_string().contains("dirty reference artifact"),
            "{err}"
        );
        assert_eq!(
            fs::read(&plan.index_path).expect("read stale index"),
            b"stale index bytes",
            "dirty index bytes must remain untouched"
        );
    }

    #[test]
    fn sweep_publication_rejects_bad_artifact_roots_before_executor() {
        for artifact_root in [
            "file:///tmp/nt-research-analytics",
            "s3://example-bucket",
            "s3://example-bucket//bad",
            "s3://example-bucket/nt-research-analytics/",
        ] {
            let temp = TempDir::new().expect("temp dir");
            let input_dir = temp.path().join("inputs");
            fs::create_dir_all(&input_dir).expect("create input dir");
            let source = write_source_pair(
                &input_dir,
                "first.toml",
                "first.object",
                "ra-run-a",
                b"first",
            );
            let plan = publication_plan(&temp, input_dir, artifact_root, vec![source]);
            let mut calls = 0;

            let err = run_backtest_sweep_publication_with_executor(
                &plan,
                |spec, object_bytes, output_dir| {
                    calls += 1;
                    write_test_contract(output_dir, spec, object_bytes, artifact_root);
                    Ok(())
                },
            )
            .expect_err("bad artifact_root must fail");

            assert_eq!(calls, 0, "executor must not run after bad artifact_root");
            assert!(
                err.to_string().contains("artifact_root"),
                "artifact_root {artifact_root:?} produced {err}"
            );
            assert!(!plan.index_path.exists(), "index must not be written");
        }
    }

    #[test]
    fn sweep_publication_rejects_parent_source_paths_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let mut source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        source.run_spec_path = PathBuf::from("nested").join("..").join("first.toml");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("parent source path must fail");

        assert_eq!(calls, 0, "executor must not run after bad source path");
        assert!(err.to_string().contains("relative to input_dir"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_absolute_source_paths_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let mut source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        source.run_spec_path = input_dir.join("first.toml");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("absolute source path must fail");

        assert_eq!(calls, 0, "executor must not run after bad source path");
        assert!(err.to_string().contains("relative to input_dir"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_publication_rejects_symlinked_source_run_spec_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&outside_dir).expect("create outside dir");
        fs::write(
            outside_dir.join("outside.toml"),
            toml::to_string_pretty(&test_run_spec("ra-run-a", b"first"))
                .expect("serialize outside run spec"),
        )
        .expect("write outside run spec");
        fs::remove_file(input_dir.join("first.toml")).expect("remove original run spec");
        std::os::unix::fs::symlink(
            outside_dir.join("outside.toml"),
            input_dir.join("first.toml"),
        )
        .expect("symlink run spec");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                calls += 1;
                write_test_contract(output_dir, spec, object_bytes, TEST_ARTIFACT_ROOT);
                Ok(())
            },
        )
        .expect_err("symlinked run-spec source path must fail");

        assert_eq!(
            calls, 0,
            "executor must not run after symlinked source path"
        );
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_publication_rejects_symlinked_source_object_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&outside_dir).expect("create outside dir");
        fs::write(outside_dir.join("outside.object"), b"first").expect("write outside object");
        fs::remove_file(input_dir.join("first.object")).expect("remove original object");
        std::os::unix::fs::symlink(
            outside_dir.join("outside.object"),
            input_dir.join("first.object"),
        )
        .expect("symlink object");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                calls += 1;
                write_test_contract(output_dir, spec, object_bytes, TEST_ARTIFACT_ROOT);
                Ok(())
            },
        )
        .expect_err("symlinked object source path must fail");

        assert_eq!(
            calls, 0,
            "executor must not run after symlinked source path"
        );
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_publication_rejects_symlinked_source_directory_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let outside_dir = temp.path().join("outside");
        fs::create_dir_all(&outside_dir).expect("create outside dir");
        let source = write_source_pair(
            &outside_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        std::os::unix::fs::symlink(&outside_dir, input_dir.join("linked"))
            .expect("symlink source directory");
        let plan = publication_plan(
            &temp,
            input_dir,
            TEST_ARTIFACT_ROOT,
            vec![BacktestSweepSourcePair {
                run_spec_path: PathBuf::from("linked").join(source.run_spec_path),
                object_path: PathBuf::from("linked").join(source.object_path),
            }],
        );
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                calls += 1;
                write_test_contract(output_dir, spec, object_bytes, TEST_ARTIFACT_ROOT);
                Ok(())
            },
        )
        .expect_err("symlinked source directory must fail");

        assert_eq!(
            calls, 0,
            "executor must not run after symlinked source path"
        );
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[cfg(unix)]
    #[test]
    fn sweep_publication_rejects_symlinked_input_dir_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let real_input_dir = temp.path().join("real-inputs");
        fs::create_dir_all(&real_input_dir).expect("create real input dir");
        let source = write_source_pair(
            &real_input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let linked_input_dir = temp.path().join("linked-inputs");
        std::os::unix::fs::symlink(&real_input_dir, &linked_input_dir).expect("symlink input dir");
        let plan = publication_plan(&temp, linked_input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(
            &plan,
            |spec, object_bytes, output_dir| {
                calls += 1;
                write_test_contract(output_dir, spec, object_bytes, TEST_ARTIFACT_ROOT);
                Ok(())
            },
        )
        .expect_err("symlinked input_dir must fail");

        assert_eq!(calls, 0, "executor must not run after symlinked input_dir");
        assert!(err.to_string().contains("input_dir"), "{err}");
        assert!(err.to_string().contains("symlink"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_empty_input_dir_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, Vec::new());
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("empty input dir must fail");

        assert_eq!(calls, 0, "executor must not run after empty input dir");
        assert!(
            err.to_string().contains("input_dir must not be empty"),
            "{err}"
        );
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_object_hash_mismatch_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        fs::write(input_dir.join("first.object"), b"wrong").expect("tamper object");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("object hash mismatch must fail before executor");

        assert_eq!(calls, 0, "executor must not see unverified object bytes");
        assert!(format!("{err:#}").contains("object SHA-256"), "{err:#}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_object_byte_length_mismatch_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        fs::write(input_dir.join("first.object"), b"longer").expect("tamper object length");
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("object byte length mismatch must fail before executor");

        assert_eq!(calls, 0, "executor must not see wrong-length object bytes");
        assert!(format!("{err:#}").contains("object byte length"), "{err:#}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_run_spec_artifact_root_mismatch_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        rewrite_source_run_spec(&input_dir, &source, |spec| {
            spec.manifest.artifact_root = "s3://other-bucket/nt-research-analytics".to_string();
            spec.manifest.output_prefix =
                "s3://other-bucket/nt-research-analytics/backtests/ra-run-a".to_string();
        });
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("run-spec artifact root mismatch must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after run-spec root mismatch"
        );
        assert!(err.to_string().contains("manifest.artifact_root"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_run_spec_output_prefix_mismatch_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        rewrite_source_run_spec(&input_dir, &source, |spec| {
            spec.manifest.output_prefix = format!("{TEST_ARTIFACT_ROOT}/not-backtests/ra-run-a");
        });
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("run-spec output prefix mismatch must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after run-spec output prefix mismatch"
        );
        assert!(err.to_string().contains("manifest.output_prefix"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_run_spec_output_prefix_for_different_run_id_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let source = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        rewrite_source_run_spec(&input_dir, &source, |spec| {
            spec.manifest.output_prefix = format!("{TEST_ARTIFACT_ROOT}/backtests/ra-run-b");
        });
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![source]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("run-spec output prefix for another run must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after run-spec output prefix targets another run"
        );
        assert!(err.to_string().contains("manifest.output_prefix"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_duplicate_remote_output_prefixes_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let first = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let second = write_source_pair(
            &input_dir,
            "second.toml",
            "second.object",
            "ra-run-b",
            b"second",
        );
        rewrite_source_run_spec(&input_dir, &second, |spec| {
            spec.manifest.output_prefix = format!("{TEST_ARTIFACT_ROOT}/backtests/ra-run-a");
        });
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![first, second]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("duplicate remote output prefix must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after duplicate remote output prefixes"
        );
        assert!(err.to_string().contains("manifest.output_prefix"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_duplicate_run_spec_names_before_object_read_or_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        let left_dir = input_dir.join("left");
        let right_dir = input_dir.join("right");
        fs::create_dir_all(&left_dir).expect("create left input dir");
        fs::create_dir_all(&right_dir).expect("create right input dir");
        let left = write_source_pair(&left_dir, "run.toml", "run.object", "ra-run-a", b"first");
        let right = write_source_pair(&right_dir, "run.toml", "run.object", "ra-run-b", b"second");
        let plan = publication_plan(
            &temp,
            input_dir,
            TEST_ARTIFACT_ROOT,
            vec![
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("left").join(left.run_spec_path),
                    object_path: PathBuf::from("left/missing-left.object"),
                },
                BacktestSweepSourcePair {
                    run_spec_path: PathBuf::from("right").join(right.run_spec_path),
                    object_path: PathBuf::from("right/missing-right.object"),
                },
            ],
        );
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("duplicate run-spec file names must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after duplicate run-spec names"
        );
        assert!(
            err.to_string().contains("duplicate run_spec_file_name"),
            "{err}"
        );
        assert!(!err.to_string().contains("missing-left.object"), "{err}");
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn sweep_publication_rejects_duplicate_output_dirs_before_executor() {
        let temp = TempDir::new().expect("temp dir");
        let input_dir = temp.path().join("inputs");
        fs::create_dir_all(&input_dir).expect("create input dir");
        let first = write_source_pair(
            &input_dir,
            "first.toml",
            "first.object",
            "ra-run-a",
            b"first",
        );
        let second = write_source_pair(
            &input_dir,
            "second.toml",
            "second.object",
            "ra-run-a",
            b"second",
        );
        let plan = publication_plan(&temp, input_dir, TEST_ARTIFACT_ROOT, vec![first, second]);
        let mut calls = 0;

        let err = run_backtest_sweep_publication_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("duplicate output dir names must fail before executor");

        assert_eq!(
            calls, 0,
            "executor must not run after duplicate output dirs"
        );
        assert!(
            err.to_string().contains("duplicate output_dir_name"),
            "{err}"
        );
        assert!(!plan.index_path.exists(), "index must not be written");
    }

    #[test]
    fn leaf_paths_reject_multiple_normal_components() {
        for error in [
            validate_leaf_path("output_dir_name", "nested/run")
                .expect_err("nested output name must fail"),
            validate_run_spec_file_name("nested/run.toml")
                .expect_err("nested run-spec name must fail"),
        ] {
            assert!(
                error
                    .to_string()
                    .contains("must be a single relative path segment"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn sweep_rejects_ancestor_materialization_targets_before_mutation_or_executor() {
        let temp = TempDir::new().expect("temp dir");
        let object_bytes = b"one-record".to_vec();
        for (case, run_spec_dir, run_output_dir) in [
            (
                "run-spec-ancestor",
                temp.path().join("run-spec-ancestor"),
                temp.path().join("run-spec-ancestor/run.toml"),
            ),
            (
                "output-ancestor",
                temp.path().join("output-ancestor/ra-run-a"),
                temp.path().join("output-ancestor"),
            ),
        ] {
            let plan = BacktestSweepPlan {
                run_spec_dir,
                run_output_dir,
                runs: vec![BacktestSweepRun {
                    run_spec_file_name: "run.toml".to_string(),
                    output_dir_name: "ra-run-a".to_string(),
                    run_spec: test_run_spec("ra-run-a", &object_bytes),
                    accepted_object_bytes: object_bytes.clone(),
                }],
            };

            let error = run_backtest_sweep_with_executor(&plan, |_, _, _| {
                panic!("ancestor target overlap must reject before executor")
            })
            .expect_err("ancestor materialization targets must fail closed");

            assert!(
                error
                    .to_string()
                    .contains("planned filesystem targets overlap"),
                "{case}: {error:#}"
            );
            assert!(
                !temp.path().join(case).exists(),
                "{case}: preflight rejection must not create target roots"
            );
        }
    }

    #[test]
    fn sweep_rejects_unverified_in_memory_payload_before_mutation_or_executor() {
        let temp = TempDir::new().expect("temp dir");
        let expected = b"expected";
        let plan = BacktestSweepPlan {
            run_spec_dir: temp.path().join("materialized-run-specs"),
            run_output_dir: temp.path().join("run-output"),
            runs: vec![BacktestSweepRun {
                run_spec_file_name: "run.toml".to_string(),
                output_dir_name: "ra-run-a".to_string(),
                run_spec: test_run_spec("ra-run-a", expected),
                accepted_object_bytes: b"tampered".to_vec(),
            }],
        };
        let mut calls = 0;

        let error = run_backtest_sweep_with_executor(&plan, |_, _, _| {
            calls += 1;
            Ok(())
        })
        .expect_err("unverified in-memory payload must fail closed");

        assert_eq!(
            calls, 0,
            "executor must not receive unverified payload bytes"
        );
        assert!(error.to_string().contains("SHA-256"), "{error:#}");
        assert!(!plan.run_spec_dir.exists());
        assert!(!plan.run_output_dir.exists());
    }

    #[test]
    fn sweep_rejects_cross_role_materialization_target_before_mutation_or_executor() {
        let temp = TempDir::new().expect("temp dir");
        let materialization_dir = temp.path().join("materialized");
        let object_bytes = b"one-record".to_vec();
        let run_id = "run.toml";
        let plan = BacktestSweepPlan {
            run_spec_dir: materialization_dir.clone(),
            run_output_dir: materialization_dir.clone(),
            runs: vec![BacktestSweepRun {
                run_spec_file_name: run_id.to_string(),
                output_dir_name: run_id.to_string(),
                run_spec: test_run_spec(run_id, &object_bytes),
                accepted_object_bytes: object_bytes,
            }],
        };

        let error = run_backtest_sweep_with_executor(&plan, |_, _, _| {
            panic!("cross-role target collision must reject before executor")
        })
        .expect_err("run-spec and output target collision must fail closed");

        assert!(
            error
                .to_string()
                .contains("planned filesystem targets overlap"),
            "{error:#}"
        );
        assert!(
            !materialization_dir.exists(),
            "preflight rejection must not create the shared target root"
        );
    }

    #[test]
    fn quote_replay_supports_self_signal_and_metadata_only() {
        // A QuoteReplay source backs QuoteReplay, SignalOnly, and MetadataOnly
        // claims; it must not back any foreign replay class.
        for requested in [
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::QuoteReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::MarkReplay,
            SourceProofFidelityClass::FundingReplay,
            SourceProofFidelityClass::L2Replay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::QuoteReplay,
                requested,
            ));
        }
    }

    #[test]
    fn index_replay_supports_self_signal_and_metadata_only() {
        for requested in [
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::IndexReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::MarkReplay,
            SourceProofFidelityClass::FundingReplay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::IndexReplay,
                requested,
            ));
        }
    }

    #[test]
    fn mark_replay_supports_self_signal_and_metadata_only() {
        for requested in [
            SourceProofFidelityClass::MarkReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::MarkReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::FundingReplay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::MarkReplay,
                requested,
            ));
        }
    }

    #[test]
    fn funding_replay_supports_self_signal_and_metadata_only() {
        for requested in [
            SourceProofFidelityClass::FundingReplay,
            SourceProofFidelityClass::SignalOnly,
            SourceProofFidelityClass::MetadataOnly,
        ] {
            assert!(source_fidelity_supports_claim(
                SourceProofFidelityClass::FundingReplay,
                requested,
            ));
        }
        for requested in [
            SourceProofFidelityClass::TradeReplay,
            SourceProofFidelityClass::QuoteReplay,
            SourceProofFidelityClass::IndexReplay,
            SourceProofFidelityClass::MarkReplay,
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::FundingReplay,
                requested,
            ));
        }
    }
}
