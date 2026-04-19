use std::{path::PathBuf, process::Command};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn validator_command() -> Command {
    if let Some(path) = option_env!("CARGO_BIN_EXE_process_validator") {
        Command::new(path)
    } else {
        let mut command = Command::new("cargo");
        command.args(["run", "--quiet", "--bin", "process_validator", "--"]);
        command
    }
}

fn run_validator(relative_dir: &str, stage: &str) -> std::process::Output {
    let delivery_dir = repo_root().join(relative_dir);
    let mut command = validator_command();
    command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(delivery_dir)
        .arg("--stage")
        .arg(stage);
    command.output().expect("validator command should execute")
}

fn combined_output(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn eth_anchor_fixture_blocks_with_semantic_and_evidence_failures() {
    let output = run_validator(
        "docs/mechanical-process-package/experiments/exp-eth-anchor-semantics",
        "review",
    );
    assert!(
        !output.status.success(),
        "eth anchor fixture must fail closed; output:\n{}",
        combined_output(&output)
    );
    let text = combined_output(&output);
    assert!(text.contains("STATUS: BLOCK"), "{text}");
    assert!(text.contains("KIND: semantic"), "{text}");
    assert!(text.contains("active.interval_open"), "{text}");
    assert!(text.contains("KIND: evidence"), "{text}");
    assert!(
        text.matches("STATUS: BLOCK").count() >= 3,
        "expected at least 3 blocking findings, got:\n{text}"
    );
}

#[test]
fn finding_canonicalization_fixture_passes() {
    let output = run_validator(
        "docs/mechanical-process-package/experiments/exp-finding-canonicalization",
        "review",
    );
    assert!(
        output.status.success(),
        "finding canonicalization fixture should pass; output:\n{}",
        combined_output(&output)
    );
    let text = combined_output(&output);
    assert!(text.contains("STATUS: PASS"), "{text}");
    assert!(text.contains("KIND: finding"), "{text}");
}

#[test]
fn proof_plan_fixture_passes_with_review_target_warning() {
    let output = run_validator(
        "docs/mechanical-process-package/experiments/exp-proof-plan-selector-path",
        "review",
    );
    assert!(
        output.status.success(),
        "proof-plan fixture should pass structurally; output:\n{}",
        combined_output(&output)
    );
    let text = combined_output(&output);
    assert!(text.contains("STATUS: PASS"), "{text}");
    assert!(text.contains("KIND: proof"), "{text}");
    assert!(text.contains("STATUS: WARN"), "{text}");
    assert!(text.contains("KIND: review_target"), "{text}");
}

#[test]
fn candidate_205_package_is_structurally_valid_at_proof_locked_stage() {
    let output = run_validator(
        "docs/mechanical-process-package/candidate-205-smoke-tag-ci",
        "proof_locked",
    );
    assert!(
        output.status.success(),
        "candidate #205 package should be structurally valid before implementation; output:\n{}",
        combined_output(&output)
    );
    let text = combined_output(&output);
    assert!(text.contains("STATUS: PASS"), "{text}");
    assert!(!text.contains("STATUS: BLOCK"), "{text}");
}
