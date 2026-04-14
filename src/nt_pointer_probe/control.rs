use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use chrono::NaiveDate;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const VALID_COVERAGE_CLASSES: &[&str] = &[
    "compile-time-api",
    "unit-behavior",
    "integration-behavior",
    "bootstrap-materialization",
    "serialization-contract",
    "network-transport",
    "timing-ordering",
];
const SHARED_NT_CRATE_PREFIXES: &[&str] = &[
    "crates/common/",
    "crates/core/",
    "crates/live/",
    "crates/model/",
    "crates/network/",
    "crates/persistence/",
    "crates/system/",
    "crates/trading/",
    "crates/execution/",
];

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlConfig {
    pub schema_version: u32,
    pub repo: String,
    pub default_branch: String,
    pub artifact_store_uri: String,
    pub artifact_retention_days: u32,
    pub max_safe_list_duration_days: i64,
    pub tag_soak_days: u32,
    pub paths: ControlPaths,
    pub status_checks: StatusChecks,
    pub develop_lane: DevelopLane,
    pub tagged_lane: TaggedLane,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPaths {
    pub registry: String,
    pub safe_list: String,
    pub replay_set: String,
    pub expected_branch_protection: String,
    pub advisory_issue_template: String,
    pub draft_pr_template: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusChecks {
    pub control_plane: String,
    pub self_test: String,
    pub develop: String,
    pub tagged: String,
    pub external_review: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DevelopLane {
    pub issue_label: String,
    pub issue_title_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaggedLane {
    pub pr_branch: String,
    pub pr_title_prefix: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryFile {
    pub schema_version: u32,
    pub coverage_classes: Vec<String>,
    pub seams: Vec<SeamEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeamEntry {
    pub name: String,
    pub risk: String,
    pub bolt_usage: Vec<String>,
    pub upstream_prefixes: Vec<String>,
    pub required_coverage: Vec<String>,
    pub escalation: EscalationMode,
    #[serde(default)]
    pub canaries: Vec<CanaryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryEntry {
    pub id: String,
    pub path: String,
    pub coverage: String,
    pub assertion: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EscalationMode {
    Fail,
    Ambiguous,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeListFile {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<SafeListEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeListEntry {
    pub path: String,
    #[serde(rename = "match")]
    pub match_kind: MatchKind,
    pub non_overlap_proof: String,
    pub approved_by: String,
    pub approved_at: String,
    pub revalidate_after: String,
    pub condition: SafeListCondition,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    Exact,
    Prefix,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeListCondition {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaySetFile {
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<ReplayEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayEntry {
    pub id: String,
    pub description: String,
    pub changed_paths: Vec<String>,
    pub expected_seams: Vec<String>,
    pub expected_result: ReplayExpectedResult,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReplayExpectedResult {
    Ambiguous,
    Fail,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedBranchProtection {
    pub schema_version: u32,
    pub branch: String,
    pub enforce_admins: bool,
    pub dismiss_stale_reviews: bool,
    pub require_code_owner_reviews: bool,
    pub required_approving_review_count: u64,
    pub required_status_checks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedControlPlane {
    pub repo_root: PathBuf,
    pub control: ControlConfig,
    pub registry: RegistryFile,
    pub safe_list: SafeListFile,
    pub replay_set: ReplaySetFile,
    pub expected_branch_protection: ExpectedBranchProtection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedBranchProtection {
    enforce_admins: bool,
    dismiss_stale_reviews: bool,
    require_code_owner_reviews: bool,
    required_approving_review_count: u64,
    required_status_checks: BTreeSet<String>,
}

impl LoadedControlPlane {
    pub fn load_from_repo_root(repo_root: &Path) -> Result<Self> {
        let control_path = repo_root.join("config/nt_pointer_probe/control.toml");
        let control: ControlConfig = load_toml(&control_path)?;
        control.validate()?;

        let registry: RegistryFile = load_toml(&repo_root.join(&control.paths.registry))?;
        let safe_list: SafeListFile = load_toml(&repo_root.join(&control.paths.safe_list))?;
        let replay_set: ReplaySetFile = load_toml(&repo_root.join(&control.paths.replay_set))?;
        let expected_branch_protection = ExpectedBranchProtection::load_and_validate(
            &repo_root.join(&control.paths.expected_branch_protection),
        )?;

        let loaded = Self {
            repo_root: repo_root.to_path_buf(),
            control,
            registry,
            safe_list,
            replay_set,
            expected_branch_protection,
        };
        loaded.validate()?;
        Ok(loaded)
    }

    pub fn validate(&self) -> Result<()> {
        validate_repo_relative(&self.control.paths.registry)?;
        validate_repo_relative(&self.control.paths.safe_list)?;
        validate_repo_relative(&self.control.paths.replay_set)?;
        validate_repo_relative(&self.control.paths.expected_branch_protection)?;
        validate_repo_relative(&self.control.paths.advisory_issue_template)?;
        validate_repo_relative(&self.control.paths.draft_pr_template)?;

        ensure!(
            self.repo_root
                .join(&self.control.paths.advisory_issue_template)
                .exists(),
            "missing advisory issue template {}",
            self.control.paths.advisory_issue_template
        );
        ensure!(
            self.repo_root
                .join(&self.control.paths.draft_pr_template)
                .exists(),
            "missing draft PR template {}",
            self.control.paths.draft_pr_template
        );

        self.registry.validate()?;
        self.safe_list
            .validate(self.control.max_safe_list_duration_days)?;
        self.replay_set.validate(&self.registry)?;
        self.expected_branch_protection
            .validate_against_control(&self.control)?;

        Ok(())
    }
}

impl ControlConfig {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported control schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        ensure!(
            !self.repo.trim().is_empty(),
            "control repo must not be empty"
        );
        ensure!(
            !self.default_branch.trim().is_empty(),
            "control default_branch must not be empty"
        );
        ensure!(
            self.artifact_store_uri.starts_with("s3://"),
            "artifact_store_uri must use s3://, got {}",
            self.artifact_store_uri
        );
        ensure!(
            self.artifact_retention_days > 0,
            "artifact_retention_days must be positive"
        );
        ensure!(
            self.max_safe_list_duration_days > 0,
            "max_safe_list_duration_days must be positive"
        );
        ensure!(self.tag_soak_days > 0, "tag_soak_days must be positive");

        let statuses = [
            self.status_checks.control_plane.as_str(),
            self.status_checks.self_test.as_str(),
            self.status_checks.develop.as_str(),
            self.status_checks.tagged.as_str(),
            self.status_checks.external_review.as_str(),
        ];
        ensure_unique_non_empty("status check", statuses)?;

        ensure!(
            !self.develop_lane.issue_label.trim().is_empty(),
            "develop lane issue_label must not be empty"
        );
        ensure!(
            !self.develop_lane.issue_title_prefix.trim().is_empty(),
            "develop lane issue_title_prefix must not be empty"
        );
        ensure!(
            !self.tagged_lane.pr_branch.trim().is_empty(),
            "tagged lane pr_branch must not be empty"
        );
        ensure!(
            !self.tagged_lane.pr_title_prefix.trim().is_empty(),
            "tagged lane pr_title_prefix must not be empty"
        );

        Ok(())
    }
}

impl RegistryFile {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported registry schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        ensure!(
            !self.coverage_classes.is_empty(),
            "registry coverage_classes must not be empty"
        );
        ensure_unique_non_empty(
            "coverage class",
            self.coverage_classes.iter().map(String::as_str),
        )?;
        for class in &self.coverage_classes {
            ensure!(
                VALID_COVERAGE_CLASSES.contains(&class.as_str()),
                "unsupported coverage class {class}"
            );
        }

        ensure!(!self.seams.is_empty(), "registry seams must not be empty");
        ensure_unique_non_empty(
            "seam name",
            self.seams.iter().map(|seam| seam.name.as_str()),
        )?;

        let mut canary_ids = BTreeSet::new();
        for seam in &self.seams {
            ensure!(
                !seam.risk.trim().is_empty(),
                "seam {} risk must not be empty",
                seam.name
            );
            ensure!(
                !seam.bolt_usage.is_empty(),
                "seam {} bolt_usage must not be empty",
                seam.name
            );
            ensure!(
                !seam.upstream_prefixes.is_empty(),
                "seam {} upstream_prefixes must not be empty",
                seam.name
            );
            ensure!(
                !seam.required_coverage.is_empty(),
                "seam {} required_coverage must not be empty",
                seam.name
            );
            for usage in &seam.bolt_usage {
                validate_repo_relative(usage)?;
            }
            for prefix in &seam.upstream_prefixes {
                validate_repo_relative(prefix)?;
            }
            for coverage in &seam.required_coverage {
                ensure!(
                    self.coverage_classes.contains(coverage),
                    "seam {} references unknown coverage class {}",
                    seam.name,
                    coverage
                );
            }
            ensure!(
                !seam.canaries.is_empty(),
                "seam {} must define at least one canary",
                seam.name
            );
            for canary in &seam.canaries {
                ensure!(
                    canary_ids.insert(canary.id.clone()),
                    "duplicate canary id {}",
                    canary.id
                );
                ensure!(
                    self.coverage_classes.contains(&canary.coverage),
                    "canary {} references unknown coverage class {}",
                    canary.id,
                    canary.coverage
                );
                ensure!(
                    seam.required_coverage.contains(&canary.coverage),
                    "canary {} uses coverage class {} not required by seam {}",
                    canary.id,
                    canary.coverage,
                    seam.name
                );
                validate_repo_relative(&canary.path)?;
                ensure!(
                    !canary.assertion.trim().is_empty(),
                    "canary {} assertion must not be empty",
                    canary.id
                );
            }
        }

        Ok(())
    }

    fn seam_names(&self) -> BTreeSet<&str> {
        self.seams.iter().map(|seam| seam.name.as_str()).collect()
    }
}

impl SafeListFile {
    fn validate(&self, max_safe_list_duration_days: i64) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported safe_list schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        for entry in &self.entries {
            validate_repo_relative(&entry.path)?;
            ensure!(
                !entry.non_overlap_proof.trim().is_empty(),
                "safe-list entry {} non_overlap_proof must not be empty",
                entry.path
            );
            ensure!(
                !entry.approved_by.trim().is_empty(),
                "safe-list entry {} approved_by must not be empty",
                entry.path
            );
            ensure!(
                !entry.condition.kind.trim().is_empty(),
                "safe-list entry {} condition.kind must not be empty",
                entry.path
            );
            ensure!(
                !entry.condition.value.trim().is_empty(),
                "safe-list entry {} condition.value must not be empty",
                entry.path
            );

            let approved_at = NaiveDate::parse_from_str(&entry.approved_at, "%Y-%m-%d")
                .with_context(|| {
                    format!(
                        "safe-list entry {} approved_at must use YYYY-MM-DD",
                        entry.path
                    )
                })?;
            let revalidate_after = NaiveDate::parse_from_str(&entry.revalidate_after, "%Y-%m-%d")
                .with_context(|| {
                format!(
                    "safe-list entry {} revalidate_after must use YYYY-MM-DD",
                    entry.path
                )
            })?;
            ensure!(
                revalidate_after >= approved_at,
                "safe-list entry {} revalidate_after must not precede approved_at",
                entry.path
            );
            ensure!(
                (revalidate_after - approved_at).num_days() <= max_safe_list_duration_days,
                "safe-list entry {} exceeds max_safe_list_duration_days",
                entry.path
            );

            let normalized = normalize_relative(&entry.path)?;
            let in_shared_crate = SHARED_NT_CRATE_PREFIXES.iter().any(|prefix| {
                normalized.starts_with(prefix) || normalized == prefix.trim_end_matches('/')
            });
            if in_shared_crate {
                ensure!(
                    entry.match_kind == MatchKind::Exact,
                    "shared NT crate safe-list entries must use exact match: {}",
                    entry.path
                );
            }
        }

        Ok(())
    }
}

impl ReplaySetFile {
    fn validate(&self, registry: &RegistryFile) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported replay_set schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        let seam_names = registry.seam_names();
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            ensure!(
                ids.insert(entry.id.clone()),
                "duplicate replay entry id {}",
                entry.id
            );
            ensure!(
                !entry.description.trim().is_empty(),
                "replay entry {} description must not be empty",
                entry.id
            );
            ensure!(
                !entry.changed_paths.is_empty(),
                "replay entry {} changed_paths must not be empty",
                entry.id
            );
            ensure!(
                !entry.expected_seams.is_empty(),
                "replay entry {} expected_seams must not be empty",
                entry.id
            );
            for path in &entry.changed_paths {
                validate_repo_relative(path)?;
            }
            for seam in &entry.expected_seams {
                ensure!(
                    seam_names.contains(seam.as_str()),
                    "replay entry {} references unknown seam {}",
                    entry.id,
                    seam
                );
            }
        }
        Ok(())
    }
}

impl ExpectedBranchProtection {
    pub fn load_and_validate(path: &Path) -> Result<Self> {
        let expected: Self = load_toml(path)?;
        expected.validate()?;
        Ok(expected)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported expected_branch_protection schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        ensure!(
            !self.branch.trim().is_empty(),
            "expected branch protection branch must not be empty"
        );
        ensure!(
            self.required_approving_review_count > 0,
            "required_approving_review_count must be positive"
        );
        ensure!(
            !self.required_status_checks.is_empty(),
            "required_status_checks must not be empty"
        );
        ensure_unique_non_empty(
            "expected branch protection status check",
            self.required_status_checks.iter().map(String::as_str),
        )?;
        Ok(())
    }

    fn validate_against_control(&self, control: &ControlConfig) -> Result<()> {
        self.validate()?;
        ensure!(
            self.branch == control.default_branch,
            "expected branch protection branch {} must match control default_branch {}",
            self.branch,
            control.default_branch
        );

        for required in [
            control.status_checks.control_plane.as_str(),
            control.status_checks.self_test.as_str(),
        ] {
            ensure!(
                self.required_status_checks
                    .iter()
                    .any(|status| status == required),
                "expected branch protection must require status check {}",
                required
            );
        }

        Ok(())
    }
}

pub fn compare_branch_protection_response(
    expected: &ExpectedBranchProtection,
    actual_json: &str,
) -> Result<()> {
    let actual = normalize_branch_protection_response(actual_json)?;
    let expected_normalized = NormalizedBranchProtection {
        enforce_admins: expected.enforce_admins,
        dismiss_stale_reviews: expected.dismiss_stale_reviews,
        require_code_owner_reviews: expected.require_code_owner_reviews,
        required_approving_review_count: expected.required_approving_review_count,
        required_status_checks: expected.required_status_checks.iter().cloned().collect(),
    };

    ensure!(
        actual.enforce_admins == expected_normalized.enforce_admins,
        "branch protection drift: enforce_admins expected {}, got {}",
        expected_normalized.enforce_admins,
        actual.enforce_admins
    );
    ensure!(
        actual.dismiss_stale_reviews == expected_normalized.dismiss_stale_reviews,
        "branch protection drift: dismiss_stale_reviews expected {}, got {}",
        expected_normalized.dismiss_stale_reviews,
        actual.dismiss_stale_reviews
    );
    ensure!(
        actual.require_code_owner_reviews == expected_normalized.require_code_owner_reviews,
        "branch protection drift: require_code_owner_reviews expected {}, got {}",
        expected_normalized.require_code_owner_reviews,
        actual.require_code_owner_reviews
    );
    ensure!(
        actual.required_approving_review_count
            == expected_normalized.required_approving_review_count,
        "branch protection drift: required_approving_review_count expected {}, got {}",
        expected_normalized.required_approving_review_count,
        actual.required_approving_review_count
    );
    ensure!(
        actual.required_status_checks == expected_normalized.required_status_checks,
        "branch protection drift: required status checks differ (expected {:?}, got {:?})",
        expected_normalized.required_status_checks,
        actual.required_status_checks
    );

    Ok(())
}

fn normalize_branch_protection_response(actual_json: &str) -> Result<NormalizedBranchProtection> {
    let value: Value =
        serde_json::from_str(actual_json).context("failed to parse branch protection JSON")?;

    if value
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| message == "Branch not protected")
    {
        return Err(anyhow!(
            "branch protection drift: expected protected branch, got unprotected branch"
        ));
    }

    let contexts = value
        .get("required_status_checks")
        .and_then(|checks| checks.get("contexts"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            anyhow!("branch protection response missing required_status_checks.contexts")
        })?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("branch protection response contains non-string status"))
        })
        .collect::<Result<BTreeSet<_>>>()?;

    let reviews = value.get("required_pull_request_reviews").ok_or_else(|| {
        anyhow!("branch protection response missing required_pull_request_reviews")
    })?;

    Ok(NormalizedBranchProtection {
        enforce_admins: value
            .get("enforce_admins")
            .and_then(|admins| admins.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing enforce_admins.enabled"))?,
        dismiss_stale_reviews: reviews
            .get("dismiss_stale_reviews")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing dismiss_stale_reviews"))?,
        require_code_owner_reviews: reviews
            .get("require_code_owner_reviews")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("branch protection response missing require_code_owner_reviews")
            })?,
        required_approving_review_count: reviews
            .get("required_approving_review_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                anyhow!("branch protection response missing required_approving_review_count")
            })?,
        required_status_checks: contexts,
    })
}

fn load_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read control artifact {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse control artifact {}", path.display()))
}

fn ensure_unique_non_empty<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a str>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        ensure!(!value.trim().is_empty(), "{label} must not be empty");
        ensure!(seen.insert(value), "duplicate {label}: {value}");
    }
    Ok(())
}

fn validate_repo_relative(path: &str) -> Result<()> {
    let _ = normalize_relative(path)?;
    Ok(())
}

fn normalize_relative(path: &str) -> Result<String> {
    ensure!(!path.trim().is_empty(), "path must not be empty");
    let path = Path::new(path);
    ensure!(
        !path.is_absolute(),
        "path must be repo-relative: {}",
        path.display()
    );
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(anyhow!(
                    "path must not contain parent traversal: {}",
                    path.display()
                ));
            }
            other => {
                return Err(anyhow!(
                    "path contains unsupported component {:?}: {}",
                    other,
                    path.display()
                ));
            }
        }
    }
    let normalized = normalized.to_string_lossy().replace('\\', "/");
    ensure!(
        !normalized.is_empty(),
        "path must not normalize to an empty value"
    );
    Ok(normalized)
}
