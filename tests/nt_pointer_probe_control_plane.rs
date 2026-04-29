use std::{fs, path::Path, path::PathBuf, process::Command};

use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_governance_responses,
    compare_branch_protection_response,
};
use serde_json::Value as JsonValue;
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

fn external_claude_config_root() -> Option<PathBuf> {
    [
        repo_root().join("../worktrees/claude-config/feat-565-bolt-v2-trust-root-guard"),
        repo_root().join("../../claude-config"),
    ]
    .into_iter()
    .find(|candidate| {
        candidate.join("lib/bolt_trust_root_validator.py").exists()
            && candidate
                .join("config/bolt-v2-trust-root-policy.json")
                .exists()
    })
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
        ".github/workflows/nt-pointer-trust-root.yml",
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
trust_root = "nt-pointer-trust-root"
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
nautilus-common = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "48d1c126335b82812ba691c5661aeb2e912cde24" }
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
        repo_root().join(".github/workflows/nt-pointer-trust-root.yml"),
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-trust-root.yml"),
    )
    .expect("fixture trust-root workflow should copy");
    fs::copy(
        repo_root().join(".github/workflows/nt-pointer-probe-self-test.yml"),
        tempdir
            .path()
            .join(".github/workflows/nt-pointer-probe-self-test.yml"),
    )
    .expect("fixture self-test workflow should copy");
    fs::copy(
        repo_root().join(".github/workflows/nt-pointer-branch-governance-drift.yml"),
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
nautilus-common = { git = "https://github.com/nautechsystems/nautilus_trader.git", rev = "48d1c126335b82812ba691c5661aeb2e912cde24", features = ["extra-surface"] }
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

fn nt_pointer_probe_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_nt_pointer_probe"))
}

fn workflow_job_step<'a>(
    yaml: &'a YamlValue,
    job_name: &str,
    step_name: &str,
) -> &'a serde_yaml::Mapping {
    let jobs = yaml
        .get(YamlValue::String("jobs".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("workflow should declare jobs");
    let job = jobs
        .get(YamlValue::String(job_name.to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("workflow should declare the expected job");
    let steps = job
        .get(YamlValue::String("steps".to_string()))
        .and_then(YamlValue::as_sequence)
        .expect("workflow job should declare steps");

    steps
        .iter()
        .find_map(|step| {
            let step = step.as_mapping()?;
            let name = step
                .get(YamlValue::String("name".to_string()))
                .and_then(YamlValue::as_str)?;
            (name == step_name).then_some(step)
        })
        .expect("workflow should declare the expected step")
}

fn workflow_step_env_value<'a>(step: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    step.get(YamlValue::String("env".to_string()))
        .and_then(YamlValue::as_mapping)
        .and_then(|env| env.get(YamlValue::String(key.to_string())))
        .and_then(YamlValue::as_str)
}

fn workflow_step_run(step: &serde_yaml::Mapping) -> &str {
    step.get(YamlValue::String("run".to_string()))
        .and_then(YamlValue::as_str)
        .expect("workflow step should declare a run script")
}

fn normalized_shell_script(run_script: &str) -> String {
    run_script
        .replace("\\\n", " ")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn script_contains_all(haystack: &str, fragments: &[&str]) -> bool {
    fragments.iter().all(|fragment| haystack.contains(fragment))
}

fn script_contains_none(haystack: &str, fragments: &[&str]) -> bool {
    fragments
        .iter()
        .all(|fragment| !haystack.contains(fragment))
}

fn logical_shell_lines(run_script: &str) -> Vec<String> {
    let mut logical_lines = Vec::new();
    let mut current = String::new();

    for raw_line in run_script.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let continued = trimmed.strip_suffix('\\').unwrap_or(trimmed).trim();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(continued);

        if !trimmed.ends_with('\\') {
            logical_lines.push(current.trim().to_string());
            current.clear();
        }
    }

    if !current.is_empty() {
        logical_lines.push(current.trim().to_string());
    }

    logical_lines
}

fn split_shell_commands(shell: &str) -> Vec<String> {
    shell
        .split("&&")
        .flat_map(|part| part.split("||"))
        .flat_map(|part| part.split(';'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn logical_shell_commands(run_script: &str) -> Vec<String> {
    logical_shell_lines(run_script)
        .into_iter()
        .flat_map(|line| split_shell_commands(&line))
        .collect()
}

fn shell_has_command_with_fragments(run_script: &str, fragments: &[&str]) -> bool {
    logical_shell_commands(run_script)
        .iter()
        .any(|command| script_contains_all(command, fragments))
}

fn shell_command_fetches_exact_pinned_sha(command: &str) -> bool {
    let tokens = command.split_whitespace().collect::<Vec<_>>();
    let Some(fetch_index) = tokens.iter().position(|token| *token == "fetch") else {
        return false;
    };

    let after_fetch = &tokens[fetch_index + 1..];
    let Some(origin_index) = after_fetch.iter().position(|token| *token == "origin") else {
        return false;
    };

    origin_index == 2
        && after_fetch.len() == origin_index + 2
        && after_fetch[origin_index + 1] == r#""$TRUST_ROOT_VALIDATOR_SHA""#
        && after_fetch[..origin_index].contains(&"--depth=1")
        && after_fetch[..origin_index].contains(&"--no-tags")
}

fn fetch_uses_approved_header_auth_for_pinned_sha(run_script: &str) -> bool {
    logical_shell_commands(run_script).iter().any(|command| {
        let uses_env_scoped_header_auth = command.contains("GIT_CONFIG_KEY_0=http.extraheader")
            && command.contains(r#"GIT_CONFIG_VALUE_0="AUTHORIZATION: basic "#);
        let uses_git_c_header_auth =
            command.contains(r#"-c http.extraheader="AUTHORIZATION: basic "#);

        shell_command_fetches_exact_pinned_sha(command)
            && (uses_env_scoped_header_auth || uses_git_c_header_auth)
    })
}

fn remote_url_embeds_credentials(run_script: &str) -> bool {
    logical_shell_commands(run_script)
        .iter()
        .any(|command| command.contains("https://") && command.contains("@github.com"))
}

fn persists_http_extraheader(run_script: &str) -> bool {
    logical_shell_commands(run_script).iter().any(|command| {
        command.contains("git config")
            && command.contains("extraheader")
            && !command.contains("--unset")
    })
}

fn materialization_mentions_head_ref(run_script: &str) -> bool {
    let script = normalized_shell_script(run_script);
    script.contains("${HEAD_REF}") || script.contains("github.event.pull_request.head.ref")
}

fn mismatch_guard_exits_nonzero(run_script: &str) -> bool {
    let guard_prefix = r#"if [ "$fetched_sha" != "$TRUST_ROOT_VALIDATOR_SHA" ]; then"#;
    let mut inside_guard = false;

    for line in run_script
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !inside_guard {
            if line == guard_prefix {
                inside_guard = true;
            }
            continue;
        }

        if line == "fi" {
            return false;
        }

        if line == "exit 1" || line.starts_with("exit 1 ") {
            return true;
        }
    }

    false
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
fn control_plane_load_no_longer_requires_guard_contract_block() {
    let tempdir = temp_fixture("valid_minimal");

    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("semantic control-plane validation should not depend on repo-local guard hashes");

    assert_eq!(loaded.control.repo, "seungpyoson/bolt-v2");
}

#[test]
fn validate_control_plane_subprocess_fails_closed_on_invalid_fixture() {
    let tempdir = temp_fixture("bad_shared_crate_prefix");
    let output = nt_pointer_probe_command()
        .args(["validate-control-plane", "--repo-root"])
        .arg(tempdir.path())
        .output()
        .expect("nt_pointer_probe validate-control-plane should run");

    assert!(
        !output.status.success(),
        "invalid control-plane fixture must fail closed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("shared NT crate safe-list entries must use exact match"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stdout.contains("validated control plane"),
        "invalid control-plane fixture must not report success: {stdout}"
    );
}

#[test]
#[cfg(debug_assertions)]
fn nt_pointer_probe_subprocess_aborts_before_success_when_panicking() {
    let tempdir = temp_fixture("valid_minimal");
    let output = nt_pointer_probe_command()
        .env("BOLT_NT_POINTER_PROBE_TEST_PANIC", "before-parse")
        .args(["validate-control-plane", "--repo-root"])
        .arg(tempdir.path())
        .output()
        .expect("nt_pointer_probe panic smoke path should run");

    assert!(
        !output.status.success(),
        "panic smoke path must terminate unsuccessfully"
    );

    #[cfg(unix)]
    assert_eq!(
        std::os::unix::process::ExitStatusExt::signal(&output.status),
        Some(6),
        "panic smoke path must terminate via SIGABRT"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nt_pointer_probe test panic before CLI parse"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stdout.contains("validated control plane"),
        "panic smoke path must abort before success handling: {stdout}"
    );
}

#[test]
fn compare_branch_protection_subprocess_fails_closed_on_drift_fixture() {
    let tempdir = temp_fixture("valid_minimal");
    fs::copy(
        fixture("branch_protection/expected.toml"),
        tempdir
            .path()
            .join("config/nt_pointer_probe/expected_branch_protection.toml"),
    )
    .expect("expected branch protection fixture should copy into temp repo");

    let output = nt_pointer_probe_command()
        .args(["compare-branch-protection", "--repo-root"])
        .arg(tempdir.path())
        .arg("--actual-json")
        .arg(fixture("branch_protection/unprotected_actual.json"))
        .output()
        .expect("nt_pointer_probe compare-branch-protection should run");

    assert!(
        !output.status.success(),
        "branch protection drift fixture must fail closed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("branch protection drift: expected protected branch"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stdout.contains("branch protection matches expected state"),
        "drift fixture must not report success: {stdout}"
    );
}

#[test]
fn compare_branch_governance_subprocess_fails_closed_on_rules_drift_fixture() {
    let tempdir = temp_fixture("valid_minimal");
    fs::copy(
        fixture("branch_protection/expected.toml"),
        tempdir
            .path()
            .join("config/nt_pointer_probe/expected_branch_protection.toml"),
    )
    .expect("expected branch protection fixture should copy into temp repo");

    let output = nt_pointer_probe_command()
        .args(["compare-branch-governance", "--repo-root"])
        .arg(tempdir.path())
        .arg("--actual-json")
        .arg(fixture("branch_protection/matching_actual.json"))
        .arg("--actual-rules-json")
        .arg(fixture(
            "branch_protection/missing_required_status_rule.json",
        ))
        .arg("--actual-ruleset-details-json")
        .arg(fixture("branch_protection/matching_rulesets.json"))
        .output()
        .expect("nt_pointer_probe compare-branch-governance should run");

    assert!(
        !output.status.success(),
        "branch governance drift fixture must fail closed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("branch governance drift: effective rules differ"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stdout.contains("branch governance matches expected state"),
        "drift fixture must not report success: {stdout}"
    );
}

#[test]
fn check_nt_mutation_subprocess_fails_closed_on_nt_manifest_change() {
    let tempdir = init_temp_git_repo();
    let output = nt_pointer_probe_command()
        .args(["check-nt-mutation", "--repo-root"])
        .arg(tempdir.path())
        .args(["--base-ref", "HEAD~1", "--head-ref", "HEAD"])
        .output()
        .expect("nt_pointer_probe check-nt-mutation should run");

    assert!(
        !output.status.success(),
        "NT manifest mutation fixture must fail closed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Cargo.toml changed NT dependency records"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !stdout.contains("no unmanaged NT mutations detected"),
        "mutation fixture must not report success: {stdout}"
    );
}

#[test]
fn trust_root_workflow_is_pull_request_target_and_pins_external_validator() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/nt-pointer-trust-root.yml"))
            .expect("trust-root workflow should load");
    let yaml: YamlValue =
        serde_yaml::from_str(&workflow).expect("trust-root workflow should parse as YAML");
    let materialize_step = workflow_job_step(
        &yaml,
        "trust_root",
        "Materialize protected files from PR head",
    );
    let materialize_run = workflow_step_run(materialize_step);

    let triggers = yaml
        .get(YamlValue::String("on".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("trust-root workflow should declare triggers");
    assert!(
        triggers.contains_key(YamlValue::String("pull_request_target".to_string())),
        "trust-root workflow must run on pull_request_target"
    );
    assert!(
        !triggers.contains_key(YamlValue::String("pull_request".to_string())),
        "trust-root workflow must not run on pull_request"
    );

    assert!(
        workflow.contains("Validate external trust root"),
        "trust-root workflow must run the external validator"
    );
    assert!(
        !workflow.contains("Validate control-plane artifacts"),
        "trust-root workflow must not run repo-local semantic validation"
    );
    assert!(
        !workflow.contains("actions/checkout@"),
        "trust-root workflow must not checkout PR content"
    );
    assert!(
        !workflow.contains("git checkout --detach"),
        "trust-root workflow must not checkout the PR head SHA"
    );
    assert!(
        shell_has_command_with_fragments(
            materialize_run,
            &[
                "jq -r '.protected_entries[].path'",
                r#""$RUNNER_TEMP/bolt-v2-trust-root-policy.json""#,
                "while read -r relative_path",
            ],
        ),
        "trust-root workflow must source protected paths from the policy jq query"
    );
    assert!(
        workflow.contains("Validate trust-root validator SHA pin"),
        "trust-root workflow must validate the external validator ref shape before fetching it"
    );
    assert!(
        workflow.contains("^[0-9a-f]{40}$"),
        "trust-root workflow must reject non-commit validator refs"
    );

    let sha_line = workflow
        .lines()
        .find(|line| line.contains("TRUST_ROOT_VALIDATOR_SHA:"))
        .expect("workflow should pin an external validator SHA");
    assert!(
        sha_line.contains("${{ vars.NT_POINTER_TRUST_ROOT_VALIDATOR_SHA }}"),
        "validator SHA must come from a GitHub variable, not a repo-owned literal"
    );
    assert!(
        !workflow.contains("refs/heads/${{ github.event.pull_request.head.ref }}"),
        "workflow must not interpolate head.ref directly into shell"
    );
    assert!(
        !workflow.contains("github.event.pull_request.head.ref"),
        "workflow must not reference head.ref at all in the no-checkout design"
    );
    assert!(
        workflow_step_env_value(materialize_step, "HEAD_REPO_FULL_NAME")
            == Some("${{ github.event.pull_request.head.repo.full_name }}"),
        "trust-root materialization must source the PR head repository through a scoped step env"
    );
    assert!(
        workflow_step_env_value(materialize_step, "HEAD_SHA")
            == Some("${{ github.event.pull_request.head.sha }}"),
        "trust-root materialization must source the exact PR head SHA through a scoped step env"
    );
    assert!(
        shell_has_command_with_fragments(
            materialize_run,
            &[
                "raw.githubusercontent.com",
                "${HEAD_REPO_FULL_NAME}",
                "${HEAD_SHA}",
                "${relative_path}",
            ],
        ),
        "trust-root materialization must fetch protected files from HEAD_REPO_FULL_NAME at the exact HEAD_SHA"
    );
    assert!(
        shell_has_command_with_fragments(
            materialize_run,
            &["curl", r#""$target_path""#, r#""$file_url""#],
        ),
        "trust-root materialization must download the exact HEAD_SHA raw URL into the materialized target path"
    );
    assert!(
        !materialization_mentions_head_ref(materialize_run),
        "trust-root materialization must not reference HEAD_REF or github.event.pull_request.head.ref"
    );
}

#[test]
fn shell_fragment_matcher_tracks_logical_commands_across_continuations() {
    let continued_fetch_line = r#"GIT_CONFIG_COUNT=1 \
GIT_CONFIG_KEY_0=http.extraheader \
GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $auth_header" \
  git -C "$source_repo" fetch --depth=1 --no-tags origin "$TRUST_ROOT_VALIDATOR_SHA""#;

    assert!(
        shell_has_command_with_fragments(
            continued_fetch_line,
            &[
                "http.extraheader",
                "AUTHORIZATION: basic",
                r#"git -C "$source_repo" fetch"#,
                "--depth=1",
                r#""$TRUST_ROOT_VALIDATOR_SHA""#,
            ],
        ),
        "shell fragment matcher must treat backslash-continued commands as one logical shell command"
    );
}

#[test]
fn trust_root_workflow_uses_authenticated_private_bundle_fetch_path() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/nt-pointer-trust-root.yml"))
            .expect("trust-root workflow should load");
    let yaml: YamlValue =
        serde_yaml::from_str(&workflow).expect("trust-root workflow should parse as YAML");
    let fetch_step = workflow_job_step(&yaml, "trust_root", "Fetch external trust-root validator");
    let fetch_run = workflow_step_run(fetch_step);

    assert!(
        !workflow.contains("uses: ./.github/actions/setup-environment"),
        "trust-root workflow must not widen the privileged bootstrap boundary through setup-environment"
    );
    assert!(
        !workflow.contains("claude-config-read-token:"),
        "trust-root workflow must not route the claude-config token through a broad shared bootstrap action"
    );
    assert!(
        !workflow.contains("JUST_VERSION:"),
        "trust-root workflow must not carry unrelated workflow-level tool bootstrap state"
    );
    assert!(
        !workflow.contains("just-version:"),
        "trust-root workflow must not satisfy unrelated setup action contracts"
    );
    assert!(
        !workflow.contains("https://raw.githubusercontent.com/${TRUST_ROOT_VALIDATOR_REPO}/${TRUST_ROOT_VALIDATOR_SHA}"),
        "trust-root workflow must not anonymously fetch the private claude-config bundle from raw.githubusercontent.com"
    );
    assert!(
        workflow_step_env_value(fetch_step, "CLAUDE_CONFIG_READ_TOKEN")
            == Some("${{ secrets.CLAUDE_CONFIG_READ_TOKEN }}"),
        "trust-root workflow must scope the private-read token to the external bundle fetch step"
    );
    assert!(
        shell_has_command_with_fragments(
            fetch_run,
            &[
                r#"git -C "$source_repo" remote add origin"#,
                r#""https://github.com/${TRUST_ROOT_VALIDATOR_REPO}.git""#,
            ],
        ),
        "trust-root workflow must use a plain GitHub HTTPS remote for the private validator repo"
    );
    assert!(
        !remote_url_embeds_credentials(fetch_run),
        "trust-root workflow must not embed credentials in the remote URL"
    );
    assert!(
        script_contains_none(
            &normalized_shell_script(fetch_run),
            &[
                "remote set-url origin",
                "git@github.com:",
                "ssh://git@github.com",
                "ssh://github.com",
            ],
        ),
        "trust-root workflow must not rewrite origin away from the plain GitHub HTTPS remote"
    );
    assert!(
        shell_has_command_with_fragments(
            fetch_run,
            &["CLAUDE_CONFIG_READ_TOKEN", "x-access-token:%s", "base64"],
        ),
        "trust-root workflow must derive the Authorization header value from CLAUDE_CONFIG_READ_TOKEN"
    );
    assert!(
        fetch_uses_approved_header_auth_for_pinned_sha(fetch_run),
        "trust-root workflow must attach approved fetch-scoped http.extraheader auth to the exact pinned git fetch"
    );
    assert!(
        !persists_http_extraheader(fetch_run),
        "trust-root workflow must not persist http.extraheader via git config outside the fetch command"
    );
    assert!(
        shell_has_command_with_fragments(fetch_run, &["fetched_sha=", "rev-parse FETCH_HEAD"]),
        "trust-root workflow must resolve FETCH_HEAD to a concrete commit SHA before using fetched bundle files"
    );
    assert!(
        mismatch_guard_exits_nonzero(fetch_run),
        "trust-root workflow must fail closed with exit 1 when FETCH_HEAD does not match the pinned trust-root bundle SHA"
    );
    assert!(
        shell_has_command_with_fragments(
            fetch_run,
            &[
                r#"git -C "$source_repo""#,
                "show",
                "FETCH_HEAD:lib/bolt_trust_root_validator.py",
            ],
        ),
        "trust-root workflow must read the validator bundle from the fetched pinned commit"
    );
    assert!(
        shell_has_command_with_fragments(
            fetch_run,
            &[
                r#"git -C "$source_repo""#,
                "show",
                "FETCH_HEAD:config/bolt-v2-trust-root-policy.json",
            ],
        ),
        "trust-root workflow must read the policy bundle from the fetched pinned commit"
    );
    assert!(
        script_contains_none(
            &normalized_shell_script(fetch_run),
            &["raw.githubusercontent.com/${TRUST_ROOT_VALIDATOR_REPO}/${TRUST_ROOT_VALIDATOR_SHA}"],
        ),
        "trust-root workflow must not anonymously fetch the private bundle from raw.githubusercontent.com"
    );
}

#[test]
fn trust_root_workflow_authenticated_fetch_matcher_rejects_tokenized_remote_urls() {
    let tokenized_remote_run = r#"auth_header="$(printf 'x-access-token:%s' "$CLAUDE_CONFIG_READ_TOKEN" | base64 | tr -d '\n')"
git -C "$source_repo" remote add origin "https://x-access-token:${CLAUDE_CONFIG_READ_TOKEN}@github.com/${TRUST_ROOT_VALIDATOR_REPO}.git"
GIT_CONFIG_COUNT=1 \
GIT_CONFIG_KEY_0=http.extraheader \
GIT_CONFIG_VALUE_0="AUTHORIZATION: basic $auth_header" \
  git -C "$source_repo" fetch --depth=1 --no-tags origin "$TRUST_ROOT_VALIDATOR_SHA""#;

    assert!(
        remote_url_embeds_credentials(tokenized_remote_run),
        "tokenized remote URLs must remain detectable as a trust-root credential persistence regression"
    );
}

#[test]
fn trust_root_workflow_fetch_auth_matcher_accepts_git_c_extraheader_form() {
    let git_c_extraheader_run = r#"auth_header="$(printf 'x-access-token:%s' "$CLAUDE_CONFIG_READ_TOKEN" | base64 | tr -d '\n')"
git -C "$source_repo" remote add origin "https://github.com/${TRUST_ROOT_VALIDATOR_REPO}.git"
git -C "$source_repo" -c http.extraheader="AUTHORIZATION: basic $auth_header" fetch --depth=1 --no-tags origin "$TRUST_ROOT_VALIDATOR_SHA""#;

    assert!(
        fetch_uses_approved_header_auth_for_pinned_sha(git_c_extraheader_run),
        "the fetch-auth matcher must accept the equivalent git -c http.extraheader form"
    );
}

#[test]
fn trust_root_workflow_fetch_auth_matcher_rejects_persisted_http_extraheader_config() {
    let persisted_auth_run = r#"auth_header="$(printf 'x-access-token:%s' "$CLAUDE_CONFIG_READ_TOKEN" | base64 | tr -d '\n')"
git -C "$source_repo" remote add origin "https://github.com/${TRUST_ROOT_VALIDATOR_REPO}.git"
git config --global http.extraheader "AUTHORIZATION: basic $auth_header"
git -C "$source_repo" fetch --depth=1 --no-tags origin "$TRUST_ROOT_VALIDATOR_SHA""#;

    assert!(
        persists_http_extraheader(persisted_auth_run),
        "persisted git config http.extraheader state must remain detectable as a trust-root auth-scope regression"
    );
}

#[test]
fn trust_root_workflow_materialization_matcher_rejects_head_ref_raw_urls() {
    let head_ref_materialization_run = r#"repo_root="$RUNNER_TEMP/bolt-v2-trust-root"
mkdir -p "$repo_root"
jq -r '.protected_entries[].path' "$RUNNER_TEMP/bolt-v2-trust-root-policy.json" | while read -r relative_path; do
  target_path="$repo_root/$relative_path"
  mkdir -p "$(dirname "$target_path")"
  file_url="https://raw.githubusercontent.com/${HEAD_REPO_FULL_NAME}/${HEAD_REF}/${relative_path}"
  curl --retry 3 --retry-all-errors -fsSLo "$target_path" "$file_url"
done
printf '%s\n' "$repo_root" > "$RUNNER_TEMP/bolt-v2-trust-root-path""#;

    assert!(
        materialization_mentions_head_ref(head_ref_materialization_run),
        "materialization must reject raw URLs that use HEAD_REF instead of the exact head SHA"
    );
}

#[test]
fn trust_root_workflow_file_is_part_of_external_snapshot_policy() {
    let Some(root) = external_claude_config_root() else {
        return;
    };
    let policy = fs::read_to_string(root.join("config/bolt-v2-trust-root-policy.json"))
        .expect("external trust-root policy should read");
    let parsed: JsonValue =
        serde_json::from_str(&policy).expect("external trust-root policy should parse");
    let protected_entries = parsed
        .get("protected_entries")
        .and_then(JsonValue::as_array)
        .expect("external trust-root policy should declare protected_entries");
    let forbidden_entries = parsed
        .get("forbidden_entries")
        .and_then(JsonValue::as_array)
        .expect("external trust-root policy should declare forbidden_entries");

    for required_path in [
        ".github/workflows/nt-pointer-trust-root.yml",
        ".github/workflows/ci.yml",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "src/lib.rs",
        "src/nt_pointer_probe/mod.rs",
        "tests/nt_pointer_probe_control_plane.rs",
    ] {
        assert!(
            protected_entries.iter().any(|entry| {
                entry.get("path").and_then(JsonValue::as_str) == Some(required_path)
            }),
            "external snapshot policy must protect {required_path}"
        );
    }

    for forbidden_path in [
        "build.rs",
        ".cargo/config",
        ".cargo/config.toml",
        ".cargo/config.d",
    ] {
        assert!(
            forbidden_entries.iter().any(|entry| {
                entry.get("path").and_then(JsonValue::as_str) == Some(forbidden_path)
            }),
            "external snapshot policy must forbid {forbidden_path}"
        );
    }
}

#[test]
fn ci_lint_workflow_covers_all_nt_pointer_workflows() {
    let justfile = fs::read_to_string(repo_root().join("justfile")).expect("justfile should load");

    for workflow in [
        ".github/workflows/nt-pointer-control-plane.yml",
        ".github/workflows/nt-pointer-trust-root.yml",
        ".github/workflows/nt-pointer-probe-self-test.yml",
        ".github/workflows/nt-pointer-branch-governance-drift.yml",
    ] {
        assert!(
            justfile.contains(workflow),
            "ci-lint-workflow must include {workflow}"
        );
    }
}

#[test]
fn nt_pin_guard_workflows_route_base_ref_through_environment() {
    for workflow_path in [
        ".github/workflows/nt-pointer-control-plane.yml",
        ".github/workflows/dependabot-auto-merge.yml",
    ] {
        let workflow =
            fs::read_to_string(repo_root().join(workflow_path)).expect("workflow should load");
        assert!(
            !workflow.contains("${{ github.event.pull_request.base.ref }}\""),
            "{workflow_path} must not interpolate base.ref directly in shell"
        );
        assert!(
            workflow.contains("BASE_REF: ${{ github.event.pull_request.base.ref }}"),
            "{workflow_path} must pass base.ref through env"
        );
        assert!(
            workflow.contains("PR_NUMBER: ${{ github.event.pull_request.number }}"),
            "{workflow_path} must pass PR number through env"
        );
        assert!(
            workflow.contains("bash scripts/nt_pin_block_guard.sh \"$GITHUB_WORKSPACE\" \"$BASE_REF\" \"$PR_NUMBER\""),
            "{workflow_path} must invoke nt_pin_block_guard with environment-routed values"
        );
    }
}

#[test]
fn self_test_workflow_is_always_present_but_runtime_gated() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/nt-pointer-probe-self-test.yml"))
            .expect("self-test workflow should load");
    let yaml: YamlValue =
        serde_yaml::from_str(&workflow).expect("self-test workflow should parse as YAML");

    let triggers = yaml
        .get(YamlValue::String("on".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("self-test workflow should declare triggers");
    let pull_request = triggers
        .get(YamlValue::String("pull_request".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("self-test workflow must trigger on pull_request");
    assert!(
        !pull_request.contains_key(YamlValue::String("paths".to_string())),
        "required self-test check must not use trigger-level paths filters"
    );

    let concurrency = yaml
        .get(YamlValue::String("concurrency".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("self-test workflow should declare concurrency");
    let cancel_in_progress = concurrency
        .get(YamlValue::String("cancel-in-progress".to_string()))
        .expect("self-test workflow should declare cancel-in-progress");
    assert_eq!(
        cancel_in_progress,
        &YamlValue::String("${{ github.event_name == 'pull_request' }}".to_string()),
        "self-test workflow must only cancel superseded pull_request runs so push attestations on main complete"
    );
    let env = yaml
        .get(YamlValue::String("env".to_string()))
        .and_then(YamlValue::as_mapping)
        .expect("self-test workflow should declare top-level env");
    let cargo_build_jobs = env
        .get(YamlValue::String("CARGO_BUILD_JOBS".to_string()))
        .expect("self-test workflow should cap Cargo build parallelism");
    assert_eq!(
        cargo_build_jobs,
        &YamlValue::String("1".to_string()),
        "self-test workflow must cap Cargo build jobs to reduce runner linker pressure"
    );

    for required_path in [
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        ".cargo/config",
        ".cargo/config.toml",
        ".cargo/config.d/*",
        ".github/dependabot.yml",
        ".github/workflows/dependabot-auto-merge.yml",
        "tests/fixtures/nt_pointer_probe/*",
    ] {
        assert!(
            workflow.contains(required_path),
            "self-test workflow scope must include {required_path}"
        );
    }
    assert!(
        workflow.contains("Reject unexpected build-time injection surfaces"),
        "self-test workflow must fail closed on unexpected build-time injection surfaces before running cargo"
    );
    assert!(
        workflow.contains("just nt-pointer-probe-forbid-build-injection-surfaces"),
        "self-test workflow must invoke the protected build-injection guard recipe"
    );

    assert!(
        workflow.contains("Determine NT pointer self-test scope"),
        "self-test workflow must determine relevance at runtime"
    );
    assert!(
        workflow.contains("Skip NT pointer probe self-tests"),
        "self-test workflow must emit a fast-pass path for irrelevant diffs"
    );
    assert!(
        workflow.contains("if: steps.scope.outputs.run == 'true'"),
        "self-test workflow must run heavy tests only when relevant"
    );
}

#[test]
fn control_plane_workflow_rejects_unexpected_build_injection_surfaces() {
    let workflow =
        fs::read_to_string(repo_root().join(".github/workflows/nt-pointer-control-plane.yml"))
            .expect("control-plane workflow should load");
    assert!(
        workflow.contains("Reject unexpected build-time injection surfaces"),
        "control-plane workflow must fail closed on unexpected build-time injection surfaces before cargo"
    );
    assert!(
        workflow.contains("just nt-pointer-probe-forbid-build-injection-surfaces"),
        "control-plane workflow must invoke the protected build-injection guard recipe"
    );
}

#[test]
fn nt_pointer_probe_binary_exits_without_drop_unwind_on_control_plane_errors() {
    let binary = fs::read_to_string(repo_root().join("src/bin/nt_pointer_probe.rs"))
        .expect("nt_pointer_probe binary should load");
    let control = fs::read_to_string(repo_root().join("src/nt_pointer_probe/control.rs"))
        .expect("nt_pointer_probe control plane should load");

    let install_abort = binary
        .find("install_abort_on_panic();")
        .expect("nt_pointer_probe binary must install abort-on-panic handling");
    let parse = binary
        .find("Cli::parse()")
        .expect("nt_pointer_probe binary must parse CLI args");

    assert!(
        binary.contains("std::process::exit(1)"),
        "nt_pointer_probe binary must terminate fail-closed without unwinding through destructors"
    );
    assert!(
        install_abort < parse,
        "nt_pointer_probe binary must install abort-on-panic handling before CLI parsing"
    );
    assert!(
        !binary.contains("LoadedControlPlane::load_from_repo_root(&repo_root)?"),
        "nt_pointer_probe binary must not use ? when constructing LoadedControlPlane"
    );
    assert!(
        !binary.contains("loaded.ensure_no_nt_mutation_from_git_refs(&base_ref, &head_ref)?"),
        "nt_pointer_probe binary must not use ? on the NT mutation guard path"
    );
    assert!(
        !control.contains("loaded.validate()?"),
        "LoadedControlPlane::load_from_repo_root must not use ? after constructing LoadedControlPlane"
    );
    assert!(
        control.contains("std::mem::forget(loaded);"),
        "LoadedControlPlane::load_from_repo_root must avoid dropping a rejected LoadedControlPlane"
    );
    assert!(
        !control.contains("control.validate()?;"),
        "LoadedControlPlane::load_from_repo_root must not use ? after constructing ControlConfig"
    );
    assert!(
        !control.contains("expected.validate()?"),
        "ExpectedBranchProtection::load_and_validate must not use ? after constructing ExpectedBranchProtection"
    );
    assert!(
        control.contains("ManuallyDrop::new(load_toml(&control_path)?)"),
        "LoadedControlPlane::load_from_repo_root must protect the constructed control config before later fallible steps"
    );
}

#[test]
fn branch_protection_comparison_forgets_normalized_state_before_drift_return() {
    let control = fs::read_to_string(repo_root().join("src/nt_pointer_probe/control.rs"))
        .expect("nt_pointer_probe control plane should load");
    let compare_start = control
        .find("pub fn compare_branch_protection_response(")
        .expect("branch protection comparison function should exist");
    let compare_end = control[compare_start..]
        .find("pub fn compare_branch_governance_responses(")
        .map(|offset| compare_start + offset)
        .expect("branch governance comparison function should follow branch protection comparison");
    let compare_fn = &control[compare_start..compare_end];

    assert!(
        !compare_fn.contains("ensure!("),
        "compare_branch_protection_response must not use ensure! while NormalizedBranchProtection locals are live"
    );
    assert!(
        compare_fn.contains("std::mem::forget(actual);"),
        "compare_branch_protection_response must forget the normalized actual state before returning detected drift"
    );
    assert!(
        compare_fn.contains("std::mem::forget(expected_normalized);"),
        "compare_branch_protection_response must forget the normalized expected state before returning detected drift"
    );

    let success_tail_start = compare_fn
        .rfind("fail_branch_protection_drift!(")
        .expect("branch protection comparison should end with a drift guard");
    let success_tail = &compare_fn[success_tail_start..];
    let forget_actual = success_tail
        .find("std::mem::forget(actual);")
        .expect("compare_branch_protection_response must forget the normalized actual state before returning success");
    let forget_expected = success_tail
        .find("std::mem::forget(expected_normalized);")
        .expect("compare_branch_protection_response must forget the normalized expected state before returning success");
    let ok_return = success_tail
        .find("Ok(())")
        .expect("compare_branch_protection_response must end in Ok(())");

    assert!(
        forget_actual < ok_return,
        "compare_branch_protection_response must forget the normalized actual state before returning success"
    );
    assert!(
        forget_expected < ok_return,
        "compare_branch_protection_response must forget the normalized expected state before returning success"
    );
}

#[test]
fn current_external_snapshot_validator_matches_local_checkout() {
    let Some(root) = external_claude_config_root() else {
        return;
    };

    let output = std::process::Command::new("python3")
        .arg(root.join("lib/bolt_trust_root_validator.py"))
        .arg("--repo")
        .arg(repo_root())
        .arg("--policy")
        .arg(root.join("config/bolt-v2-trust-root-policy.json"))
        .output()
        .expect("external validator should run");

    assert!(
        output.status.success(),
        "external validator failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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
fn nt_mutation_checker_rejects_nt_lockfile_change() {
    let tempdir = init_temp_git_repo();

    fs::write(
        tempdir.path().join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "nautilus-common"
version = "0.1.0"
source = "git+https://github.com/nautechsystems/nautilus_trader.git?rev=deadbeef#deadbeef"
dependencies = ["serde"]
"#,
    )
    .expect("mutated Cargo.lock should write");

    let status = std::process::Command::new("git")
        .args(["add", "Cargo.lock"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--no-verify", "-m", "lockfile override"])
        .current_dir(tempdir.path())
        .status()
        .expect("git commit should run");
    assert!(status.success(), "git commit should succeed");

    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("temp git repo control plane should load");

    let err = loaded
        .ensure_no_nt_mutation_from_git_refs("HEAD~1", "HEAD")
        .expect_err("NT lockfile mutation should fail closed");

    assert!(
        err.to_string()
            .contains("Cargo.lock changed NT lock records"),
        "unexpected error: {err}"
    );
}

#[test]
fn nt_mutation_checker_ignores_non_nt_lockfile_change() {
    let tempdir = init_temp_git_repo();

    fs::write(
        tempdir.path().join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "serde"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    )
    .expect("non-NT Cargo.lock should write");

    let status = std::process::Command::new("git")
        .args(["add", "Cargo.lock"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--no-verify", "-m", "lockfile metadata"])
        .current_dir(tempdir.path())
        .status()
        .expect("git commit should run");
    assert!(status.success(), "git commit should succeed");

    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("temp git repo control plane should load");

    loaded
        .ensure_no_nt_mutation_from_git_refs("HEAD~1", "HEAD")
        .expect("non-NT lockfile changes should not trip the guard");
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
fn nt_mutation_checker_rejects_cargo_env_override_change() {
    let tempdir = init_temp_git_repo();
    fs::create_dir_all(tempdir.path().join(".cargo")).expect(".cargo dir should create");
    fs::write(
        tempdir.path().join(".cargo/config.toml"),
        r#"[paths]
search = []
"#,
    )
    .expect("base cargo config should write");

    let status = std::process::Command::new("git")
        .args(["add", ".cargo/config.toml"])
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
        tempdir.path().join(".cargo/config.toml"),
        r#"[paths]
search = []

[env]
RUSTC_WRAPPER = "tests/malicious_rustc.sh"
"#,
    )
    .expect("mutated cargo config should write");
    let status = std::process::Command::new("git")
        .args(["add", ".cargo/config.toml"])
        .current_dir(tempdir.path())
        .status()
        .expect("git add should run");
    assert!(status.success(), "git add should succeed");
    let status = std::process::Command::new("git")
        .args(["commit", "--no-verify", "-m", "cargo env override"])
        .current_dir(tempdir.path())
        .status()
        .expect("git commit should run");
    assert!(status.success(), "git commit should succeed");

    let loaded = LoadedControlPlane::load_from_repo_root(tempdir.path())
        .expect("temp git repo control plane should load");
    let err = loaded
        .ensure_no_nt_mutation_from_git_refs("HEAD~1", "HEAD")
        .expect_err("cargo env override should fail closed");

    assert!(
        err.to_string()
            .contains(".cargo/config.toml changed guarded cargo config state"),
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
  "nautilus-persistence-macros",
  "nautilus-polymarket",
  "nautilus-serialization",
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
trust_root = "nt-pointer-trust-root"
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
      - dependency-name: "nautilus-data"
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
      - dependency-name: "nautilus-persistence-macros"
      - dependency-name: "nautilus-polymarket"
      - dependency-name: "nautilus-serialization"
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
required_approving_review_count = 0
strict_required_status_checks = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-trust-root",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]
required_status_check_app_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368, nt-pointer-trust-root = 15368, nt-pointer-control-plane = 15368, nt-pointer-probe-self-test = 15368 }

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
do_not_enforce_on_create = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
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
        )
            || err.to_string().contains(
                "required_effective_rules pull_request required_approving_review_count 1 must match classic required_approving_review_count 0"
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
required_approving_review_count = 0
strict_required_status_checks = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
  "nt-pointer-trust-root",
  "nt-pointer-control-plane",
  "nt-pointer-probe-self-test",
]
required_status_check_app_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368, nt-pointer-trust-root = 15368, nt-pointer-control-plane = 15368, nt-pointer-probe-self-test = 15368 }

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
do_not_enforce_on_create = false
required_status_checks = [
  "gate",
  "clippy",
  "test",
  "build",
]
required_status_check_integration_ids = { gate = 15368, clippy = 15368, test = 15368, build = 15368 }

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
      "required_approving_review_count": 0,
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
        { "context": "build", "integration_id": 99999 }
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
fn branch_governance_comparison_rejects_do_not_enforce_on_create_drift() {
    let expected =
        ExpectedBranchProtection::load_and_validate(&fixture("branch_protection/expected.toml"))
            .expect("expected branch protection fixture should parse");
    let actual_protection =
        std::fs::read_to_string(fixture("branch_protection/matching_actual.json"))
            .expect("matching branch protection fixture should load");
    let actual_rules = std::fs::read_to_string(fixture("branch_protection/matching_rules.json"))
        .expect("matching rules fixture should load")
        .replace(
            "do_not_enforce_on_create\": false",
            "do_not_enforce_on_create\": true",
        );
    let actual_rulesets =
        std::fs::read_to_string(fixture("branch_protection/matching_rulesets.json"))
            .expect("matching ruleset details fixture should load");

    let err = compare_branch_governance_responses(
        &expected,
        &actual_protection,
        &actual_rules,
        &actual_rulesets,
    )
    .expect_err("do_not_enforce_on_create drift should fail governance comparison");

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
            .contains("branch protection drift: required_approving_review_count expected 0, got 2"),
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
    let fetch_run = fetch
        .get("run")
        .and_then(YamlValue::as_str)
        .expect("fetch step should define a shell script");
    assert!(
        fetch_run.contains("fetch_json()"),
        "drift fetch step must centralize API fetch failure handling"
    );
    assert!(
        !fetch_run.contains("&& [ ! -s "),
        "drift fetch step must fail closed on any gh api failure, not only empty-body failures"
    );
    assert!(
        fetch_run.contains("xargs -r -I{} gh api \"repos/${GITHUB_REPOSITORY}/rulesets/{}\""),
        "drift fetch step must fail closed if any per-ruleset fetch fails"
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
