//! Pure Research Analytics contract helpers.
//!
//! This module deliberately does not own a backtest runner, mutate source-proof
//! or BTE artifacts, touch SSM, or write runtime config. It validates that an
//! RA-owned promotion package is only a typed, claim-limited handoff artifact
//! and materializes sweep inputs for the existing BTE operator path.

use std::{
    error::Error,
    fmt, fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    artifact_index::ResearchAnalyticsSubfamily,
    operator::{RESULT_CONTRACT_FILE, RunSpec, run_operator_from_run_spec},
    result_contract::BacktestResultContract,
    source_proof::SourceProofFidelityClass,
};

const RESEARCH_ANALYTICS_KIND_PATH: &str = "research-analytics";
const RESEARCH_ANALYTICS_SCHEMA_VERSION: &str = "v1";

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
    fs::create_dir_all(&plan.run_spec_dir)
        .with_context(|| format!("create run-spec dir {}", plan.run_spec_dir.display()))?;
    fs::create_dir_all(&plan.run_output_dir)
        .with_context(|| format!("create run-output dir {}", plan.run_output_dir.display()))?;

    let mut reports = Vec::with_capacity(plan.runs.len());
    for run in &plan.runs {
        validate_run_spec_file_name(&run.run_spec_file_name)?;
        validate_leaf_path("output_dir_name", &run.output_dir_name)?;
        ensure!(
            !run.accepted_object_bytes.is_empty(),
            "accepted_object_bytes for run {} must not be empty",
            run.run_spec.manifest.run_id
        );

        let run_spec_path = plan.run_spec_dir.join(&run.run_spec_file_name);
        let output_dir = plan.run_output_dir.join(&run.output_dir_name);
        fs::create_dir_all(&output_dir)
            .with_context(|| format!("create run output dir {}", output_dir.display()))?;

        let run_spec_toml =
            toml::to_string_pretty(&run.run_spec).context("serialize typed run-spec TOML")?;
        fs::write(&run_spec_path, run_spec_toml)
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
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    Draft,
    Blocked,
    ReadyForReview,
    ChangesRequested,
    Rejected,
    ApprovedForConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostApprovalAction {
    AutoMerge,
    AutoEnableStrategy,
    ScheduleLiveTrading,
    TouchSsmCredentials,
    MutateProductionRuntimeConfig,
}

impl PostApprovalAction {
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
pub struct SourceProofEvidenceRef {
    pub source_proof_id: String,
    pub source_proof_version: Option<u64>,
    pub source_proof_report_uri: String,
    pub source_proof_report_hash: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub accepted: bool,
}

impl SourceProofEvidenceRef {
    fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_non_empty("source_proof_id", &self.source_proof_id)?;
        validate_non_empty("source_proof_report_uri", &self.source_proof_report_uri)?;
        validate_sha256("source_proof_report_hash", &self.source_proof_report_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacktestEvidenceRef {
    pub result_contract_id: String,
    pub result_contract_uri: String,
    pub result_contract_hash: String,
    pub objective: bool,
}

impl BacktestEvidenceRef {
    fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_non_empty("result_contract_id", &self.result_contract_id)?;
        validate_non_empty("result_contract_uri", &self.result_contract_uri)?;
        validate_sha256("result_contract_hash", &self.result_contract_hash)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionPackage {
    pub package_version: u64,
    pub artifact_root: String,
    pub artifact_uri: String,
    pub status: PromotionStatus,
    pub source_proof_refs: Vec<SourceProofEvidenceRef>,
    pub backtest_result_refs: Vec<BacktestEvidenceRef>,
    pub preserved_claim_limits: Vec<String>,
    pub requested_claim_fidelity: SourceProofFidelityClass,
    pub typed_config_uri: Option<String>,
    pub typed_config_hash: Option<String>,
    pub dashboard_field_refs: Vec<String>,
    pub reviewer_policy_refs: Vec<String>,
    pub non_live_boundary: bool,
    pub notebook_runtime_code_refs: Vec<String>,
    pub accepts_source_proofs: bool,
    pub mutates_source_proofs: bool,
    pub mutates_backtest_result_contracts: bool,
    pub weakens_forbidden_claims: bool,
    pub post_approval_actions: Vec<PostApprovalAction>,
}

impl PromotionPackage {
    pub fn validate(&self) -> Result<(), PromotionPackageError> {
        validate_package_identity(self)?;
        validate_promotion_family_uri("artifact_uri", &self.artifact_root, &self.artifact_uri)?;

        for source_ref in &self.source_proof_refs {
            source_ref.validate()?;
        }
        for backtest_ref in &self.backtest_result_refs {
            backtest_ref.validate()?;
        }
        for claim_limit in &self.preserved_claim_limits {
            validate_non_empty("preserved_claim_limits", claim_limit)?;
        }
        for reviewer_ref in &self.reviewer_policy_refs {
            validate_non_empty("reviewer_policy_refs", reviewer_ref)?;
        }
        for dashboard_ref in &self.dashboard_field_refs {
            validate_non_empty("dashboard_field_refs", dashboard_ref)?;
        }
        for notebook_ref in &self.notebook_runtime_code_refs {
            validate_non_empty("notebook_runtime_code_refs", notebook_ref)?;
        }
        if let Some(uri) = &self.typed_config_uri {
            validate_promotion_family_uri("typed_config_uri", &self.artifact_root, uri)?;
        }
        if let Some(hash) = &self.typed_config_hash {
            validate_sha256("typed_config_hash", hash)?;
        }

        let forbidden = self.forbidden_behavior_violations();
        if !forbidden.is_empty() {
            return Err(PromotionPackageError::ForbiddenPromotionPackageBehavior {
                violations: forbidden,
            });
        }

        if self.status == PromotionStatus::ApprovedForConfig {
            let missing = self.approved_for_config_missing_requirements();
            if !missing.is_empty() {
                return Err(PromotionPackageError::ApprovedForConfigMissing { missing });
            }
        }

        Ok(())
    }

    fn approved_for_config_missing_requirements(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self
            .source_proof_refs
            .iter()
            .any(|source_ref| source_ref.accepted)
            || self
                .source_proof_refs
                .iter()
                .any(|source_ref| !source_ref.accepted)
        {
            missing.push("accepted source proof refs");
        }
        if !self
            .backtest_result_refs
            .iter()
            .any(|backtest_ref| backtest_ref.objective)
            || self
                .backtest_result_refs
                .iter()
                .any(|backtest_ref| !backtest_ref.objective)
        {
            missing.push("objective backtest result refs");
        }
        if self.preserved_claim_limits.is_empty() {
            missing.push("preserved claim limits");
        }
        if self
            .typed_config_uri
            .as_deref()
            .is_none_or(|uri| uri.trim().is_empty())
        {
            missing.push("typed config uri");
        }
        if self
            .typed_config_hash
            .as_deref()
            .is_none_or(|hash| hash.trim().is_empty())
        {
            missing.push("typed config hash");
        }
        if self.reviewer_policy_refs.is_empty() {
            missing.push("reviewer/policy refs");
        }
        if !self.non_live_boundary {
            missing.push("explicit non-live boundary");
        }
        missing
    }

    fn forbidden_behavior_violations(&self) -> Vec<String> {
        let mut violations = Vec::new();
        if self.requested_claim_is_incompatible_with_source_fidelity() {
            violations.push("proof-strength upgrade or fidelity-incompatible claim".to_string());
        }
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
            self.post_approval_actions
                .iter()
                .map(|action| action.description().to_string()),
        );
        violations
    }

    fn requested_claim_is_incompatible_with_source_fidelity(&self) -> bool {
        self.source_proof_refs.iter().any(|source_ref| {
            !source_fidelity_supports_claim(
                source_ref.fidelity_class,
                self.requested_claim_fidelity,
            )
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionPackageError {
    EmptyField {
        field: &'static str,
    },
    InvalidPackageVersion,
    InvalidSha256 {
        field: &'static str,
        value: String,
    },
    UnsupportedArtifactRoot {
        artifact_root: String,
    },
    ArtifactOutsidePromotionFamily {
        field: &'static str,
        artifact_root: String,
        uri: String,
        expected_prefix: String,
    },
    ApprovedForConfigMissing {
        missing: Vec<&'static str>,
    },
    ForbiddenPromotionPackageBehavior {
        violations: Vec<String>,
    },
}

impl fmt::Display for PromotionPackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => write!(formatter, "{field} must not be empty"),
            Self::InvalidPackageVersion => {
                write!(formatter, "package_version must be greater than zero")
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
            Self::ArtifactOutsidePromotionFamily {
                field,
                artifact_root,
                uri,
                expected_prefix,
            } => write!(
                formatter,
                "{field} {uri:?} is outside RA promotion package family for artifact_root {artifact_root:?}; expected prefix {expected_prefix:?}"
            ),
            Self::ApprovedForConfigMissing { missing } => write!(
                formatter,
                "approved_for_config missing required evidence/boundaries: {}",
                missing.join(", ")
            ),
            Self::ForbiddenPromotionPackageBehavior { violations } => write!(
                formatter,
                "promotion package contains forbidden behavior: {}",
                violations.join(", ")
            ),
        }
    }
}

impl Error for PromotionPackageError {}

fn validate_package_identity(package: &PromotionPackage) -> Result<(), PromotionPackageError> {
    if package.package_version == 0 {
        return Err(PromotionPackageError::InvalidPackageVersion);
    }
    validate_non_empty("artifact_root", &package.artifact_root)?;
    validate_non_empty("artifact_uri", &package.artifact_uri)?;
    if !package.artifact_root.starts_with("s3://") {
        return Err(PromotionPackageError::UnsupportedArtifactRoot {
            artifact_root: package.artifact_root.clone(),
        });
    }
    Ok(())
}

fn validate_promotion_family_uri(
    field: &'static str,
    artifact_root: &str,
    uri: &str,
) -> Result<(), PromotionPackageError> {
    validate_non_empty(field, uri)?;
    let expected_prefix = promotion_package_prefix(artifact_root);
    if uri.starts_with(&expected_prefix) {
        Ok(())
    } else {
        Err(PromotionPackageError::ArtifactOutsidePromotionFamily {
            field,
            artifact_root: artifact_root.to_string(),
            uri: uri.to_string(),
            expected_prefix,
        })
    }
}

fn promotion_package_prefix(artifact_root: &str) -> String {
    format!(
        "{}/{}/{}/{}/",
        artifact_root.trim_end_matches('/'),
        RESEARCH_ANALYTICS_KIND_PATH,
        RESEARCH_ANALYTICS_SCHEMA_VERSION,
        ResearchAnalyticsSubfamily::PromotionPackages.as_str()
    )
}

fn validate_non_empty(field: &'static str, value: &str) -> Result<(), PromotionPackageError> {
    if value.trim().is_empty() {
        Err(PromotionPackageError::EmptyField { field })
    } else {
        Ok(())
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), PromotionPackageError> {
    let is_sha256 = value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if is_sha256 {
        Ok(())
    } else {
        Err(PromotionPackageError::InvalidSha256 {
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
