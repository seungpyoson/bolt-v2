use std::{fs, path::Path, path::PathBuf};

use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_governance_responses,
    compare_branch_protection_response,
};
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
        "tests/reference_actor.rs",
        "tests/reference_pipeline.rs",
        ".github/dependabot.yml",
        ".github/workflows/nt-pointer-control-plane.yml",
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
check_mutation_recipe = "nt-pointer-probe-check-nt-mutation"
check_mutation_recipe_sha256 = "7dd549c4410e59938eee58967b29fb6b78bafc886ad8dcee54b416a6277c93b2"
validate_control_plane_recipe = "nt-pointer-probe-validate-control-plane"
validate_control_plane_recipe_sha256 = "5bba05dd823f5d2196c4b4b2eb996001f21679df96c96e403ebe61b1c6ea6f8b"
self_test_recipe = "nt-pointer-probe-self-test"
self_test_recipe_sha256 = "06a2d6cd1a7d82a843b3acb04805465819eca5514aaf89cd1e7181dbd53c33f8"
control_plane_workflow = ".github/workflows/nt-pointer-control-plane.yml"
control_plane_job = "control_plane"
control_plane_step = "Block direct NT pin changes"
control_plane_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{ github.event.pull_request.base.ref }}\" \"${{ github.event.pull_request.number }}\""
dependabot_workflow = ".github/workflows/dependabot-auto-merge.yml"
dependabot_job = "dependabot"
dependabot_step = "Block NT pin auto-merge"
dependabot_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{ github.event.pull_request.base.ref }}\" \"${{ github.event.pull_request.number }}\""
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
    fs::write(
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-control-plane.yml"),
        r#"name: NT Pointer Control Plane
jobs:
  control_plane:
    runs-on: ubuntu-latest
    steps:
      - name: Block direct NT pin changes
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#,
    )
    .expect("fixture control-plane workflow should write");
    fs::write(
        tempdir
            .path()
            .join(".github/workflows/dependabot-auto-merge.yml"),
        r#"name: Dependabot auto-merge
jobs:
  dependabot:
    runs-on: ubuntu-latest
    steps:
      - name: Block NT pin auto-merge
        run: |
          bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#,
    )
    .expect("fixture dependabot workflow should write");
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
        repo_root().join("justfile"),
        tempdir.path().join("justfile"),
    )
    .expect("fixture justfile should copy");

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
            .contains(
                "safe-list entry crates/ condition.value must be one of docs, examples, tests, unused-adapter for kind upstream-path-kind"
            ),
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
check_mutation_recipe = "nt-pointer-probe-check-nt-mutation"
check_mutation_recipe_sha256 = "7dd549c4410e59938eee58967b29fb6b78bafc886ad8dcee54b416a6277c93b2"
validate_control_plane_recipe = "nt-pointer-probe-validate-control-plane"
validate_control_plane_recipe_sha256 = "5bba05dd823f5d2196c4b4b2eb996001f21679df96c96e403ebe61b1c6ea6f8b"
self_test_recipe = "nt-pointer-probe-self-test"
self_test_recipe_sha256 = "06a2d6cd1a7d82a843b3acb04805465819eca5514aaf89cd1e7181dbd53c33f8"
control_plane_workflow = ".github/workflows/nt-pointer-control-plane.yml"
control_plane_job = "control_plane"
control_plane_step = "Block direct NT pin changes"
control_plane_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{ github.event.pull_request.base.ref }}\" \"${{ github.event.pull_request.number }}\""
dependabot_workflow = ".github/workflows/dependabot-auto-merge.yml"
dependabot_job = "dependabot"
dependabot_step = "Block NT pin auto-merge"
dependabot_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{ github.event.pull_request.base.ref }}\" \"${{ github.event.pull_request.number }}\""
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
