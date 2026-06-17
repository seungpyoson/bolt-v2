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
    hashing::{is_lowercase_sha256_hex, sha256_hex},
    operator::{RESULT_CONTRACT_FILE, RunSpec, run_operator_from_run_spec},
    result_contract::BacktestResultContract,
    source_proof::SourceProofFidelityClass,
};

const RESEARCH_ANALYTICS_KIND_PATH: &str = "research-analytics";
const RESEARCH_ANALYTICS_SCHEMA_VERSION: &str = "v1";
const RESEARCH_ANALYTICS_EXPERIMENT_RESULTS_SUBFAMILY: &str = "experiment-results";
const RUN_POINTER_INDEX_SCHEMA_VERSION: u64 = 1;

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
/// Returns an error if a run-spec cannot be materialized, the provided BTE
/// executor fails, or the persisted result contract is missing/invalid.
pub fn run_backtest_sweep_with_executor<F>(
    plan: &BacktestSweepPlan,
    mut executor: F,
) -> Result<BacktestSweepReport>
where
    F: FnMut(&RunSpec, &[u8], &Path) -> Result<()>,
{
    ensure!(!plan.runs.is_empty(), "sweep must include at least one run");

    let mut seen_run_spec_file_names = BTreeSet::new();
    let mut seen_output_dir_names = BTreeSet::new();
    for run in &plan.runs {
        validate_run_spec_file_name(&run.run_spec_file_name)?;
        validate_leaf_path("output_dir_name", &run.output_dir_name)?;
        ensure!(
            !run.accepted_object_bytes.is_empty(),
            "accepted_object_bytes for run {} must not be empty",
            run.run_spec.manifest.run_id
        );
        ensure!(
            seen_run_spec_file_names.insert(run.run_spec_file_name.clone()),
            "duplicate run_spec_file_name {:?}",
            run.run_spec_file_name
        );
        ensure!(
            seen_output_dir_names.insert(run.output_dir_name.clone()),
            "duplicate output_dir_name {:?}",
            run.output_dir_name
        );

        let run_spec_path = plan.run_spec_dir.join(&run.run_spec_file_name);
        let output_dir = plan.run_output_dir.join(&run.output_dir_name);
        ensure!(
            !run_spec_path
                .try_exists()
                .with_context(|| format!("check run-spec path {}", run_spec_path.display()))?,
            "run-spec path already exists: {}",
            run_spec_path.display()
        );
        ensure!(
            !output_dir
                .try_exists()
                .with_context(|| format!("check output_dir {}", output_dir.display()))?,
            "output_dir already exists: {}",
            output_dir.display()
        );
    }

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

    let expected_accepted_object_sha256 = sha256_hex(&run.accepted_object_bytes);
    ensure!(
        contract.accepted_object_sha256 == expected_accepted_object_sha256,
        "{} accepted_object_sha256 {:?} does not match accepted object bytes {:?}",
        result_contract_path.display(),
        contract.accepted_object_sha256,
        expected_accepted_object_sha256
    );
    ensure!(
        run.run_spec.accepted_object.sha256 == expected_accepted_object_sha256,
        "run-spec accepted_object.sha256 {:?} does not match accepted object bytes {:?}",
        run.run_spec.accepted_object.sha256,
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

    Ok(())
}

fn validate_run_pointer_artifact_root(artifact_root: &str) -> Result<String> {
    let artifact_root = artifact_root.trim_end_matches('/').to_string();
    ensure!(
        !artifact_root.trim().is_empty(),
        "artifact_root must not be empty"
    );
    ensure!(
        artifact_root.starts_with("s3://"),
        "artifact_root must be an s3:// URI"
    );
    Ok(artifact_root)
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
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
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
        ] {
            assert!(!source_fidelity_supports_claim(
                SourceProofFidelityClass::MarkReplay,
                requested,
            ));
        }
    }
}
