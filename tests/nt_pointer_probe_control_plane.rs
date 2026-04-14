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
        "src",
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

[tagged_lane]
pr_branch = "automation/nt-pointer-probe"
pr_title_prefix = "NT Pointer Probe"
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
            .contains("registry path does not exist in repo: src/does_not_exist.rs"),
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

    compare_branch_governance_responses(&expected, &actual_protection, &actual_rules)
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

    let err = compare_branch_governance_responses(&expected, &actual_protection, &actual_rules)
        .expect_err("missing ruleset status check should fail drift comparison");

    assert!(
        err.to_string()
            .contains("branch governance drift: effective rules differ"),
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
