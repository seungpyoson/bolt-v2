use std::{fs, path::Path, path::PathBuf};

use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_governance_responses,
    compare_branch_protection_response,
};
use serde_yaml::Value as YamlValue;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::symlink;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn fixture(path: &str) -> PathBuf {
    repo_root()
        .join("tests/fixtures/nt_pointer_probe")
        .join(path)
}

fn copy_fixture_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("fixture destination should be creatable");
    for entry in fs::read_dir(source).expect("fixture source should exist") {
        let entry = entry.expect("fixture entry should read");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_fixture_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should copy");
        }
    }
}

#[cfg(unix)]
fn link_repo_entry(source: &Path, destination: &Path) {
    symlink(source, destination).expect("repo entry should symlink");
}

#[cfg(not(unix))]
fn link_repo_entry(source: &Path, destination: &Path) {
    if source.is_dir() {
        copy_fixture_tree(source, destination);
    } else {
        fs::copy(source, destination).expect("repo entry should copy");
    }
}

fn temp_fixture(name: &str) -> TempDir {
    let tempdir = tempfile::tempdir().expect("tempdir should create");
    copy_fixture_tree(&fixture(name), tempdir.path());

    for relative in [
        "Cargo.toml",
        "justfile",
        "src",
        "scripts/nt_pin_block_guard.sh",
        "scripts/require_rust_verification_owner.sh",
        "scripts/install_ci_rust_verification_owner.sh",
        "tests/reference_actor.rs",
        "tests/reference_pipeline.rs",
        ".github/dependabot.yml",
        ".github/actions/setup-environment/action.yml",
        ".github/workflows/nt-pointer-control-plane.yml",
        ".github/workflows/nt-pointer-probe-self-test.yml",
        ".github/workflows/nt-pointer-branch-governance-drift.yml",
        ".github/workflows/dependabot-auto-merge.yml",
    ] {
        let source = repo_root().join(relative);
        let destination = tempdir.path().join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).expect("parent dir should exist");
        }
        link_repo_entry(&source, &destination);
    }

    tempdir
}

fn init_temp_git_repo() -> TempDir {
    let tempdir = tempfile::tempdir().expect("temp git repo should create");
    copy_fixture_tree(&fixture("valid_minimal"), tempdir.path());

    fs::create_dir_all(tempdir.path().join("src/platform")).expect("src/platform should create");
    fs::create_dir_all(tempdir.path().join("src/clients")).expect("src/clients should create");
    fs::create_dir_all(tempdir.path().join("tests")).expect("tests dir should create");
    fs::create_dir_all(tempdir.path().join(".github/actions/setup-environment"))
        .expect("setup-environment dir should create");
    fs::create_dir_all(tempdir.path().join(".github/workflows"))
        .expect("workflow dir should create");
    fs::create_dir_all(tempdir.path().join("scripts")).expect("scripts dir should create");

    fs::write(
        tempdir.path().join("config/nt_pointer_probe/control.toml"),
        r#"schema_version = 1
repo = "seungpyoson/bolt-v2"
default_branch = "main"
artifact_store_uri = "s3://bolt-deploy-artifacts/artifacts/bolt-v2/nt-pointer-probe"
artifact_retention_days = 90
max_safe_list_duration_days = 30
tag_soak_days = 7
nt_crates = ["nautilus-common"]

[paths]
registry = "config/nt_pointer_probe/registry.toml"
safe_list = "config/nt_pointer_probe/safe_list.toml"
replay_set = "config/nt_pointer_probe/replay_set.toml"
expected_branch_protection = "config/nt_pointer_probe/expected_branch_protection.toml"
advisory_issue_template = ".github/nt-pointer-probe/advisory_issue.md"
draft_pr_template = ".github/nt-pointer-probe/draft_pr.md"

[status_checks]
control_plane = "nt-pointer-control-plane"
self_test = "nt-pointer-probe-self-test"
develop = "nt-pointer-probe-develop"
tagged = "nt-pointer-probe-tagged"
external_review = "external-adversarial-review"

[develop_lane]
issue_label = "nt-pointer-probe-advisory"
issue_title_prefix = "NT Pointer Probe Advisory"

[drift_lane]
issue_label = "nt-pointer-probe-drift"
issue_title_prefix = "NT Pointer Probe Drift"

[tagged_lane]
pr_branch = "automation/nt-pointer-probe"
pr_title_prefix = "NT Pointer Probe"

[guard_contract]
script_path = "scripts/nt_pin_block_guard.sh"
script_sha256 = "fa795594d4368b32b364a2601cb2701ff04ae4dd1696eb85c708b150e594e16f"
justfile_path = "justfile"
justfile_sha256 = "658f0704bc279d0e702c68ae4fd9b540bbdfa37bc99121e51e3d1bd6d69ca51f"
owner_require_script_path = "scripts/require_rust_verification_owner.sh"
owner_require_script_sha256 = "629dc8f068538400b445f19946e10ddaf0a6550e92c9da7c13c3ad51d0fd7e31"
owner_install_script_path = "scripts/install_ci_rust_verification_owner.sh"
owner_install_script_sha256 = "98d2c3f1d3c0eaf1ffcc1b5ae5c1f0f30f89b48ae33c2f308f47d33e68d13a4d"
setup_environment_action_path = ".github/actions/setup-environment/action.yml"
setup_environment_action_sha256 = "e5db83cc2ea93cb2c49fa86c64c1089c56da1005db82845b02efa28569039834"
control_plane_workflow = ".github/workflows/nt-pointer-control-plane.yml"
control_plane_job = "control_plane"
control_plane_job_sha256 = "1a398426e4936b834db5109135ac547b1bbcc2a40d36656d2d590853ba4b4aec"
self_test_workflow = ".github/workflows/nt-pointer-probe-self-test.yml"
self_test_job = "self_test"
self_test_job_sha256 = "c5769a64de8eaed10ccf7749ea5d7b28121c3a02bda731736c82edd6a3e311cd"
dependabot_workflow = ".github/workflows/dependabot-auto-merge.yml"
dependabot_job = "dependabot"
dependabot_job_sha256 = "a03cf579aee5e6eab93934a7f2b65fb942afeb90be82b29eb69d8abb982f24c5"
drift_workflow = ".github/workflows/nt-pointer-branch-governance-drift.yml"
drift_job = "branch_protection_drift"
drift_job_sha256 = "6e5da4b1c0849a40048a80371849978f75515bf72bb6f24a5d98608a80501d7f"
"#,
    )
    .expect("fixture control.toml should write");
    fs::write(
        tempdir.path().join("Cargo.toml"),
        r#"[package]
name = "fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
nautilus-common = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "af2aefc24451ed5c51b94e64459421f1dd540bfb" }
"#,
    )
    .expect("fixture Cargo.toml should write");
    fs::write(
        tempdir.path().join(".github/dependabot.yml"),
        r#"version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    ignore:
      - dependency-name: "nautilus-common"
"#,
    )
    .expect("fixture dependabot should write");
    fs::copy(
        repo_root().join(".github/workflows/nt-pointer-control-plane.yml"),
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-control-plane.yml"),
    )
    .expect("fixture control-plane workflow should copy");
    fs::copy(
        repo_root().join(".github/workflows/nt-pointer-probe-self-test.yml"),
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-probe-self-test.yml"),
    )
    .expect("fixture self-test workflow should copy");
    fs::copy(
        repo_root()
            .join(".github/workflows/nt-pointer-branch-governance-drift.yml"),
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-branch-governance-drift.yml"),
    )
    .expect("fixture drift workflow should copy");
    fs::copy(
        repo_root().join(".github/workflows/dependabot-auto-merge.yml"),
        tempdir
            .path()
            .join(".github/workflows/dependabot-auto-merge.yml"),
    )
    .expect("fixture dependabot workflow should copy");
    fs::write(
        tempdir.path().join("src/platform/reference_actor.rs"),
        "// fixture\n",
    )
    .expect("fixture reference_actor should write");
    fs::write(
        tempdir.path().join("src/clients/chainlink.rs"),
        "// fixture\n",
    )
    .expect("fixture chainlink should write");
    fs::write(
        tempdir.path().join("tests/reference_actor.rs"),
        "// fixture\n",
    )
    .expect("fixture reference actor test should write");
    fs::write(
        tempdir.path().join("tests/reference_pipeline.rs"),
        "// fixture\n",
    )
    .expect("fixture reference pipeline test should write");
    fs::copy(
        repo_root().join("scripts/nt_pin_block_guard.sh"),
        tempdir.path().join("scripts/nt_pin_block_guard.sh"),
    )
    .expect("fixture guard script should copy");
    fs::copy(
        repo_root().join("scripts/require_rust_verification_owner.sh"),
        tempdir
            .path()
            .join("scripts/require_rust_verification_owner.sh"),
    )
    .expect("fixture owner require script should copy");
    fs::copy(
        repo_root().join("scripts/install_ci_rust_verification_owner.sh"),
        tempdir
            .path()
            .join("scripts/install_ci_rust_verification_owner.sh"),
    )
    .expect("fixture owner install script should copy");
    fs::copy(
        repo_root().join("justfile"),
        tempdir.path().join("justfile"),
    )
    .expect("fixture justfile should copy");
    fs::copy(
        repo_root().join(".github/actions/setup-environment/action.yml"),
        tempdir
            .path()
            .join(".github/actions/setup-environment/action.yml"),
    )
    .expect("fixture setup action should copy");

    for command in [
        ["init"].as_slice(),
        ["config", "user.name", "fixture"].as_slice(),
        ["config", "user.email", "fixture@example.com"].as_slice(),
        ["add", "."].as_slice(),
        ["commit", "--no-verify", "-m", "base"].as_slice(),
    ] {
        let status = std::process::Command::new("git")
            .args(command)
            .current_dir(tempdir.path())
            .status()
            .expect("git command should run");
        assert!(
            status.success(),
            "git command should succeed: {:?}",
            command
        );
    }

    fs::write(
        tempdir.path().join("Cargo.toml"),
        r#"[package]
name = "fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
nautilus-common = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "af2aefc24451ed5c51b94e64459421f1dd540bfb", features = ["extra-surface"] }
"#,
    )
    .expect("mutated Cargo.toml should write");

    let status = std::process::Command::new("git")
        .args(["add", "Cargo.toml"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--no-verify", "-m", "head"])
        .current_dir(tempdir.path())
        .status()
        .expect("git commit should run");
    assert!(status.success(), "git commit should succeed");

    tempdir
}

#[test]
fn repo_control_plane_loads_and_validates() {
    let loaded = LoadedControlPlane::load_from_repo_root(&repo_root())
        .expect("repo control plane should load and validate");

    assert_eq!(loaded.control.schema_version, 1);
    assert_eq!(loaded.control.default_branch, "main");
    assert!(
        loaded
            .registry
            .seams
            .iter()
            .any(|seam| seam.name == "subscription_custom_data_semantics"),
        "expected initial seam registry to include subscription semantics seam"
    );
}

#[test]
fn control_plane_workflow_is_pull_request_only() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/nt-pointer-control-plane.yml"))
            .expect("control-plane workflow should load");
    let yaml: YamlValue =
        serde_yaml::from_str(&workflow).expect("control-plane workflow should parse as YAML");

    let triggers = yaml
        .get(YamlValue::String("on".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("control-plane workflow should declare triggers");

    assert!(
        triggers.contains_key(YamlValue::String("pull_request".to_string())),
        "control-plane workflow must run on pull_request"
    );
    assert!(
        !triggers.contains_key(YamlValue::String("workflow_dispatch".to_string())),
        "control-plane workflow must not be manually dispatchable"
    );
    assert!(
        !triggers.contains_key(YamlValue::String("schedule".to_string())),
        "control-plane workflow must not be scheduled"
    );
}

#[test]
fn guard_contract_rejects_block_step_gating_fields() {
    let tempdir = temp_fixture("valid_minimal");
    let workflow_path = tempdir
        .path()
        .join(".github/workflows/nt-pointer-control-plane.yml");
    fs::remove_file(&workflow_path).expect("workflow symlink should be removable");
    fs::write(
        &workflow_path,
        r#"name: NT Pointer Control Plane
jobs:
  control_plane:
    name: nt-pointer-control-plane
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - name: Setup environment
        id: setup
        uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
      - name: Validate control-plane artifacts
        run: just nt-pointer-probe-validate-control-plane
      - name: Block direct NT pin changes
        if: github.event_name == 'pull_request' && false
        shell: bash
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#,
    )
    .expect("gated workflow fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("guard step gating fields should fail validation");

    assert!(
        err.to_string()
            .contains("must keep the exact guard job contract"),
        "unexpected error: {err}"
    );
}

#[test]
fn guard_contract_rejects_top_level_justfile_variable_drift() {
    let tempdir = temp_fixture("valid_minimal");
    let justfile_path = tempdir.path().join("justfile");
    fs::remove_file(&justfile_path).expect("justfile symlink should be removable");
    let contents = fs::read_to_string(repo_root().join("justfile")).expect("justfile should read");
    fs::write(
        &justfile_path,
        contents.replace(
            "rust_verification_owner := env_var('HOME') + \"/.claude/lib/rust_verification.py\"",
            "rust_verification_owner := \"./wrapper.py\"",
        ),
    )
    .expect("mutated justfile should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("top-level justfile variable drift should fail validation");

    assert!(
        err.to_string().contains("justfile hash drift"),
        "unexpected error: {err}"
    );
}

#[test]
fn guard_contract_rejects_setup_environment_action_drift() {
    let tempdir = temp_fixture("valid_minimal");
    let action_path = tempdir
        .path()
        .join(".github/actions/setup-environment/action.yml");
    fs::remove_file(&action_path).expect("setup action symlink should be removable");
    fs::write(
        &action_path,
        r#"name: Setup Environment
runs:
  using: composite
  steps:
    - name: noop
      shell: bash
      run: |
        exit 0
"#,
    )
    .expect("mutated setup action should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("setup action drift should fail validation");

    assert!(
        err.to_string()
            .contains("setup-environment action hash drift"),
        "unexpected error: {err}"
    );
}

#[test]
fn guard_contract_rejects_extra_prep_steps_before_block() {
    let tempdir = temp_fixture("valid_minimal");
    let workflow_path = tempdir
        .path()
        .join(".github/workflows/nt-pointer-control-plane.yml");
    fs::remove_file(&workflow_path).expect("workflow symlink should be removable");
    fs::write(
        &workflow_path,
        r#"name: NT Pointer Control Plane
jobs:
  control_plane:
    name: nt-pointer-control-plane
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd # v6.0.2
      - name: Setup environment
        id: setup
        uses: ./.github/actions/setup-environment
        with:
          claude-config-read-token: ${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}
          just-version: ${{ env.JUST_VERSION }}
          lint-workflow-contract: "true"
      - name: Validate control-plane artifacts
        run: just nt-pointer-probe-validate-control-plane
      - name: Workspace setup
        shell: bash
        run: |
          cat > scripts/nt_pin_block_guard.sh <<'EOF'
          #!/usr/bin/env bash
          exit 0
          EOF
      - name: Block direct NT pin changes
        if: github.event_name == 'pull_request'
        shell: bash
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#,
    )
    .expect("prep-step workflow fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("prep-step bypass should fail validation");

    assert!(
        err.to_string()
            .contains("must keep the exact guard job contract"),
        "unexpected error: {err}"
    );
}

#[test]
fn nt_mutation_checker_rejects_root_manifest_change() {
    let tempdir = init_temp_git_repo();
    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("temp git repo control plane should load");

    let err = loaded
        .ensure_no_nt_mutation_from_git_refs("HEAD~1", "HEAD")
        .expect_err("root manifest NT mutation should fail closed");

    assert!(
        err.to_string()
            .contains("Cargo.toml changed NT dependency records"),
        "unexpected error: {err}"
    );
}

#[test]
fn nt_mutation_checker_rejects_config_d_override_change() {
    let tempdir = init_temp_git_repo();
    fs::create_dir_all(tempdir.path().join(".cargo/config.d")).expect("config.d dir should create");
    fs::write(
        tempdir.path().join(".cargo/config.d/override.toml"),
        r#"[paths]
search = []
"#,
    )
    .expect("base config.d should write");

    let status = std::process::Command::new("git")
        .args(["add", ".cargo/config.d/override.toml"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--amend", "--no-verify", "--no-edit"])
        .current_dir(tempdir.path())
        .status()
        .expect("git amend should run");
    assert!(status.success(), "git amend should succeed");

    fs::write(
        tempdir.path().join(".cargo/config.d/override.toml"),
        r#"[source.nautilus]
git = "https://github.com/evil/nautilus_trader.git"
"#,
    )
    .expect("mutated config.d should write");
    let status = std::process::Command::new("git")
        .args(["add", ".cargo/config.d/override.toml"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--no-verify", "-m", "config override"])
        .current_dir(tempdir.path())
        .status()
        .expect("git commit should run");
    assert!(status.success(), "git commit should succeed");

    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("temp git repo control plane should load");

    let err = loaded
        .ensure_no_nt_mutation_from_git_refs("HEAD~1", "HEAD")
        .expect_err("config.d NT mutation should fail closed");

    assert!(
        err.to_string()
            .contains(".cargo/config.d/override.toml changed guarded cargo config state"),
        "unexpected error: {err}"
    );
}

#[test]
fn shared_crate_prefix_safe_list_fails_closed() {
    let tempdir = temp_fixture("bad_shared_crate_prefix");
    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("shared NT crate prefix safe-list should fail validation");

    assert!(
        err.to_string()
            .contains("shared NT crate safe-list entries must use exact match"),
        "unexpected error: {err}"
    );
}

#[test]
fn shared_crate_root_prefix_safe_list_fails_closed() {
    let tempdir = temp_fixture("bad_shared_crate_root_prefix");
    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("shared NT crate root safe-list should fail validation");

    assert!(
        err.to_string()
            .contains("shared NT crate safe-list entries must use exact match"),
        "unexpected error: {err}"
    );
}

#[test]
fn safe_list_parent_prefix_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "crates/"
match = "prefix"
non_overlap_proof = "Invalid fixture: parent of shared crate roots must not be safe-listed broadly."
approved_by = "fixture"
approved_at = "2099-01-01"
revalidate_after = "2099-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "shared-nt-crate-parent"
"#,
    )
    .expect("parent prefix fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("shared NT crate parent prefix should fail validation");

    assert!(
        err.to_string()
            .contains("shared NT crate safe-list entries must use exact match"),
        "unexpected error: {err}"
    );
}

#[test]
fn expired_safe_list_entry_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "docs/"
match = "prefix"
non_overlap_proof = "Expired fixture entry."
approved_by = "fixture"
approved_at = "2000-01-01"
revalidate_after = "2000-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "docs"
"#,
    )
    .expect("expired fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("expired safe-list entries should fail validation");

    assert!(
        err.to_string().contains("safe-list entry docs/ is expired"),
        "unexpected error: {err}"
    );
}

#[test]
fn absolute_control_plane_paths_fail_before_loading_artifacts() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir.path().join("config/nt_pointer_probe/control.toml"),
        r#"schema_version = 1
repo = "seungpyoson/bolt-v2"
default_branch = "main"
artifact_store_uri = "s3://bolt-deploy-artifacts/artifacts/bolt-v2/nt-pointer-probe"
artifact_retention_days = 90
max_safe_list_duration_days = 30
tag_soak_days = 7
nt_crates = [
  "nautilus-common",
  "nautilus-core",
  "nautilus-binance",
  "nautilus-bybit",
  "nautilus-deribit",
  "nautilus-hyperliquid",
  "nautilus-kraken",
  "nautilus-live",
  "nautilus-model",
  "nautilus-network",
  "nautilus-okx",
  "nautilus-persistence",
  "nautilus-polymarket",
  "nautilus-system",
  "nautilus-trading",
  "nautilus-execution",
]

[paths]
registry = "/etc/passwd"
safe_list = "config/nt_pointer_probe/safe_list.toml"
replay_set = "config/nt_pointer_probe/replay_set.toml"
expected_branch_protection = "config/nt_pointer_probe/expected_branch_protection.toml"
advisory_issue_template = ".github/nt-pointer-probe/advisory_issue.md"
draft_pr_template = ".github/nt-pointer-probe/draft_pr.md"

[status_checks]
control_plane = "nt-pointer-control-plane"
self_test = "nt-pointer-probe-self-test"
develop = "nt-pointer-probe-develop"
tagged = "nt-pointer-probe-tagged"
external_review = "external-adversarial-review"

[develop_lane]
issue_label = "nt-pointer-probe-advisory"
issue_title_prefix = "NT Pointer Probe Advisory"

[drift_lane]
issue_label = "nt-pointer-probe-drift"
issue_title_prefix = "NT Pointer Probe Drift"

[tagged_lane]
pr_branch = "automation/nt-pointer-probe"
pr_title_prefix = "NT Pointer Probe"

[guard_contract]
script_path = "scripts/nt_pin_block_guard.sh"
script_sha256 = "fa795594d4368b32b364a2601cb2701ff04ae4dd1696eb85c708b150e594e16f"
justfile_path = "justfile"
justfile_sha256 = "658f0704bc279d0e702c68ae4fd9b540bbdfa37bc99121e51e3d1bd6d69ca51f"
owner_require_script_path = "scripts/require_rust_verification_owner.sh"
owner_require_script_sha256 = "629dc8f068538400b445f19946e10ddaf0a6550e92c9da7c13c3ad51d0fd7e31"
owner_install_script_path = "scripts/install_ci_rust_verification_owner.sh"
owner_install_script_sha256 = "98d2c3f1d3c0eaf1ffcc1b5ae5c1f0f30f89b48ae33c2f308f47d33e68d13a4d"
setup_environment_action_path = ".github/actions/setup-environment/action.yml"
setup_environment_action_sha256 = "e5db83cc2ea93cb2c49fa86c64c1089c56da1005db82845b02efa28569039834"
control_plane_workflow = ".github/workflows/nt-pointer-control-plane.yml"
control_plane_job = "control_plane"
control_plane_job_sha256 = "1a398426e4936b834db5109135ac547b1bbcc2a40d36656d2d590853ba4b4aec"
self_test_workflow = ".github/workflows/nt-pointer-probe-self-test.yml"
self_test_job = "self_test"
self_test_job_sha256 = "c5769a64de8eaed10ccf7749ea5d7b28121c3a02bda731736c82edd6a3e311cd"
dependabot_workflow = ".github/workflows/dependabot-auto-merge.yml"
dependabot_job = "dependabot"
dependabot_job_sha256 = "a03cf579aee5e6eab93934a7f2b65fb942afeb90be82b29eb69d8abb982f24c5"
drift_workflow = ".github/workflows/nt-pointer-branch-governance-drift.yml"
drift_job = "branch_protection_drift"
drift_job_sha256 = "6e5da4b1c0849a40048a80371849978f75515bf72bb6f24a5d98608a80501d7f"
"#,
    )
    .expect("invalid control fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("absolute control artifact paths should fail validation");

    assert!(
        err.to_string()
            .contains("path must be repo-relative: /etc/passwd"),
        "unexpected error: {err}"
    );
}

#[test]
fn missing_bolt_usage_path_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir.path().join("config/nt_pointer_probe/registry.toml"),
        r#"schema_version = 1
coverage_classes = [
  "compile-time-api",
  "unit-behavior",
  "integration-behavior",
  "bootstrap-materialization",
  "serialization-contract",
  "network-transport",
  "timing-ordering",
]

[[seams]]
name = "subscription_custom_data_semantics"
risk = "Subscription ownership and custom-data delivery changes can break Bolt actor wiring."
bolt_usage = ["src/does_not_exist.rs"]
upstream_prefixes = [
  "crates/common/src",
  "crates/system/src",
  "crates/adapters/polymarket/src",
]
required_coverage = ["compile-time-api", "integration-behavior"]
escalation = "fail"

[[seams.canaries]]
id = "tests/reference_actor.rs::reference_actor_subscribes_to_quotes_for_configured_venues"
path = "tests/reference_actor.rs"
coverage = "integration-behavior"
assertion = "Reference actor still subscribes through the expected NT client seams."

[[seams.canaries]]
id = "tests/reference_pipeline.rs::builds_shared_chainlink_reference_data_client_for_all_configured_chainlink_venues"
path = "tests/reference_pipeline.rs"
coverage = "compile-time-api"
assertion = "Reference pipeline still compiles and wires shared custom-data semantics."
"#,
    )
    .expect("invalid registry fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("missing bolt usage path should fail validation");

    assert!(
        err.to_string()
            .contains("repo path does not exist: src/does_not_exist.rs"),
        "unexpected error: {err}"
    );
}

#[test]
fn second_dependabot_cargo_block_without_nt_ignores_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    let dependabot_path = tempdir.path().join(".github/dependabot.yml");
    fs::remove_file(&dependabot_path).expect("dependabot symlink should be removable");
    fs::write(
        &dependabot_path,
        r#"version: 2
updates:
  - package-ecosystem: "cargo"
    directory: "/"
    ignore:
      - dependency-name: "nautilus-common"
      - dependency-name: "nautilus-core"
      - dependency-name: "nautilus-binance"
      - dependency-name: "nautilus-bybit"
      - dependency-name: "nautilus-deribit"
      - dependency-name: "nautilus-hyperliquid"
      - dependency-name: "nautilus-kraken"
      - dependency-name: "nautilus-live"
      - dependency-name: "nautilus-model"
      - dependency-name: "nautilus-network"
      - dependency-name: "nautilus-okx"
      - dependency-name: "nautilus-persistence"
      - dependency-name: "nautilus-polymarket"
      - dependency-name: "nautilus-system"
      - dependency-name: "nautilus-trading"
      - dependency-name: "nautilus-execution"
  - package-ecosystem: "cargo"
    directory: "/crates/core"
    ignore:
      - dependency-name: "nautilus-common"
  - package-ecosystem: "github-actions"
    directory: "/"
"#,
    )
    .expect("dependabot fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("all cargo dependabot blocks must enforce NT ignores");

    assert!(
        err.to_string()
            .contains("do not match Dependabot NT ignores"),
        "unexpected error: {err}"
    );
}

#[test]
fn duplicate_safe_list_path_and_match_kind_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "docs/"
match = "prefix"
non_overlap_proof = "Documentation paths are not compiled into Bolt or the NT Rust crates."
approved_by = "fixture"
approved_at = "2099-01-01"
revalidate_after = "2099-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "docs"

[[entries]]
path = "./docs/"
match = "prefix"
non_overlap_proof = "Duplicate normalized path."
approved_by = "fixture"
approved_at = "2099-01-01"
revalidate_after = "2099-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "docs"
"#,
    )
    .expect("duplicate safe-list fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("duplicate normalized safe-list paths should fail validation");

    assert!(
        err.to_string()
            .contains("duplicate safe-list entry for docs with match kind Prefix"),
        "unexpected error: {err}"
    );
}

#[test]
fn unsupported_safe_list_condition_value_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "docs/"
match = "prefix"
non_overlap_proof = "Invalid fixture: unsupported upstream-path-kind value."
approved_by = "fixture"
approved_at = "2099-01-01"
revalidate_after = "2099-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "bogus"
"#,
    )
    .expect("invalid safe-list condition fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("unsupported safe-list condition value should fail validation");

    assert!(
        err.to_string().contains(
            "safe-list entry docs/ condition.value must be one of docs, examples, tests, unused-adapter for kind upstream-path-kind"
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn safe_list_condition_path_mismatch_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "src/"
match = "prefix"
non_overlap_proof = "Invalid fixture: src is not a docs path."
approved_by = "fixture"
approved_at = "2099-01-01"
revalidate_after = "2099-01-30"

[entries.condition]
kind = "upstream-path-kind"
value = "docs"
"#,
    )
    .expect("mismatched safe-list condition fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("path/condition mismatch should fail validation");

    assert!(
        err.to_string().contains(
            "safe-list entry src/ condition upstream-path-kind=docs does not match path semantics"
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn effective_review_count_below_classic_floor_fails_closed() {
    let tempdir = temp_fixture("valid_minimal");
    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/expected_branch_protection.toml"),
        r#"schema_version = 1
branch = "main"
enforce_admins = true
allow_deletions = false
allow_force_pushes = false
block_creations = false
dismiss_stale_reviews = true
required_linear_history = false
required_conversation_resolution = false
lock_branch = false
require_signed_commits = false
require_code_owner_reviews = false
required_approving_review_count = 1
strict_required_status_checks = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]
required_status_check_app_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368, nt-pointer-control-plane = 15368, nt-pointer-probe-self-test = 15368 }

[[required_effective_rules]]
type = "deletion"

[[required_effective_rules]]
type = "non_fast_forward"

[[required_effective_rules]]
type = "pull_request"
required_approving_review_count = 0
dismiss_stale_reviews_on_push = false
require_code_owner_review = false
require_last_push_approval = false
required_review_thread_resolution = false
allowed_merge_methods = ["merge", "squash", "rebase"]

[[required_effective_rules]]
type = "required_status_checks"
strict_required_status_checks_policy = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]

[[required_rulesets]]
id = 14763241
name = "Branch governance"
enforcement = "active"
allowed_bypass_actors = []

[[required_rulesets]]
id = 14763242
name = "CI gates"
enforcement = "active"
allowed_bypass_actors = []
"#,
    )
    .expect("mismatched expected branch protection fixture should write");

    let err = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect_err("effective review floor mismatch should fail validation");

    assert!(
        err.to_string().contains(
            "required_effective_rules pull_request required_approving_review_count 0 must match classic required_approving_review_count 1"
        ),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_accepts_matching_fixture() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual = std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
        .expect("matching actual fixture should load");

    compare_branch_protection_response(&expected, &actual)
        .expect("matching branch protection fixture should compare cleanly");
}

#[test]
fn branch_governance_comparison_accepts_matching_fixture() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual_protection =
        std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
            .expect("matching branch protection fixture should load");
    let actual_rules = std::fs::read_to_string(fixture("branch_protection/matching_rules.json"))
        .expect("matching rules fixture should load");
    let actual_rulesets =
        std::fs::read_to_string(fixture("branch_protection/matching_rulesets.json"))
            .expect("matching ruleset details fixture should load");

    compare_branch_governance_responses(
        &expected,
        &actual_protection,
        &actual_rules,
        &actual_rulesets,
    )
    .expect("matching branch governance fixture should compare cleanly");
}

#[test]
fn branch_governance_comparison_rejects_wrong_effective_rule_integration_id() {
    let tempdir = tempfile::tempdir().expect("tempdir should create");
    fs::write(
        tempdir.path().join("expected.toml"),
        r#"schema_version = 1
branch = "main"
enforce_admins = true
allow_deletions = false
allow_force_pushes = false
block_creations = false
dismiss_stale_reviews = true
required_linear_history = false
required_conversation_resolution = false
lock_branch = false
require_signed_commits = false
require_code_owner_reviews = false
required_approving_review_count = 1
strict_required_status_checks = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]
required_status_check_app_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368, nt-pointer-control-plane = 15368, nt-pointer-probe-self-test = 15368 }

[[required_effective_rules]]
type = "deletion"

[[required_effective_rules]]
type = "non_fast_forward"

[[required_effective_rules]]
type = "pull_request"
required_approving_review_count = 1
dismiss_stale_reviews_on_push = false
require_code_owner_review = false
require_last_push_approval = false
required_review_thread_resolution = false
allowed_merge_methods = ["merge", "squash", "rebase"]

[[required_effective_rules]]
type = "required_status_checks"
strict_required_status_checks_policy = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]
required_status_check_integration_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368, nt-pointer-control-plane = 15368, nt-pointer-probe-self-test = 15368 }

[[required_rulesets]]
id = 14763241
name = "Branch governance"
enforcement = "active"
allowed_bypass_actors = []

[[required_rulesets]]
id = 14763242
name = "CI gates"
enforcement = "active"
allowed_bypass_actors = []
"#,
    )
    .expect("expected branch protection fixture should write");

    let expected =
        ExpectedBranchProtection::load_and_validate(&tempdir.path().join("expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual_protection =
        std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
            .expect("matching branch protection fixture should load");
    let actual_rules = r#"[
  { "type": "deletion" },
  { "type": "non_fast_forward" },
  {
    "type": "pull_request",
    "parameters": {
      "required_approving_review_count": 1,
      "dismiss_stale_reviews_on_push": false,
      "required_reviewers": [],
      "require_code_owner_review": false,
      "require_last_push_approval": false,
      "required_review_thread_resolution": false,
      "allowed_merge_methods": ["merge", "squash", "rebase"]
    }
  },
  {
    "type": "required_status_checks",
    "parameters": {
      "strict_required_status_checks_policy": false,
      "do_not_enforce_on_create": false,
      "required_status_checks": [
        { "context": "gate", "integration_id": 15368 },
        { "context": "clippy", "integration_id": 15368 },
        { "context": "test", "integration_id": 15368 },
        { "context": "build", "integration_id": 15368 },
        { "context": "nt-pointer-control-plane", "integration_id": 99999 },
        { "context": "nt-pointer-probe-self-test", "integration_id": 15368 }
      ]
    }
  }
]"#;
    let actual_rulesets =
        std::fs::read_to_string(fixture("branch_protection/matching_rulesets.json"))
            .expect("matching ruleset details fixture should load");

    let err = compare_branch_governance_responses(
        &expected,
        &actual_protection,
        actual_rules,
        &actual_rulesets,
    )
    .expect_err("mismatched effective-rule integration ID should fail closed");

    assert!(
        err.to_string().contains("effective rules differ"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_rejects_unprotected_branch() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual = std::fs::read_to_string(fixture("branch_protection/unprotected_actual.json"))
        .expect("unprotected actual fixture should load");

    let err = compare_branch_protection_response(&expected, &actual)
        .expect_err("unprotected branch should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch protection drift: expected protected branch"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_governance_comparison_rejects_rules_drift() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual_protection =
        std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
            .expect("matching branch protection fixture should load");
    let actual_rules = std::fs::read_to_string(fixture(
        "branch_protection/missing_required_status_rule.json",
    ))
    .expect("mismatched rules fixture should load");
    let actual_rulesets =
        std::fs::read_to_string(fixture("branch_protection/matching_rulesets.json"))
            .expect("matching ruleset details fixture should load");

    let err = compare_branch_governance_responses(
        &expected,
        &actual_protection,
        &actual_rules,
        &actual_rulesets,
    )
    .expect_err("missing ruleset status check should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch governance drift: effective rules differ"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_governance_comparison_rejects_bypass_actor_drift() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual_protection =
        std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
            .expect("matching branch protection fixture should load");
    let actual_rules = std::fs::read_to_string(fixture("branch_protection/matching_rules.json"))
        .expect("matching rules fixture should load");
    let actual_rulesets =
        std::fs::read_to_string(fixture("branch_protection/bypass_actor_rulesets.json"))
            .expect("bypass-actor ruleset details fixture should load");

    let err = compare_branch_governance_responses(
        &expected,
        &actual_protection,
        &actual_rules,
        &actual_rulesets,
    )
    .expect_err("ruleset bypass actor drift should fail comparison");

    assert!(
        err.to_string()
            .contains("branch governance drift: ruleset details differ"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_surfaces_api_message() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual =
        std::fs::read_to_string(fixture("branch_protection/permission_denied_actual.json"))
            .expect("permission-denied fixture should load");

    let err = compare_branch_protection_response(&expected, &actual)
        .expect_err("permission-denied API response should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch protection API error: Must have admin rights to Repository."),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_rejects_wrong_review_count() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual =
        std::fs::read_to_string(fixture("branch_protection/wrong_review_count_actual.json"))
            .expect("wrong-review-count fixture should load");

    let err = compare_branch_protection_response(&expected, &actual)
        .expect_err("wrong review count should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch protection drift: required_approving_review_count expected 1, got 2"),
        "unexpected error: {err}"
    );
}

#[test]
fn branch_protection_comparison_rejects_wrong_required_check_app_id() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual =
        std::fs::read_to_string(fixture("branch_protection/wrong_check_app_id_actual.json"))
            .expect("wrong-app-id fixture should load");

    let err = compare_branch_protection_response(&expected, &actual)
        .expect_err("wrong required-check app id should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch protection drift: required status check app ids differ"),
        "unexpected error: {err}"
    );
}

#[test]
fn drift_lane_workflow_exposes_durable_failure_surface() {
    let workflow = fs::read_to_string(
        repo_root().join(".github/workflows/nt-pointer-branch-governance-drift.yml"),
    )
    .expect("drift workflow should load");
    let yaml: YamlValue =
        serde_yaml::from_str(&workflow).expect("control-plane workflow should parse as YAML");

    let triggers = yaml
        .get(YamlValue::String("on".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("drift workflow should declare triggers");
    assert!(
        triggers.contains_key(YamlValue::String("schedule".to_string())),
        "drift workflow must remain scheduled"
    );
    assert!(
        !triggers.contains_key(YamlValue::String("workflow_dispatch".to_string())),
        "drift workflow must not be manually dispatchable"
    );

    let permissions = yaml
        .get(YamlValue::String("permissions".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("workflow should declare permissions");
    let issues_permission = permissions
        .get(YamlValue::String("issues".to_string()))
        .and_then(YamlValue::as_str)
        .expect("workflow should declare issues permission");
    assert_eq!(issues_permission, "write");

    let steps = yaml
        .get(YamlValue::String("jobs".to_string()))
        .and_then(YamlValue::as_mapping)
        .and_then(|jobs| jobs.get(YamlValue::String("branch_protection_drift".to_string())))
        .and_then(|job| job.get("steps"))
        .and_then(YamlValue::as_sequence)
        .expect("branch_protection_drift steps should exist");

    let fetch = steps
        .iter()
        .find(|step| {
            step.get("name").and_then(YamlValue::as_str) == Some("Fetch branch protection state")
        })
        .expect("fetch step should exist");
    assert_eq!(fetch.get("id").and_then(YamlValue::as_str), Some("fetch"));
    assert_eq!(
        fetch.get("continue-on-error").and_then(YamlValue::as_bool),
        Some(true)
    );

    let compare = steps
        .iter()
        .find(|step| {
            step.get("name").and_then(YamlValue::as_str)
                == Some("Compare branch governance to expected state")
        })
        .expect("compare step should exist");
    assert_eq!(
        compare.get("id").and_then(YamlValue::as_str),
        Some("compare")
    );
    assert_eq!(
        compare.get("if").and_then(YamlValue::as_str),
        Some("steps.fetch.outcome == 'success'")
    );
    assert_eq!(
        compare
            .get("continue-on-error")
            .and_then(YamlValue::as_bool),
        Some(true)
    );

    let expected_failure_if =
        Some("steps.fetch.outcome == 'failure' || steps.compare.outcome == 'failure'");
    for step_name in [
        "Update drift issue on failure",
        "Fail after drift issue update",
    ] {
        let step = steps
            .iter()
            .find(|step| step.get("name").and_then(YamlValue::as_str) == Some(step_name))
            .unwrap_or_else(|| panic!("{step_name} should exist"));
        assert_eq!(
            step.get("if").and_then(YamlValue::as_str),
            expected_failure_if
        );
    }
}
