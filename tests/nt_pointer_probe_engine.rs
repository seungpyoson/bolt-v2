use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git command should run");
    assert!(
        output.status.success(),
        "git {:?} failed: stdout={}, stderr={}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git stdout should be utf-8")
        .trim()
        .to_string()
}

struct UpstreamRepo {
    dir: TempDir,
    current_sha: String,
    target_sha: String,
    source_ref: String,
}

fn init_upstream_repo() -> UpstreamRepo {
    let tempdir = tempfile::tempdir().expect("upstream tempdir should create");
    let root = tempdir.path();

    fs::create_dir_all(root.join("crates/common/src")).expect("common crate dir should create");
    fs::create_dir_all(root.join("docs")).expect("docs dir should create");
    fs::write(
        root.join("Cargo.toml"),
        r#"[workspace]
members = ["crates/common"]
resolver = "2"
"#,
    )
    .expect("upstream workspace manifest should write");
    fs::write(
        root.join("crates/common/Cargo.toml"),
        r#"[package]
name = "nautilus-common"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )
    .expect("upstream crate manifest should write");
    fs::write(
        root.join("crates/common/src/lib.rs"),
        r#"pub const VERSION: &str = "a";
"#,
    )
    .expect("upstream crate source should write");
    fs::write(root.join("docs/guide.md"), "current\n").expect("upstream docs should write");

    git(root, &["init"]);
    git(root, &["config", "user.name", "fixture"]);
    git(root, &["config", "user.email", "fixture@example.com"]);
    git(root, &["add", "."]);
    git(root, &["commit", "--no-verify", "-m", "current"]);
    let current_sha = git(root, &["rev-parse", "HEAD"]);

    fs::write(
        root.join("crates/common/src/lib.rs"),
        r#"pub const VERSION: &str = "b";
"#,
    )
    .expect("upstream updated crate source should write");
    fs::write(root.join("docs/guide.md"), "updated\n").expect("upstream updated docs should write");
    git(root, &["add", "."]);
    git(root, &["commit", "--no-verify", "-m", "target"]);
    let target_sha = git(root, &["rev-parse", "HEAD"]);
    let source_ref = "update-candidate".to_string();
    git(root, &["branch", &source_ref, &target_sha]);

    UpstreamRepo {
        dir: tempdir,
        current_sha,
        target_sha,
        source_ref,
    }
}

fn init_bolt_repo(upstream: &UpstreamRepo, with_registry_gap: bool) -> TempDir {
    let tempdir = tempfile::tempdir().expect("bolt tempdir should create");
    copy_fixture_tree(&fixture("valid_minimal"), tempdir.path());

    fs::create_dir_all(tempdir.path().join("src")).expect("src dir should create");
    fs::create_dir_all(tempdir.path().join("tests")).expect("tests dir should create");
    fs::create_dir_all(tempdir.path().join("scripts")).expect("scripts dir should create");
    fs::create_dir_all(tempdir.path().join(".github/workflows"))
        .expect("workflow dir should create");
    fs::create_dir_all(tempdir.path().join(".github/nt-pointer-probe"))
        .expect("template dir should create");

    let upstream_url = format!(
        "file://{}",
        upstream
            .dir
            .path()
            .to_str()
            .expect("upstream path should be utf-8")
    );

    fs::write(
        tempdir.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "probe-fixture"
version = "0.1.0"
edition = "2024"

[dependencies]
nautilus-common = {{ git = "{upstream_url}", rev = "{current_sha}" }}
"#,
            current_sha = upstream.current_sha
        ),
    )
    .expect("fixture Cargo.toml should write");
    fs::write(
        tempdir.path().join("src/lib.rs"),
        r#"pub fn version() -> &'static str {
    nautilus_common::VERSION
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_canary_passes() {
        assert_eq!(super::version(), "b");
    }
}
"#,
    )
    .expect("fixture src/lib.rs should write");

    if with_registry_gap {
        fs::write(
            tempdir.path().join("src/gap.rs"),
            r#"pub fn gap_version() -> &'static str {
    nautilus_common::VERSION
}
"#,
        )
        .expect("fixture src/gap.rs should write");
    }

    fs::write(
        tempdir.path().join("tests/probe_canary.rs"),
        r#"#[test]
fn integration_canary_passes() {
    assert_eq!(nautilus_common::VERSION, "b");
    assert_eq!(probe_fixture::version(), "b");
}
"#,
    )
    .expect("fixture integration test should write");

    let shell_canary = tempdir.path().join("tests/verify_probe.sh");
    fs::write(
        &shell_canary,
        r#"#!/bin/bash
set -euo pipefail

cargo check --quiet
"#,
    )
    .expect("fixture shell canary should write");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&shell_canary)
            .expect("shell canary metadata should read")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&shell_canary, permissions)
            .expect("shell canary permissions should set");
    }

    let guard_script = "#!/bin/bash\nset -euo pipefail\nexit 0\n";
    fs::write(
        tempdir.path().join("scripts/nt_pin_block_guard.sh"),
        guard_script,
    )
    .expect("fixture guard script should write");
    #[cfg(unix)]
    {
        let guard_path = tempdir.path().join("scripts/nt_pin_block_guard.sh");
        let mut permissions = fs::metadata(&guard_path)
            .expect("guard script metadata should read")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&guard_path, permissions).expect("guard script permissions should set");
    }

    let justfile = r#"nt-pointer-probe-check-nt-mutation base_ref head_ref:
    echo check mutation

nt-pointer-probe-validate-control-plane:
    echo validate control plane

nt-pointer-probe-self-test:
    echo self test
"#;
    fs::write(tempdir.path().join("justfile"), justfile).expect("fixture justfile should write");

    let control_plane_workflow = r#"name: NT Pointer Control Plane
jobs:
  control_plane:
    runs-on: ubuntu-latest
    steps:
      - name: Block direct NT pin changes
        run: bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#;
    fs::write(
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-control-plane.yml"),
        control_plane_workflow,
    )
    .expect("fixture control-plane workflow should write");

    let dependabot_workflow = r#"name: Dependabot auto-merge
jobs:
  dependabot:
    runs-on: ubuntu-latest
    steps:
      - name: Block NT pin auto-merge
        run: bash scripts/nt_pin_block_guard.sh "$GITHUB_WORKSPACE" "${{ github.event.pull_request.base.ref }}" "${{ github.event.pull_request.number }}"
"#;
    fs::write(
        tempdir
            .path()
            .join(".github/workflows/dependabot-auto-merge.yml"),
        dependabot_workflow,
    )
    .expect("fixture dependabot workflow should write");

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
            .join(".github/nt-pointer-probe/advisory_issue.md"),
        "# Advisory\n",
    )
    .expect("fixture advisory template should write");
    fs::write(
        tempdir.path().join(".github/nt-pointer-probe/draft_pr.md"),
        "# Draft PR\n",
    )
    .expect("fixture draft PR template should write");

    let guard_script_sha = sha256_hex(guard_script.as_bytes());
    let check_mutation_recipe_sha = sha256_hex(b"    echo check mutation\n");
    let validate_control_plane_recipe_sha = sha256_hex(b"    echo validate control plane\n");
    let self_test_recipe_sha = sha256_hex(b"    echo self test\n");

    fs::write(
        tempdir.path().join("config/nt_pointer_probe/control.toml"),
        format!(
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
script_sha256 = "{guard_script_sha}"
check_mutation_recipe = "nt-pointer-probe-check-nt-mutation"
check_mutation_recipe_sha256 = "{check_mutation_recipe_sha}"
validate_control_plane_recipe = "nt-pointer-probe-validate-control-plane"
validate_control_plane_recipe_sha256 = "{validate_control_plane_recipe_sha}"
self_test_recipe = "nt-pointer-probe-self-test"
self_test_recipe_sha256 = "{self_test_recipe_sha}"
control_plane_workflow = ".github/workflows/nt-pointer-control-plane.yml"
control_plane_job = "control_plane"
control_plane_step = "Block direct NT pin changes"
control_plane_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{{{ github.event.pull_request.base.ref }}}}\" \"${{{{ github.event.pull_request.number }}}}\""
dependabot_workflow = ".github/workflows/dependabot-auto-merge.yml"
dependabot_job = "dependabot"
dependabot_step = "Block NT pin auto-merge"
dependabot_run = "bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"${{{{ github.event.pull_request.base.ref }}}}\" \"${{{{ github.event.pull_request.number }}}}\""
"#,
        ),
    )
    .expect("fixture control.toml should write");

    let mut bolt_usage = vec![
        "src/lib.rs".to_string(),
        "tests/probe_canary.rs".to_string(),
    ];
    if with_registry_gap {
        bolt_usage = vec!["src/lib.rs".to_string()];
    }
    fs::write(
        tempdir.path().join("config/nt_pointer_probe/registry.toml"),
        format!(
            r#"schema_version = 1
coverage_classes = [
  "compile-time-api",
  "unit-behavior",
  "integration-behavior",
]

[[seams]]
name = "common_contract"
risk = "Fixture seam."
bolt_usage = [{bolt_usage}]
upstream_prefixes = ["crates/common/src/"]
required_coverage = ["compile-time-api", "unit-behavior", "integration-behavior"]
escalation = "fail"

[[seams.canaries]]
id = "tests/verify_probe.sh::managed_build_contract"
path = "tests/verify_probe.sh"
coverage = "compile-time-api"
assertion = "Shell canary still passes."

[[seams.canaries]]
id = "src/lib.rs::tests::unit_canary_passes"
path = "src/lib.rs"
coverage = "unit-behavior"
assertion = "Unit canary still passes."

[[seams.canaries]]
id = "tests/probe_canary.rs::integration_canary_passes"
path = "tests/probe_canary.rs"
coverage = "integration-behavior"
assertion = "Integration canary still passes."
"#,
            bolt_usage = bolt_usage
                .iter()
                .map(|path| format!("\"{path}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
    .expect("fixture registry.toml should write");

    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/safe_list.toml"),
        r#"schema_version = 1

[[entries]]
path = "docs/"
match = "prefix"
non_overlap_proof = "Docs do not affect runtime behavior."
approved_by = "fixture"
approved_at = "2026-04-15"
revalidate_after = "2026-05-15"

[entries.condition]
kind = "upstream-path-kind"
value = "docs"
"#,
    )
    .expect("fixture safe_list.toml should write");

    fs::write(
        tempdir
            .path()
            .join("config/nt_pointer_probe/replay_set.toml"),
        r#"schema_version = 1

[[entries]]
id = "common-change"
description = "Fixture replay entry."
changed_paths = ["crates/unused/src/lib.rs"]
expected_seams = ["common_contract"]
expected_result = "ambiguous"
"#,
    )
    .expect("fixture replay_set.toml should write");

    git(tempdir.path(), &["init"]);
    git(tempdir.path(), &["config", "user.name", "fixture"]);
    git(
        tempdir.path(),
        &["config", "user.email", "fixture@example.com"],
    );
    git(tempdir.path(), &["add", "."]);
    git(tempdir.path(), &["commit", "--no-verify", "-m", "fixture"]);

    tempdir
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn run_dry_run(
    repo_root: &Path,
    upstream_root: &Path,
    source_ref: &str,
    artifact_out: &Path,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_nt_pointer_probe"))
        .args([
            "dry-run",
            "--repo-root",
            repo_root.to_str().expect("repo path should be utf-8"),
            "--lane",
            "tagged-release",
            "--source-ref",
            source_ref,
            "--artifact-out",
            artifact_out
                .to_str()
                .expect("artifact path should be utf-8"),
            "--upstream-repo-root",
            upstream_root
                .to_str()
                .expect("upstream repo path should be utf-8"),
        ])
        .output()
        .expect("dry-run command should execute")
}

fn parse_artifact(path: &Path) -> Value {
    let contents = fs::read_to_string(path).expect("artifact should exist");
    serde_json::from_str(&contents).expect("artifact should be valid JSON")
}

#[test]
fn dry_run_writes_artifact_and_runs_required_canaries() {
    let upstream = init_upstream_repo();
    let repo = init_bolt_repo(&upstream, false);
    let artifact = repo.path().join("probe-artifact.json");

    let output = run_dry_run(
        repo.path(),
        upstream.dir.path(),
        &upstream.source_ref,
        &artifact,
    );

    assert!(
        output.status.success(),
        "dry-run should succeed once implemented, stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let artifact = parse_artifact(&artifact);
    assert_eq!(artifact["schema_version"], 1);
    assert_eq!(artifact["requested_ref"], upstream.source_ref);
    assert_eq!(artifact["previous_nt_sha"], upstream.current_sha);
    assert_eq!(artifact["resolved_nt_sha"], upstream.target_sha);
    assert_eq!(artifact["result"], "pass");
    assert_eq!(
        artifact["required_seams"],
        serde_json::json!(["common_contract"])
    );

    let classifications = artifact["upstream_diff"]["changed_paths"]
        .as_array()
        .expect("changed_paths should be an array");
    assert!(
        classifications.iter().any(|entry| {
            entry["path"] == "docs/guide.md" && entry["classification"] == "safe_list"
        }),
        "artifact should record docs path as safe-listed"
    );
    assert!(
        classifications.iter().any(|entry| {
            entry["path"] == "crates/common/src/lib.rs"
                && entry["classification"] == "seam"
                && entry["seams"] == serde_json::json!(["common_contract"])
        }),
        "artifact should record common source path as seam-owned"
    );

    let canaries = artifact["required_canaries"]
        .as_array()
        .expect("required_canaries should be an array");
    assert_eq!(canaries.len(), 3);
    assert!(
        canaries.iter().all(|entry| entry["status"] == "passed"),
        "all fixture canaries should pass once implemented"
    );

    assert_eq!(
        artifact["inventory"]["production"]
            .as_array()
            .expect("production inventory should be an array")
            .len(),
        1
    );
    assert_eq!(
        artifact["inventory"]["test_support"]
            .as_array()
            .expect("test inventory should be an array")
            .len(),
        1
    );
}

#[test]
fn dry_run_surfaces_registry_gap_and_still_writes_artifact() {
    let upstream = init_upstream_repo();
    let repo = init_bolt_repo(&upstream, true);
    let artifact = repo.path().join("probe-artifact.json");

    let output = run_dry_run(
        repo.path(),
        upstream.dir.path(),
        &upstream.source_ref,
        &artifact,
    );

    assert!(
        !output.status.success(),
        "dry-run should fail closed on registry gaps once implemented"
    );

    let artifact = parse_artifact(&artifact);
    assert_eq!(artifact["result"], "fail");
    assert!(
        artifact["failures"]
            .as_array()
            .expect("failures should be an array")
            .iter()
            .any(|entry| entry == "registry_gap"),
        "artifact should record registry_gap failure"
    );
    assert!(
        artifact["inventory"]["registry_gaps"]
            .as_array()
            .expect("registry_gaps should be an array")
            .iter()
            .any(|entry| entry["path"] == "src/gap.rs"),
        "artifact should record the missing seam owner"
    );
}

#[test]
fn dry_run_uses_working_tree_snapshot_not_head() {
    let upstream = init_upstream_repo();
    let repo = init_bolt_repo(&upstream, false);
    let artifact = repo.path().join("probe-artifact.json");

    fs::write(
        repo.path().join("src/lib.rs"),
        r#"pub fn version() -> &'static str {
    nautilus_common::VERSION
}

#[cfg(test)]
mod tests {
    #[test]
    fn unit_canary_passes() {
        assert_eq!(super::version(), "not-b");
    }
}
"#,
    )
    .expect("dirty src/lib.rs should write");

    let output = run_dry_run(
        repo.path(),
        upstream.dir.path(),
        &upstream.source_ref,
        &artifact,
    );

    assert!(
        !output.status.success(),
        "dirty working-tree snapshot should fail canary execution"
    );

    let artifact = parse_artifact(&artifact);
    assert_eq!(artifact["result"], "fail");
    assert!(
        artifact["failures"]
            .as_array()
            .expect("failures should be an array")
            .iter()
            .any(|entry| entry == "canary_failed"),
        "artifact should record canary failure from dirty working tree"
    );
}

#[test]
fn dry_run_replay_ambiguity_fails_closed() {
    let upstream = init_upstream_repo();
    let repo = init_bolt_repo(&upstream, false);
    let artifact = repo.path().join("probe-artifact.json");

    fs::write(
        repo.path().join("config/nt_pointer_probe/replay_set.toml"),
        r#"schema_version = 1

[[entries]]
id = "common-change"
description = "Fixture replay entry."
changed_paths = ["crates/common/src/lib.rs"]
expected_seams = ["common_contract"]
expected_result = "ambiguous"
"#,
    )
    .expect("replay_set.toml should update");

    let output = run_dry_run(
        repo.path(),
        upstream.dir.path(),
        &upstream.source_ref,
        &artifact,
    );

    assert!(
        !output.status.success(),
        "replay ambiguity should fail closed"
    );

    let artifact = parse_artifact(&artifact);
    let changed_paths = artifact["upstream_diff"]["changed_paths"]
        .as_array()
        .expect("changed_paths should be an array");
    assert!(
        changed_paths.iter().any(|entry| {
            entry["path"] == "crates/common/src/lib.rs" && entry["classification"] == "ambiguous"
        }),
        "artifact should mark replay-matched path ambiguous"
    );
}

#[test]
fn dry_run_rejects_zero_match_unit_canary_selectors() {
    let upstream = init_upstream_repo();
    let repo = init_bolt_repo(&upstream, false);
    let artifact = repo.path().join("probe-artifact.json");

    fs::write(
        repo.path().join("config/nt_pointer_probe/registry.toml"),
        r#"schema_version = 1
coverage_classes = [
  "compile-time-api",
  "unit-behavior",
  "integration-behavior",
]

[[seams]]
name = "common_contract"
risk = "Fixture seam."
bolt_usage = ["src/lib.rs", "tests/probe_canary.rs"]
upstream_prefixes = ["crates/common/src/"]
required_coverage = ["compile-time-api", "unit-behavior", "integration-behavior"]
escalation = "fail"

[[seams.canaries]]
id = "tests/verify_probe.sh::managed_build_contract"
path = "tests/verify_probe.sh"
coverage = "compile-time-api"
assertion = "Shell canary still passes."

[[seams.canaries]]
id = "src/lib.rs::tests::missing_unit_canary"
path = "src/lib.rs"
coverage = "unit-behavior"
assertion = "Bogus unit canary should fail."

[[seams.canaries]]
id = "tests/probe_canary.rs::integration_canary_passes"
path = "tests/probe_canary.rs"
coverage = "integration-behavior"
assertion = "Integration canary still passes."
"#,
    )
    .expect("registry.toml should update");

    let output = run_dry_run(
        repo.path(),
        upstream.dir.path(),
        &upstream.source_ref,
        &artifact,
    );

    assert!(
        !output.status.success(),
        "zero-match unit canary selector should fail closed"
    );

    let artifact = parse_artifact(&artifact);
    assert!(
        artifact["required_canaries"]
            .as_array()
            .expect("required_canaries should be an array")
            .iter()
            .any(|entry| {
                entry["id"] == "src/lib.rs::tests::missing_unit_canary"
                    && entry["status"] == "failed"
            }),
        "artifact should record failed zero-match unit canary"
    );
}
