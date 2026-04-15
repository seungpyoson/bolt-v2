use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use chrono::{NaiveDate, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_yaml::Value as YamlValue;
use toml::Value as TomlValue;

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
fn default_required_status_check_floor() -> Vec<String> {
    vec![
        "nt-pointer-control-plane".to_string(),
        "nt-pointer-probe-self-test".to_string(),
    ]
}

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
    pub nt_crates: Vec<String>,
    #[serde(default = "default_required_status_check_floor")]
    pub required_status_check_floor: Vec<String>,
    pub paths: ControlPaths,
    pub status_checks: StatusChecks,
    pub develop_lane: DevelopLane,
    pub drift_lane: DriftLane,
    pub tagged_lane: TaggedLane,
    pub guard_contract: GuardContract,
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
pub struct DriftLane {
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
pub struct GuardContract {
    pub script_path: String,
    pub script_sha256: String,
    pub justfile_path: String,
    pub justfile_sha256: String,
    pub owner_require_script_path: String,
    pub owner_require_script_sha256: String,
    pub owner_install_script_path: String,
    pub owner_install_script_sha256: String,
    pub setup_environment_action_path: String,
    pub setup_environment_action_sha256: String,
    pub control_plane_workflow: String,
    pub control_plane_job: String,
    pub control_plane_job_sha256: String,
    pub self_test_workflow: String,
    pub self_test_job: String,
    pub self_test_job_sha256: String,
    pub dependabot_workflow: String,
    pub dependabot_job: String,
    pub dependabot_job_sha256: String,
    pub drift_workflow: String,
    pub drift_job: String,
    pub drift_job_sha256: String,
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    Exact,
    Prefix,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeListCondition {
    pub kind: SafeListConditionKind,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SafeListConditionKind {
    UpstreamPathKind,
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
    pub allow_deletions: bool,
    pub allow_force_pushes: bool,
    pub block_creations: bool,
    pub dismiss_stale_reviews: bool,
    pub required_linear_history: bool,
    pub required_conversation_resolution: bool,
    pub lock_branch: bool,
    pub require_signed_commits: bool,
    pub require_code_owner_reviews: bool,
    pub required_approving_review_count: u64,
    pub strict_required_status_checks: bool,
    pub required_status_checks: Vec<String>,
    #[serde(default)]
    pub required_status_check_app_ids: BTreeMap<String, u64>,
    #[serde(default)]
    pub required_effective_rules: Vec<ExpectedEffectiveRule>,
    #[serde(default)]
    pub required_rulesets: Vec<ExpectedRuleset>,
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
    allow_deletions: bool,
    allow_force_pushes: bool,
    block_creations: bool,
    dismiss_stale_reviews: bool,
    required_linear_history: bool,
    required_conversation_resolution: bool,
    lock_branch: bool,
    require_signed_commits: bool,
    require_code_owner_reviews: bool,
    required_approving_review_count: u64,
    required_status_checks: BTreeSet<String>,
    required_status_check_app_ids: BTreeMap<String, u64>,
    strict_required_status_checks: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExpectedEffectiveRule {
    Deletion,
    NonFastForward,
    PullRequest {
        required_approving_review_count: u64,
        dismiss_stale_reviews_on_push: bool,
        require_code_owner_review: bool,
        require_last_push_approval: bool,
        required_review_thread_resolution: bool,
        allowed_merge_methods: Vec<String>,
    },
    RequiredStatusChecks {
        strict_required_status_checks_policy: bool,
        required_status_checks: Vec<String>,
        #[serde(default)]
        required_status_check_integration_ids: BTreeMap<String, u64>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRuleset {
    pub id: u64,
    pub name: String,
    pub enforcement: String,
    #[serde(default)]
    pub allowed_bypass_actors: Vec<String>,
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

        self.validate_nt_crate_inventory()?;
        self.validate_guard_contract()?;
        self.registry.validate(&self.repo_root)?;
        self.safe_list
            .validate(self.control.max_safe_list_duration_days, &self.registry)?;
        self.replay_set.validate(&self.registry)?;
        self.expected_branch_protection
            .validate_against_control(&self.control)?;

        Ok(())
    }

    pub fn nt_crate_diff_pattern(&self) -> String {
        let mut literals: Vec<String> = self
            .control
            .nt_crates
            .iter()
            .map(|name| regex_escape_literal(name))
            .collect();
        literals.push("nautilus_trader\\.git".to_string());
        format!("^[+-].*({})", literals.join("|"))
    }

    pub fn ensure_no_nt_mutation_from_git_refs(
        &self,
        base_ref: &str,
        head_ref: &str,
    ) -> Result<()> {
        let configured: BTreeSet<String> = self.control.nt_crates.iter().cloned().collect();
        let changed_files = git_changed_files(&self.repo_root, base_ref, head_ref)?;
        let mut reasons = Vec::new();

        for path in changed_files {
            if !is_nt_guarded_surface(&path) {
                continue;
            }

            if path.ends_with("Cargo.toml") {
                let before = extract_nt_dependency_records_from_cargo_toml(
                    &git_show_toml_or_empty(&self.repo_root, base_ref, &path)?,
                    &configured,
                );
                let after = extract_nt_dependency_records_from_cargo_toml(
                    &git_show_toml_or_empty(&self.repo_root, head_ref, &path)?,
                    &configured,
                );
                if before != after {
                    reasons.push(format!(
                        "{} changed NT dependency records (before {:?}, after {:?})",
                        path, before, after
                    ));
                }
                continue;
            }

            if path.ends_with("Cargo.lock") {
                let before = extract_nt_lock_records(
                    &git_show_text_or_empty(&self.repo_root, base_ref, &path)?,
                    &configured,
                )?;
                let after = extract_nt_lock_records(
                    &git_show_text_or_empty(&self.repo_root, head_ref, &path)?,
                    &configured,
                )?;
                if before != after {
                    reasons.push(format!("{} changed NT lock records", path));
                }
                continue;
            }

            if is_cargo_config_path(&path) {
                let before = extract_guarded_cargo_config_state(&git_show_toml_or_empty(
                    &self.repo_root,
                    base_ref,
                    &path,
                )?);
                let after = extract_guarded_cargo_config_state(&git_show_toml_or_empty(
                    &self.repo_root,
                    head_ref,
                    &path,
                )?);
                if before != after {
                    reasons.push(format!("{} changed guarded cargo config state", path));
                }
            }
        }

        ensure!(
            reasons.is_empty(),
            "NT pin changes are blocked until the probe-generated path is active:\n{}",
            reasons
                .into_iter()
                .map(|reason| format!("  - {}", reason))
                .collect::<Vec<_>>()
                .join("\n")
        );

        Ok(())
    }

    fn validate_nt_crate_inventory(&self) -> Result<()> {
        let configured: BTreeSet<String> = self.control.nt_crates.iter().cloned().collect();
        let cargo_toml: TomlValue = load_toml(&self.repo_root.join("Cargo.toml"))?;
        let cargo_nt_crates = extract_nt_crates_from_cargo_toml(&cargo_toml, &configured);

        ensure!(
            cargo_nt_crates == configured,
            "configured nt_crates {:?} do not match Cargo.toml NT crates {:?}",
            configured,
            cargo_nt_crates
        );

        let dependabot_blocks = extract_nt_ignores_from_dependabot_cargo_blocks(
            &fs::read_to_string(self.repo_root.join(".github/dependabot.yml"))
                .context("failed to read .github/dependabot.yml")?,
        )?;
        ensure!(
            !dependabot_blocks.is_empty(),
            "Dependabot must declare at least one cargo updates block"
        );
        for (directory, ignores) in dependabot_blocks {
            ensure!(
                ignores == configured,
                "configured nt_crates {:?} do not match Dependabot NT ignores {:?} in cargo block {}",
                configured,
                ignores,
                directory
            );
        }

        Ok(())
    }

    fn validate_guard_contract(&self) -> Result<()> {
        let script_path = self
            .repo_root
            .join(&self.control.guard_contract.script_path);
        validate_repo_path_exists(&self.repo_root, &self.control.guard_contract.script_path)?;
        let script_hash = sha256_hex(
            &fs::read(&script_path)
                .with_context(|| format!("failed to read {}", script_path.display()))?,
        );
        ensure!(
            script_hash == self.control.guard_contract.script_sha256,
            "guard script hash drift for {}",
            self.control.guard_contract.script_path
        );

        for (path, expected_hash, label) in [
            (
                self.control.guard_contract.justfile_path.as_str(),
                self.control.guard_contract.justfile_sha256.as_str(),
                "justfile",
            ),
            (
                self.control
                    .guard_contract
                    .owner_require_script_path
                    .as_str(),
                self.control
                    .guard_contract
                    .owner_require_script_sha256
                    .as_str(),
                "owner require script",
            ),
            (
                self.control
                    .guard_contract
                    .owner_install_script_path
                    .as_str(),
                self.control
                    .guard_contract
                    .owner_install_script_sha256
                    .as_str(),
                "owner install script",
            ),
            (
                self.control
                    .guard_contract
                    .setup_environment_action_path
                    .as_str(),
                self.control
                    .guard_contract
                    .setup_environment_action_sha256
                    .as_str(),
                "setup-environment action",
            ),
        ] {
            validate_repo_path_exists(&self.repo_root, path)?;
            let actual_hash = sha256_hex(
                &fs::read(self.repo_root.join(path))
                    .with_context(|| format!("failed to read {}", path))?,
            );
            ensure!(
                actual_hash == expected_hash,
                "{} hash drift for {}",
                label,
                path
            );
        }

        let control_plane_workflow = fs::read_to_string(
            self.repo_root
                .join(&self.control.guard_contract.control_plane_workflow),
        )
        .with_context(|| {
            format!(
                "failed to read {}",
                self.control.guard_contract.control_plane_workflow
            )
        })?;
        ensure!(
            workflow_job_matches_hash(
                &control_plane_workflow,
                &self.control.guard_contract.control_plane_job,
                &self.control.guard_contract.control_plane_job_sha256,
            )?,
            "{} must keep the exact guard job contract",
            self.control.guard_contract.control_plane_workflow
        );

        let self_test_workflow = fs::read_to_string(
            self.repo_root
                .join(&self.control.guard_contract.self_test_workflow),
        )
        .with_context(|| {
            format!(
                "failed to read {}",
                self.control.guard_contract.self_test_workflow
            )
        })?;
        ensure!(
            workflow_job_matches_hash(
                &self_test_workflow,
                &self.control.guard_contract.self_test_job,
                &self.control.guard_contract.self_test_job_sha256,
            )?,
            "{} must keep the exact guard job contract",
            self.control.guard_contract.self_test_workflow
        );

        let dependabot_workflow = fs::read_to_string(
            self.repo_root
                .join(&self.control.guard_contract.dependabot_workflow),
        )
        .with_context(|| {
            format!(
                "failed to read {}",
                self.control.guard_contract.dependabot_workflow
            )
        })?;
        ensure!(
            workflow_job_matches_hash(
                &dependabot_workflow,
                &self.control.guard_contract.dependabot_job,
                &self.control.guard_contract.dependabot_job_sha256,
            )?,
            "{} must keep the exact guard job contract",
            self.control.guard_contract.dependabot_workflow
        );

        let drift_workflow = fs::read_to_string(
            self.repo_root
                .join(&self.control.guard_contract.drift_workflow),
        )
        .with_context(|| {
            format!(
                "failed to read {}",
                self.control.guard_contract.drift_workflow
            )
        })?;
        ensure!(
            workflow_job_matches_hash(
                &drift_workflow,
                &self.control.guard_contract.drift_job,
                &self.control.guard_contract.drift_job_sha256,
            )?,
            "{} must keep the exact guard job contract",
            self.control.guard_contract.drift_workflow
        );

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
        ensure!(!self.nt_crates.is_empty(), "nt_crates must not be empty");
        ensure_unique_non_empty("nt crate", self.nt_crates.iter().map(String::as_str))?;
        for nt_crate in &self.nt_crates {
            ensure!(
                nt_crate.starts_with("nautilus-"),
                "nt_crate must start with nautilus-: {}",
                nt_crate
            );
        }

        ensure!(
            !self.required_status_check_floor.is_empty(),
            "required_status_check_floor must not be empty"
        );
        ensure_unique_non_empty(
            "required status check floor entry",
            self.required_status_check_floor.iter().map(String::as_str),
        )?;

        validate_repo_relative(&self.paths.registry)?;
        validate_repo_relative(&self.paths.safe_list)?;
        validate_repo_relative(&self.paths.replay_set)?;
        validate_repo_relative(&self.paths.expected_branch_protection)?;
        validate_repo_relative(&self.paths.advisory_issue_template)?;
        validate_repo_relative(&self.paths.draft_pr_template)?;
        validate_repo_relative(&self.guard_contract.script_path)?;
        validate_repo_relative(&self.guard_contract.justfile_path)?;
        validate_repo_relative(&self.guard_contract.owner_require_script_path)?;
        validate_repo_relative(&self.guard_contract.owner_install_script_path)?;
        validate_repo_relative(&self.guard_contract.setup_environment_action_path)?;
        validate_repo_relative(&self.guard_contract.control_plane_workflow)?;
        validate_repo_relative(&self.guard_contract.self_test_workflow)?;
        validate_repo_relative(&self.guard_contract.dependabot_workflow)?;
        validate_repo_relative(&self.guard_contract.drift_workflow)?;

        let statuses = [
            self.status_checks.control_plane.as_str(),
            self.status_checks.self_test.as_str(),
            self.status_checks.develop.as_str(),
            self.status_checks.tagged.as_str(),
            self.status_checks.external_review.as_str(),
        ];
        ensure_unique_non_empty("status check", statuses)?;
        for required in &self.required_status_check_floor {
            ensure!(
                statuses.contains(&required.as_str()),
                "required_status_check_floor references unknown status check {}",
                required
            );
        }

        ensure!(
            !self.develop_lane.issue_label.trim().is_empty(),
            "develop lane issue_label must not be empty"
        );
        ensure!(
            !self.develop_lane.issue_title_prefix.trim().is_empty(),
            "develop lane issue_title_prefix must not be empty"
        );
        ensure!(
            !self.drift_lane.issue_label.trim().is_empty(),
            "drift lane issue_label must not be empty"
        );
        ensure!(
            !self.drift_lane.issue_title_prefix.trim().is_empty(),
            "drift lane issue_title_prefix must not be empty"
        );
        ensure!(
            !self.tagged_lane.pr_branch.trim().is_empty(),
            "tagged lane pr_branch must not be empty"
        );
        ensure!(
            !self.tagged_lane.pr_title_prefix.trim().is_empty(),
            "tagged lane pr_title_prefix must not be empty"
        );
        for field in [
            self.guard_contract.script_sha256.as_str(),
            self.guard_contract.justfile_path.as_str(),
            self.guard_contract.justfile_sha256.as_str(),
            self.guard_contract.owner_require_script_path.as_str(),
            self.guard_contract.owner_require_script_sha256.as_str(),
            self.guard_contract.owner_install_script_path.as_str(),
            self.guard_contract.owner_install_script_sha256.as_str(),
            self.guard_contract.setup_environment_action_path.as_str(),
            self.guard_contract.setup_environment_action_sha256.as_str(),
            self.guard_contract.self_test_workflow.as_str(),
            self.guard_contract.self_test_job.as_str(),
            self.guard_contract.self_test_job_sha256.as_str(),
            self.guard_contract.control_plane_job.as_str(),
            self.guard_contract.control_plane_job_sha256.as_str(),
            self.guard_contract.dependabot_job.as_str(),
            self.guard_contract.dependabot_job_sha256.as_str(),
            self.guard_contract.drift_workflow.as_str(),
            self.guard_contract.drift_job.as_str(),
            self.guard_contract.drift_job_sha256.as_str(),
        ] {
            ensure!(
                !field.trim().is_empty(),
                "guard contract fields must not be empty"
            );
        }

        Ok(())
    }
}

impl RegistryFile {
    fn validate(&self, repo_root: &Path) -> Result<()> {
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
                validate_repo_path_exists(repo_root, usage)?;
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
                validate_repo_path_exists(repo_root, &canary.path)?;
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
    fn validate(&self, max_safe_list_duration_days: i64, registry: &RegistryFile) -> Result<()> {
        ensure!(
            self.schema_version == CURRENT_SCHEMA_VERSION,
            "unsupported safe_list schema_version {}, expected {CURRENT_SCHEMA_VERSION}",
            self.schema_version
        );
        let today = Utc::now().date_naive();
        let mut seen = BTreeSet::new();
        let protected_roots = derive_protected_safe_list_roots(registry);
        let used_upstream_prefixes = registry
            .seams
            .iter()
            .flat_map(|seam| seam.upstream_prefixes.iter())
            .map(|prefix| normalize_relative(prefix))
            .collect::<Result<BTreeSet<_>>>()?;
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
            ensure!(
                revalidate_after >= today,
                "safe-list entry {} is expired",
                entry.path
            );

            let normalized = normalize_relative(&entry.path)?;
            ensure!(
                seen.insert((normalized.clone(), entry.match_kind)),
                "duplicate safe-list entry for {} with match kind {:?}",
                normalized,
                entry.match_kind
            );
            let in_shared_crate = protected_roots
                .iter()
                .any(|prefix| path_overlaps_prefix(&normalized, prefix));
            if in_shared_crate {
                ensure!(
                    entry.match_kind == MatchKind::Exact,
                    "shared NT crate safe-list entries must use exact match: {}",
                    entry.path
                );
            }
            validate_safe_list_condition(entry, &normalized, &used_upstream_prefixes)?;
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
        ensure!(
            !self.required_status_check_app_ids.is_empty(),
            "required_status_check_app_ids must not be empty"
        );
        ensure!(
            !self.required_effective_rules.is_empty(),
            "required_effective_rules must not be empty"
        );
        ensure_unique_non_empty(
            "expected branch protection status check",
            self.required_status_checks.iter().map(String::as_str),
        )?;
        let required_status_checks = self
            .required_status_checks
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let app_id_status_checks = self
            .required_status_check_app_ids
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        ensure!(
            app_id_status_checks == required_status_checks,
            "required_status_check_app_ids keys {:?} must match required_status_checks {:?}",
            app_id_status_checks,
            required_status_checks
        );
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

        for required in &control.required_status_check_floor {
            ensure!(
                self.required_status_checks
                    .iter()
                    .any(|status| status == required),
                "expected branch protection must require status check {}",
                required
            );
        }

        let effective_required_status_checks = self
            .required_effective_rules
            .iter()
            .filter_map(|rule| match rule {
                ExpectedEffectiveRule::RequiredStatusChecks {
                    required_status_checks,
                    ..
                } => Some(required_status_checks),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();

        let classic_required_status_checks = self
            .required_status_checks
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        ensure!(
            effective_required_status_checks == classic_required_status_checks,
            "required_effective_rules status checks {:?} must match classic required_status_checks {:?}",
            effective_required_status_checks,
            classic_required_status_checks
        );

        for integration_ids in self
            .required_effective_rules
            .iter()
            .filter_map(|rule| match rule {
                ExpectedEffectiveRule::RequiredStatusChecks {
                    required_status_check_integration_ids,
                    ..
                } if !required_status_check_integration_ids.is_empty() => {
                    Some(required_status_check_integration_ids)
                }
                _ => None,
            })
        {
            let effective_app_id_status_checks =
                integration_ids.keys().cloned().collect::<BTreeSet<_>>();
            ensure!(
                effective_app_id_status_checks == classic_required_status_checks,
                "required_effective_rules required_status_check_integration_ids keys {:?} must match classic required_status_checks {:?}",
                effective_app_id_status_checks,
                classic_required_status_checks
            );
            ensure!(
                integration_ids == &self.required_status_check_app_ids,
                "required_effective_rules required_status_check_integration_ids {:?} must match classic required_status_check_app_ids {:?}",
                integration_ids,
                self.required_status_check_app_ids
            );
        }

        for pull_request_rule in
            self.required_effective_rules
                .iter()
                .filter_map(|rule| match rule {
                    ExpectedEffectiveRule::PullRequest {
                        required_approving_review_count,
                        ..
                    } => Some(*required_approving_review_count),
                    _ => None,
                })
        {
            ensure!(
                pull_request_rule == self.required_approving_review_count,
                "required_effective_rules pull_request required_approving_review_count {} must match classic required_approving_review_count {}",
                pull_request_rule,
                self.required_approving_review_count
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
        allow_deletions: expected.allow_deletions,
        allow_force_pushes: expected.allow_force_pushes,
        block_creations: expected.block_creations,
        dismiss_stale_reviews: expected.dismiss_stale_reviews,
        required_linear_history: expected.required_linear_history,
        required_conversation_resolution: expected.required_conversation_resolution,
        lock_branch: expected.lock_branch,
        require_signed_commits: expected.require_signed_commits,
        require_code_owner_reviews: expected.require_code_owner_reviews,
        required_approving_review_count: expected.required_approving_review_count,
        required_status_checks: expected.required_status_checks.iter().cloned().collect(),
        required_status_check_app_ids: expected.required_status_check_app_ids.clone(),
        strict_required_status_checks: expected.strict_required_status_checks,
    };

    ensure!(
        actual.enforce_admins == expected_normalized.enforce_admins,
        "branch protection drift: enforce_admins expected {}, got {}",
        expected_normalized.enforce_admins,
        actual.enforce_admins
    );
    ensure!(
        actual.allow_deletions == expected_normalized.allow_deletions,
        "branch protection drift: allow_deletions expected {}, got {}",
        expected_normalized.allow_deletions,
        actual.allow_deletions
    );
    ensure!(
        actual.allow_force_pushes == expected_normalized.allow_force_pushes,
        "branch protection drift: allow_force_pushes expected {}, got {}",
        expected_normalized.allow_force_pushes,
        actual.allow_force_pushes
    );
    ensure!(
        actual.block_creations == expected_normalized.block_creations,
        "branch protection drift: block_creations expected {}, got {}",
        expected_normalized.block_creations,
        actual.block_creations
    );
    ensure!(
        actual.dismiss_stale_reviews == expected_normalized.dismiss_stale_reviews,
        "branch protection drift: dismiss_stale_reviews expected {}, got {}",
        expected_normalized.dismiss_stale_reviews,
        actual.dismiss_stale_reviews
    );
    ensure!(
        actual.required_linear_history == expected_normalized.required_linear_history,
        "branch protection drift: required_linear_history expected {}, got {}",
        expected_normalized.required_linear_history,
        actual.required_linear_history
    );
    ensure!(
        actual.required_conversation_resolution
            == expected_normalized.required_conversation_resolution,
        "branch protection drift: required_conversation_resolution expected {}, got {}",
        expected_normalized.required_conversation_resolution,
        actual.required_conversation_resolution
    );
    ensure!(
        actual.lock_branch == expected_normalized.lock_branch,
        "branch protection drift: lock_branch expected {}, got {}",
        expected_normalized.lock_branch,
        actual.lock_branch
    );
    ensure!(
        actual.require_signed_commits == expected_normalized.require_signed_commits,
        "branch protection drift: require_signed_commits expected {}, got {}",
        expected_normalized.require_signed_commits,
        actual.require_signed_commits
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
    ensure!(
        actual.required_status_check_app_ids == expected_normalized.required_status_check_app_ids,
        "branch protection drift: required status check app ids differ (expected {:?}, got {:?})",
        expected_normalized.required_status_check_app_ids,
        actual.required_status_check_app_ids
    );
    ensure!(
        actual.strict_required_status_checks == expected_normalized.strict_required_status_checks,
        "branch protection drift: strict status-check policy expected {}, got {}",
        expected_normalized.strict_required_status_checks,
        actual.strict_required_status_checks
    );

    Ok(())
}

pub fn compare_branch_governance_responses(
    expected: &ExpectedBranchProtection,
    actual_branch_protection_json: &str,
    actual_rules_json: &str,
    actual_ruleset_details_json: &str,
) -> Result<()> {
    compare_branch_protection_response(expected, actual_branch_protection_json)?;

    let actual_rules = normalize_effective_rules_response(actual_rules_json)?;
    let expected_rules = expected
        .required_effective_rules
        .iter()
        .map(expected_effective_rule_signature)
        .collect::<BTreeSet<_>>();

    ensure!(
        actual_rules == expected_rules,
        "branch governance drift: effective rules differ (expected {:?}, got {:?})",
        expected_rules,
        actual_rules
    );

    let actual_rulesets = normalize_ruleset_details_response(actual_ruleset_details_json)?;
    let expected_rulesets = expected
        .required_rulesets
        .iter()
        .map(expected_ruleset_signature)
        .collect::<BTreeSet<_>>();

    ensure!(
        actual_rulesets == expected_rulesets,
        "branch governance drift: ruleset details differ (expected {:?}, got {:?})",
        expected_rulesets,
        actual_rulesets
    );

    Ok(())
}

fn normalize_ruleset_details_response(actual_json: &str) -> Result<BTreeSet<String>> {
    let rulesets: Value =
        serde_json::from_str(actual_json).context("failed to parse ruleset details JSON")?;
    let rulesets = rulesets
        .as_array()
        .ok_or_else(|| anyhow!("ruleset details response must be an array"))?;

    let mut signatures = BTreeSet::new();
    for ruleset in rulesets {
        let id = ruleset
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow!("ruleset detail missing id"))?;
        let name = ruleset
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ruleset detail missing name"))?;
        let enforcement = ruleset
            .get("enforcement")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("ruleset detail missing enforcement"))?;
        let bypass_actors = ruleset
            .get("bypass_actors")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("ruleset detail missing bypass_actors"))?
            .iter()
            .map(|actor| {
                let actor_id = actor
                    .get("actor_id")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("bypass actor missing actor_id"))?;
                let actor_type = actor
                    .get("actor_type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("bypass actor missing actor_type"))?;
                let bypass_mode = actor
                    .get("bypass_mode")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("bypass actor missing bypass_mode"))?;
                Ok(format!("{}:{}:{}", actor_type, actor_id, bypass_mode))
            })
            .collect::<Result<BTreeSet<_>>>()?;

        signatures.insert(format!(
            "ruleset|id={}|name={}|enforcement={}|bypass_actors={}",
            id,
            name,
            enforcement,
            bypass_actors.into_iter().collect::<Vec<_>>().join(",")
        ));
    }

    Ok(signatures)
}

fn normalize_branch_protection_response(actual_json: &str) -> Result<NormalizedBranchProtection> {
    let value: Value =
        serde_json::from_str(actual_json).context("failed to parse branch protection JSON")?;

    if let Some(message) = value.get("message").and_then(Value::as_str) {
        let has_branch_protection_fields = value.get("required_status_checks").is_some()
            || value.get("required_pull_request_reviews").is_some()
            || value.get("enforce_admins").is_some();
        if message == "Branch not protected" {
            return Err(anyhow!(
                "branch protection drift: expected protected branch, got unprotected branch"
            ));
        }

        if !has_branch_protection_fields {
            return Err(anyhow!("branch protection API error: {}", message));
        }
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
    let check_app_ids = value
        .get("required_status_checks")
        .and_then(|checks| checks.get("checks"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("branch protection response missing required_status_checks.checks"))?
        .iter()
        .map(|entry| {
            let context = entry
                .get("context")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("required_status_checks.check entry missing context"))?;
            let app_id = entry
                .get("app_id")
                .and_then(Value::as_u64)
                .ok_or_else(|| anyhow!("required_status_checks.check entry missing app_id"))?;
            Ok((context.to_string(), app_id))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    let reviews = value.get("required_pull_request_reviews").ok_or_else(|| {
        anyhow!("branch protection response missing required_pull_request_reviews")
    })?;

    Ok(NormalizedBranchProtection {
        enforce_admins: value
            .get("enforce_admins")
            .and_then(|admins| admins.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing enforce_admins.enabled"))?,
        allow_deletions: value
            .get("allow_deletions")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing allow_deletions.enabled"))?,
        allow_force_pushes: value
            .get("allow_force_pushes")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("branch protection response missing allow_force_pushes.enabled")
            })?,
        block_creations: value
            .get("block_creations")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing block_creations.enabled"))?,
        dismiss_stale_reviews: reviews
            .get("dismiss_stale_reviews")
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing dismiss_stale_reviews"))?,
        required_linear_history: value
            .get("required_linear_history")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("branch protection response missing required_linear_history.enabled")
            })?,
        required_conversation_resolution: value
            .get("required_conversation_resolution")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!(
                    "branch protection response missing required_conversation_resolution.enabled"
                )
            })?,
        lock_branch: value
            .get("lock_branch")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| anyhow!("branch protection response missing lock_branch.enabled"))?,
        require_signed_commits: value
            .get("required_signatures")
            .and_then(|field| field.get("enabled"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("branch protection response missing required_signatures.enabled")
            })?,
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
        required_status_check_app_ids: check_app_ids,
        strict_required_status_checks: value
            .get("required_status_checks")
            .and_then(|checks| checks.get("strict"))
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                anyhow!("branch protection response missing required_status_checks.strict")
            })?,
    })
}

fn normalize_effective_rules_response(actual_json: &str) -> Result<BTreeSet<String>> {
    let rules: Value =
        serde_json::from_str(actual_json).context("failed to parse effective rules JSON")?;
    let rules = rules
        .as_array()
        .ok_or_else(|| anyhow!("effective rules response must be an array"))?;

    let mut signatures = BTreeSet::new();
    for rule in rules {
        let Some(rule_type) = rule.get("type").and_then(Value::as_str) else {
            return Err(anyhow!("effective rule is missing type"));
        };
        let signature = match rule_type {
            "deletion" => "deletion".to_string(),
            "non_fast_forward" => "non_fast_forward".to_string(),
            "pull_request" => {
                let parameters = rule
                    .get("parameters")
                    .ok_or_else(|| anyhow!("pull_request rule missing parameters"))?;
                let allowed_merge_methods = parameters
                    .get("allowed_merge_methods")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("pull_request rule missing allowed_merge_methods"))?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| anyhow!("allowed_merge_methods entry must be a string"))
                    })
                    .collect::<Result<BTreeSet<_>>>()?;
                format!(
                    "pull_request|required_approving_review_count={}|dismiss_stale_reviews_on_push={}|require_code_owner_review={}|require_last_push_approval={}|required_review_thread_resolution={}|allowed_merge_methods={}",
                    parameters
                        .get("required_approving_review_count")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| anyhow!(
                            "pull_request rule missing required_approving_review_count"
                        ))?,
                    parameters
                        .get("dismiss_stale_reviews_on_push")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!(
                            "pull_request rule missing dismiss_stale_reviews_on_push"
                        ))?,
                    parameters
                        .get("require_code_owner_review")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!(
                            "pull_request rule missing require_code_owner_review"
                        ))?,
                    parameters
                        .get("require_last_push_approval")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!(
                            "pull_request rule missing require_last_push_approval"
                        ))?,
                    parameters
                        .get("required_review_thread_resolution")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!(
                            "pull_request rule missing required_review_thread_resolution"
                        ))?,
                    allowed_merge_methods
                        .into_iter()
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
            "required_status_checks" => {
                let parameters = rule
                    .get("parameters")
                    .ok_or_else(|| anyhow!("required_status_checks rule missing parameters"))?;
                let checks = parameters
                    .get("required_status_checks")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        anyhow!("required_status_checks rule missing required_status_checks")
                    })?;
                let mut contexts = BTreeSet::new();
                let mut integration_ids = BTreeMap::new();
                for entry in checks {
                    let context = entry
                        .get("context")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            anyhow!("required_status_checks rule entry missing context string")
                        })?;
                    contexts.insert(context.clone());
                    if let Some(integration_id) =
                        entry.get("integration_id").and_then(Value::as_u64)
                    {
                        integration_ids.insert(context, integration_id);
                    }
                }
                let integrations = integration_ids
                    .iter()
                    .map(|(context, integration_id)| format!("{context}:{integration_id}"))
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "required_status_checks|strict_required_status_checks_policy={}|contexts={}|integration_ids={}",
                    parameters
                        .get("strict_required_status_checks_policy")
                        .and_then(Value::as_bool)
                        .ok_or_else(|| anyhow!("required_status_checks rule missing strict_required_status_checks_policy"))?,
                    contexts.into_iter().collect::<Vec<_>>().join(","),
                    integrations
                )
            }
            other => return Err(anyhow!("unsupported effective rule type {}", other)),
        };
        signatures.insert(signature);
    }

    Ok(signatures)
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

fn validate_repo_path_exists(repo_root: &Path, path: &str) -> Result<()> {
    let normalized = normalize_relative(path)?;
    ensure!(
        repo_root.join(&normalized).exists(),
        "repo path does not exist: {}",
        normalized
    );
    Ok(())
}

fn validate_safe_list_condition(
    entry: &SafeListEntry,
    normalized_path: &str,
    used_upstream_prefixes: &BTreeSet<String>,
) -> Result<()> {
    match entry.condition.kind {
        SafeListConditionKind::UpstreamPathKind => {
            ensure!(
                matches!(
                    entry.condition.value.as_str(),
                    "docs" | "examples" | "tests" | "unused-adapter"
                ),
                "safe-list entry {} condition.value must be one of docs, examples, tests, unused-adapter for kind upstream-path-kind",
                entry.path
            );
            let path_matches = match entry.condition.value.as_str() {
                "docs" => normalized_path == "docs" || normalized_path.starts_with("docs/"),
                "examples" => {
                    normalized_path == "examples" || normalized_path.starts_with("examples/")
                }
                "tests" => {
                    normalized_path == "tests"
                        || normalized_path.starts_with("tests/")
                        || normalized_path.contains("/tests/")
                }
                "unused-adapter" => {
                    (normalized_path == "crates/adapters"
                        || normalized_path.starts_with("crates/adapters/"))
                        && !used_upstream_prefixes
                            .iter()
                            .any(|prefix| path_overlaps_prefix(normalized_path, prefix))
                }
                _ => false,
            };
            ensure!(
                path_matches,
                "safe-list entry {} condition upstream-path-kind={} does not match path semantics",
                entry.path,
                entry.condition.value
            );
        }
    }

    Ok(())
}

fn derive_protected_safe_list_roots(registry: &RegistryFile) -> BTreeSet<String> {
    let mut roots = SHARED_NT_CRATE_PREFIXES
        .iter()
        .map(|prefix| (*prefix).to_string())
        .collect::<BTreeSet<_>>();
    roots.extend(
        registry
            .seams
            .iter()
            .flat_map(|seam| seam.upstream_prefixes.iter())
            .filter_map(|prefix| protected_safe_list_root(prefix)),
    );
    roots
}

fn protected_safe_list_root(prefix: &str) -> Option<String> {
    let normalized = normalize_relative(prefix).ok()?;
    let segments = normalized.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        ["crates", "adapters", adapter, ..] => Some(format!("crates/adapters/{adapter}/")),
        ["crates", crate_name, ..] => Some(format!("crates/{crate_name}/")),
        _ => None,
    }
}

fn path_overlaps_prefix(path: &str, prefix: &str) -> bool {
    path.starts_with(prefix)
        || path == prefix.trim_end_matches('/')
        || prefix.starts_with(&(path.to_string() + "/"))
}

fn git_changed_files(repo_root: &Path, base_ref: &str, head_ref: &str) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["diff", "--name-only", &format!("{base_ref}...{head_ref}")])
        .current_dir(repo_root)
        .output()
        .context("failed to run git diff --name-only")?;

    ensure!(
        output.status.success(),
        "git diff --name-only failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_owned)
        .collect())
}

fn git_show_toml_or_empty(repo_root: &Path, git_ref: &str, path: &str) -> Result<TomlValue> {
    let text = git_show_text_or_empty(repo_root, git_ref, path)?;
    if text.trim().is_empty() {
        return Ok(TomlValue::Table(Default::default()));
    }
    toml::from_str(&text).with_context(|| format!("failed to parse {} at {}", path, git_ref))
}

fn git_show_text_or_empty(repo_root: &Path, git_ref: &str, path: &str) -> Result<String> {
    let exists = std::process::Command::new("git")
        .args(["cat-file", "-e", &format!("{git_ref}:{path}")])
        .current_dir(repo_root)
        .status()
        .context("failed to run git cat-file")?;
    if !exists.success() {
        return Ok(String::new());
    }

    let output = std::process::Command::new("git")
        .args(["show", &format!("{git_ref}:{path}")])
        .current_dir(repo_root)
        .output()
        .with_context(|| format!("failed to read {} at {}", path, git_ref))?;

    ensure!(
        output.status.success(),
        "git show {}:{} failed: {}",
        git_ref,
        path,
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .with_context(|| format!("{} at {} is not valid UTF-8", path, git_ref))
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

fn is_nt_guarded_surface(path: &str) -> bool {
    path == "Cargo.toml"
        || path == "Cargo.lock"
        || path.ends_with("/Cargo.toml")
        || path.ends_with("/Cargo.lock")
        || is_cargo_config_path(path)
}

fn is_cargo_config_path(path: &str) -> bool {
    path == ".cargo/config.toml"
        || path == ".cargo/config"
        || path.starts_with(".cargo/config.d/")
        || path.ends_with("/.cargo/config.toml")
        || path.ends_with("/.cargo/config")
        || path.contains("/.cargo/config.d/")
}

fn extract_guarded_cargo_config_state(config: &TomlValue) -> BTreeSet<String> {
    let mut state = BTreeSet::new();
    for key in ["patch", "replace", "source", "paths"] {
        if let Some(value) = config.get(key) {
            state.insert(format!("{}={}", key, canonical_toml_value(value)));
        }
    }
    state
}

fn extract_nt_lock_records(
    lockfile_contents: &str,
    configured: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    if lockfile_contents.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    let lockfile: TomlValue =
        toml::from_str(lockfile_contents).context("failed to parse Cargo.lock")?;
    let mut records = BTreeSet::new();
    let Some(packages) = lockfile.get("package").and_then(TomlValue::as_array) else {
        return Ok(records);
    };

    for package in packages {
        let Some(name) = package.get("name").and_then(TomlValue::as_str) else {
            continue;
        };
        let source = package
            .get("source")
            .and_then(TomlValue::as_str)
            .unwrap_or("");
        if configured.contains(name)
            || name.starts_with("nautilus-")
            || source.contains("nautilus_trader.git")
        {
            records.insert(canonical_toml_value(package));
        }
    }

    Ok(records)
}

fn extract_nt_crates_from_cargo_toml(
    cargo_toml: &TomlValue,
    configured: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut crates = BTreeSet::new();
    let Some(table) = cargo_toml.as_table() else {
        return crates;
    };
    collect_nt_data_from_toml_table(table, &mut Vec::new(), configured, Some(&mut crates), None);
    crates
}

fn extract_nt_dependency_records_from_cargo_toml(
    cargo_toml: &TomlValue,
    configured: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut records = BTreeSet::new();
    let Some(table) = cargo_toml.as_table() else {
        return records;
    };
    collect_nt_data_from_toml_table(table, &mut Vec::new(), configured, None, Some(&mut records));
    records
}

fn extract_nt_ignores_from_dependabot_cargo_blocks(
    contents: &str,
) -> Result<Vec<(String, BTreeSet<String>)>> {
    #[derive(Deserialize)]
    struct DependabotConfig {
        #[serde(default)]
        updates: Vec<DependabotUpdate>,
    }

    #[derive(Deserialize)]
    struct DependabotUpdate {
        #[serde(rename = "package-ecosystem")]
        package_ecosystem: String,
        directory: String,
        #[serde(default)]
        ignore: Vec<DependabotIgnore>,
    }

    #[derive(Deserialize)]
    struct DependabotIgnore {
        #[serde(rename = "dependency-name")]
        dependency_name: String,
    }

    let parsed: DependabotConfig =
        serde_yaml::from_str(contents).context("failed to parse .github/dependabot.yml")?;
    Ok(parsed
        .updates
        .into_iter()
        .filter(|update| update.package_ecosystem == "cargo")
        .map(|update| {
            (
                update.directory,
                update
                    .ignore
                    .into_iter()
                    .filter_map(|ignore| {
                        ignore
                            .dependency_name
                            .starts_with("nautilus-")
                            .then_some(ignore.dependency_name)
                    })
                    .collect(),
            )
        })
        .collect())
}

fn regex_escape_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn canonical_toml_value(value: &TomlValue) -> String {
    match value {
        TomlValue::String(inner) => format!("\"{}\"", inner),
        TomlValue::Integer(inner) => inner.to_string(),
        TomlValue::Float(inner) => inner.to_string(),
        TomlValue::Boolean(inner) => inner.to_string(),
        TomlValue::Datetime(inner) => inner.to_string(),
        TomlValue::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_toml_value)
                .collect::<Vec<_>>()
                .join(",")
        ),
        TomlValue::Table(table) => {
            let mut entries = table.iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!("{}={}", key, canonical_toml_value(value)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
fn extract_just_recipe_body(contents: &str, recipe_name: &str) -> Result<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    let mut header = None;
    let mut start = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(rest) = line.strip_prefix(recipe_name)
            && (rest.starts_with(':') || rest.starts_with(' '))
            && rest.contains(':')
        {
            header = Some(*line);
            start = Some(index + 1);
            break;
        }
        if line.starts_with(&format!("{}:", recipe_name)) {
            header = Some(*line);
            start = Some(index + 1);
            break;
        }
    }
    let Some(start) = start else {
        return Err(anyhow!("just recipe {} not found", recipe_name));
    };
    let header = header.expect("recipe header should exist when start exists");

    let mut body = vec![header];
    for line in &lines[start..] {
        if line.starts_with("    ") || line.starts_with('\t') || line.is_empty() {
            body.push(*line);
        } else {
            break;
        }
    }
    let normalized = body.join("\n").trim_end().to_string();
    Ok(format!("{}\n", normalized))
}

fn workflow_job_hash(contents: &str, job_name: &str) -> Result<Option<String>> {
    let value: YamlValue =
        serde_yaml::from_str(contents).context("failed to parse workflow YAML")?;
    let Some(jobs) = value.get("jobs").and_then(YamlValue::as_mapping) else {
        return Ok(None);
    };

    let Some(job) = jobs.get(YamlValue::String(job_name.to_string())) else {
        return Ok(None);
    };
    let normalized = canonical_yaml_value(job)?;
    Ok(Some(sha256_hex(normalized.as_bytes())))
}

fn workflow_job_matches_hash(contents: &str, job_name: &str, expected_hash: &str) -> Result<bool> {
    Ok(workflow_job_hash(contents, job_name)?.is_some_and(|actual| actual == expected_hash))
}

fn canonical_yaml_value(value: &YamlValue) -> Result<String> {
    match value {
        YamlValue::Null => Ok("null".to_string()),
        YamlValue::Bool(inner) => Ok(inner.to_string()),
        YamlValue::Number(inner) => Ok(inner.to_string()),
        YamlValue::String(inner) => {
            serde_json::to_string(inner).context("failed to canonicalize YAML string")
        }
        YamlValue::Sequence(items) => Ok(format!(
            "[{}]",
            items
                .iter()
                .map(canonical_yaml_value)
                .collect::<Result<Vec<_>>>()?
                .join(",")
        )),
        YamlValue::Mapping(map) => {
            let mut entries = map
                .iter()
                .map(|(key, value)| {
                    let key = key
                        .as_str()
                        .ok_or_else(|| anyhow!("workflow contract keys must be strings"))?;
                    Ok((key.to_string(), canonical_yaml_value(value)?))
                })
                .collect::<Result<Vec<_>>>()?;
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Ok(format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(&key).expect("string keys serialize"),
                        value
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ))
        }
        YamlValue::Tagged(tagged) => canonical_yaml_value(&tagged.value),
    }
}

fn collect_nt_data_from_toml_table(
    table: &toml::map::Map<String, TomlValue>,
    path: &mut Vec<String>,
    configured: &BTreeSet<String>,
    mut crates: Option<&mut BTreeSet<String>>,
    mut records: Option<&mut BTreeSet<String>>,
) {
    for (key, value) in table {
        path.push(key.clone());

        if path.last().is_some_and(|segment| {
            matches!(
                segment.as_str(),
                "dependencies" | "dev-dependencies" | "build-dependencies"
            )
        }) {
            if let Some(dep_table) = value.as_table() {
                for (name, dep_value) in dep_table {
                    if dependency_entry_is_nt(name, dep_value, configured) {
                        let canonical_name = canonical_dependency_name(name).to_string();
                        if let Some(crates) = crates.as_deref_mut() {
                            crates.insert(canonical_name);
                        }
                        if let Some(records) = records.as_deref_mut() {
                            records.insert(format!(
                                "{}::{}={}",
                                path.join("."),
                                name,
                                canonical_toml_value(dep_value)
                            ));
                        }
                    }
                }
            }
            path.pop();
            continue;
        }

        if path.len() == 2 && path[0] == "patch" {
            if let Some(dep_table) = value.as_table() {
                for (name, dep_value) in dep_table {
                    if dependency_entry_is_nt(name, dep_value, configured) {
                        let canonical_name = canonical_dependency_name(name).to_string();
                        if let Some(crates) = crates.as_deref_mut() {
                            crates.insert(canonical_name);
                        }
                        if let Some(records) = records.as_deref_mut() {
                            records.insert(format!(
                                "{}::{}={}",
                                path.join("."),
                                name,
                                canonical_toml_value(dep_value)
                            ));
                        }
                    }
                }
            }
            path.pop();
            continue;
        }

        if path.len() == 1 && path[0] == "replace" {
            if let Some(dep_table) = value.as_table() {
                for (name, dep_value) in dep_table {
                    if dependency_entry_is_nt(name, dep_value, configured) {
                        let canonical_name = canonical_dependency_name(name).to_string();
                        if let Some(crates) = crates.as_deref_mut() {
                            crates.insert(canonical_name);
                        }
                        if let Some(records) = records.as_deref_mut() {
                            records.insert(format!(
                                "{}::{}={}",
                                path.join("."),
                                name,
                                canonical_toml_value(dep_value)
                            ));
                        }
                    }
                }
            }
            path.pop();
            continue;
        }

        if let Some(subtable) = value.as_table() {
            collect_nt_data_from_toml_table(
                subtable,
                path,
                configured,
                crates.as_deref_mut(),
                records.as_deref_mut(),
            );
        }

        path.pop();
    }
}

fn canonical_dependency_name(name: &str) -> &str {
    name.split(':').next().unwrap_or(name)
}

fn dependency_entry_is_nt(name: &str, value: &TomlValue, configured: &BTreeSet<String>) -> bool {
    let canonical_name = canonical_dependency_name(name);
    if canonical_name.starts_with("nautilus-") || configured.contains(canonical_name) {
        return true;
    }

    value
        .get("git")
        .and_then(TomlValue::as_str)
        .is_some_and(|git| git.contains("nautilus_trader.git"))
}

fn expected_effective_rule_signature(rule: &ExpectedEffectiveRule) -> String {
    match rule {
        ExpectedEffectiveRule::Deletion => "deletion".to_string(),
        ExpectedEffectiveRule::NonFastForward => "non_fast_forward".to_string(),
        ExpectedEffectiveRule::PullRequest {
            required_approving_review_count,
            dismiss_stale_reviews_on_push,
            require_code_owner_review,
            require_last_push_approval,
            required_review_thread_resolution,
            allowed_merge_methods,
            ..
        } => format!(
            "pull_request|required_approving_review_count={}|dismiss_stale_reviews_on_push={}|require_code_owner_review={}|require_last_push_approval={}|required_review_thread_resolution={}|allowed_merge_methods={}",
            required_approving_review_count,
            dismiss_stale_reviews_on_push,
            require_code_owner_review,
            require_last_push_approval,
            required_review_thread_resolution,
            allowed_merge_methods
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",")
        ),
        ExpectedEffectiveRule::RequiredStatusChecks {
            strict_required_status_checks_policy,
            required_status_checks,
            required_status_check_integration_ids,
        } => {
            let contexts = required_status_checks
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(",");
            let integrations = required_status_check_integration_ids
                .iter()
                .map(|(context, integration_id)| format!("{context}:{integration_id}"))
                .collect::<Vec<_>>()
                .join(",");
            format!(
                "required_status_checks|strict_required_status_checks_policy={}|contexts={}|integration_ids={}",
                strict_required_status_checks_policy, contexts, integrations
            )
        }
    }
}

fn expected_ruleset_signature(ruleset: &ExpectedRuleset) -> String {
    format!(
        "ruleset|id={}|name={}|enforcement={}|bypass_actors={}",
        ruleset.id,
        ruleset.name,
        ruleset.enforcement,
        ruleset
            .allowed_bypass_actors
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",")
    )
}

#[cfg(test)]
mod unit_tests {
    use super::{
        ControlConfig, canonical_yaml_value, extract_guarded_cargo_config_state,
        extract_just_recipe_body, extract_nt_crates_from_cargo_toml,
        extract_nt_ignores_from_dependabot_cargo_blocks, load_toml, sha256_hex, workflow_job_hash,
        workflow_job_matches_hash,
    };
    use serde_yaml::Value as YamlValue;
    use std::{collections::BTreeSet, fs, path::PathBuf};

    #[test]
    fn cargo_nt_extractor_scans_workspace_target_build_and_patch_sections() {
        let cargo_toml = toml::from_str::<toml::Value>(
            r#"
[package]
name = "fixture"
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
nautilus-common = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "abc" }

[build-dependencies]
nautilus-trading = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "def" }

[target.'cfg(unix)'.dependencies]
nautilus-core = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "ghi" }

[patch."https://github.com/nautechsystems/nautilus_trader.git"]
nautilus-execution = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "jkl" }
"#,
        )
        .expect("fixture Cargo.toml should parse");

        let configured = BTreeSet::from([
            "nautilus-common".to_string(),
            "nautilus-core".to_string(),
            "nautilus-trading".to_string(),
            "nautilus-execution".to_string(),
        ]);

        let extracted = extract_nt_crates_from_cargo_toml(&cargo_toml, &configured);

        assert_eq!(extracted, configured);
    }

    #[test]
    fn cargo_nt_extractor_scans_replace_section() {
        let cargo_toml = toml::from_str::<toml::Value>(
            r#"
[package]
name = "fixture"
version = "0.1.0"
edition = "2024"

[replace]
"nautilus-common:0.1.0" = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "abc" }
"#,
        )
        .expect("fixture Cargo.toml should parse");

        let configured = BTreeSet::from(["nautilus-common".to_string()]);

        let extracted = extract_nt_crates_from_cargo_toml(&cargo_toml, &configured);

        assert_eq!(extracted, configured);
    }

    #[test]
    fn cargo_config_path_detection_includes_config_d_entries() {
        assert!(super::is_cargo_config_path(".cargo/config.toml"));
        assert!(super::is_cargo_config_path(".cargo/config"));
        assert!(super::is_cargo_config_path(".cargo/config.d/override.toml"));
        assert!(super::is_cargo_config_path(
            "crates/model/.cargo/config.toml"
        ));
        assert!(super::is_cargo_config_path("crates/model/.cargo/config"));
        assert!(super::is_cargo_config_path(
            "crates/model/.cargo/config.d/override.toml"
        ));
        assert!(!super::is_cargo_config_path(".cargo/not-config.toml"));
    }

    #[test]
    fn cargo_config_state_includes_replace_entries() {
        let config = toml::from_str::<toml::Value>(
            r#"
[replace]
"nautilus-common:0.1.0" = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "abc" }
"#,
        )
        .expect("fixture config should parse");

        let state = extract_guarded_cargo_config_state(&config);

        assert!(
            state.iter().any(|entry| entry.starts_with("replace=")),
            "replace entries should be guarded"
        );
    }

    #[test]
    fn dependabot_nt_extractor_collects_all_cargo_blocks() {
        let contents = r#"
version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    ignore:
      - dependency-name: "nautilus-common"
      - dependency-name: "nautilus-core"
  - package-ecosystem: "cargo"
    directory: "/crates/core"
    ignore:
      - dependency-name: "nautilus-common"
      - dependency-name: "nautilus-core"
  - package-ecosystem: "github-actions"
    directory: "/"
    ignore:
      - dependency-name: "nautilus-fake"
"#;

        let extracted = extract_nt_ignores_from_dependabot_cargo_blocks(contents)
            .expect("dependabot fixture should parse");

        assert_eq!(
            extracted,
            vec![
                (
                    "/".to_string(),
                    BTreeSet::from(["nautilus-common".to_string(), "nautilus-core".to_string()])
                ),
                (
                    "/crates/core".to_string(),
                    BTreeSet::from(["nautilus-common".to_string(), "nautilus-core".to_string()])
                )
            ]
        );
    }

    #[test]
    fn dependabot_nt_extractor_reports_yaml_errors() {
        let err = extract_nt_ignores_from_dependabot_cargo_blocks("not: [valid")
            .expect_err("malformed YAML should fail");
        assert!(
            err.to_string()
                .contains("failed to parse .github/dependabot.yml")
        );
    }

    #[test]
    fn workflow_guard_detection_requires_exact_job_contract() {
        let baseline = r#"
name: Example
jobs:
  control_plane:
    name: nt-pointer-control-plane
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@pinned
      - name: Block direct NT pin changes
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#;
        let gated = r#"
name: Example
jobs:
  control_plane:
    name: nt-pointer-control-plane
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@pinned
      - name: Block direct NT pin changes
        if: github.event_name == 'pull_request' && false
        shell: bash
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#;
        let prep_step = r#"
name: Example
jobs:
  control_plane:
    name: nt-pointer-control-plane
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@pinned
      - name: Workspace setup
        shell: bash
        run: |
          exit 0
      - name: Block direct NT pin changes
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#;

        let value: YamlValue =
            serde_yaml::from_str(baseline).expect("baseline workflow should parse");
        let jobs = value
            .get("jobs")
            .and_then(YamlValue::as_mapping)
            .expect("workflow should include jobs");
        let job = jobs
            .get(YamlValue::String("control_plane".to_string()))
            .expect("workflow should include control_plane");
        let expected_hash = sha256_hex(
            canonical_yaml_value(job)
                .expect("job should canonicalize")
                .as_bytes(),
        );

        assert!(
            workflow_job_matches_hash(baseline, "control_plane", &expected_hash)
                .expect("baseline workflow should parse"),
            "baseline workflow should match its contract"
        );
        assert!(
            !workflow_job_matches_hash(gated, "control_plane", &expected_hash)
                .expect("gated workflow should parse"),
            "gating fields should drift the job contract"
        );
        assert!(
            !workflow_job_matches_hash(prep_step, "control_plane", &expected_hash)
                .expect("prep-step workflow should parse"),
            "extra prep steps should drift the job contract"
        );
    }

    #[test]
    fn just_recipe_extraction_hashes_normalized_body() {
        let justfile = r#"
recipe-name arg:
    echo hello
    echo world

next:
    echo next
"#;

        let body =
            extract_just_recipe_body(justfile, "recipe-name").expect("recipe should extract");
        assert_eq!(body, "recipe-name arg:\n    echo hello\n    echo world\n");
        assert_eq!(
            sha256_hex(body.as_bytes()),
            "a2630f081b7424aec0b869d604e864506faaede33f34a5161a3ba681aa52893b"
        );
    }

    #[test]
    fn repo_guard_contract_hashes_match_tracked_files() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let control: ControlConfig =
            load_toml(&repo_root.join("config/nt_pointer_probe/control.toml"))
                .expect("control.toml should parse");

        for (path, expected_hash) in [
            (
                control.guard_contract.script_path.as_str(),
                control.guard_contract.script_sha256.as_str(),
            ),
            (
                control.guard_contract.justfile_path.as_str(),
                control.guard_contract.justfile_sha256.as_str(),
            ),
            (
                control.guard_contract.owner_require_script_path.as_str(),
                control.guard_contract.owner_require_script_sha256.as_str(),
            ),
            (
                control.guard_contract.owner_install_script_path.as_str(),
                control.guard_contract.owner_install_script_sha256.as_str(),
            ),
            (
                control
                    .guard_contract
                    .setup_environment_action_path
                    .as_str(),
                control
                    .guard_contract
                    .setup_environment_action_sha256
                    .as_str(),
            ),
        ] {
            let actual_hash = sha256_hex(
                &fs::read(repo_root.join(path)).expect("guard contract file should read"),
            );
            assert_eq!(
                actual_hash, expected_hash,
                "guard contract hash drift for {}",
                path
            );
        }

        for (workflow_path, job_name, expected_hash) in [
            (
                control.guard_contract.control_plane_workflow.as_str(),
                control.guard_contract.control_plane_job.as_str(),
                control.guard_contract.control_plane_job_sha256.as_str(),
            ),
            (
                control.guard_contract.self_test_workflow.as_str(),
                control.guard_contract.self_test_job.as_str(),
                control.guard_contract.self_test_job_sha256.as_str(),
            ),
            (
                control.guard_contract.dependabot_workflow.as_str(),
                control.guard_contract.dependabot_job.as_str(),
                control.guard_contract.dependabot_job_sha256.as_str(),
            ),
            (
                control.guard_contract.drift_workflow.as_str(),
                control.guard_contract.drift_job.as_str(),
                control.guard_contract.drift_job_sha256.as_str(),
            ),
        ] {
            let contents =
                fs::read_to_string(repo_root.join(workflow_path)).expect("workflow should read");
            let actual_hash = workflow_job_hash(&contents, job_name)
                .expect("workflow should parse")
                .expect("workflow should contain guarded job");
            assert_eq!(
                actual_hash, expected_hash,
                "guard contract hash drift for {}",
                workflow_path
            );
        }
    }
}
