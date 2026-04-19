use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use tempfile::tempdir;

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

fn copy_dir_all(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("destination dir should create");
    for entry in fs::read_dir(src).expect("source dir should read") {
        let entry = entry.expect("dir entry should read");
        let file_type = entry.file_type().expect("file type should read");
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &dst_path);
        } else {
            fs::copy(entry.path(), dst_path).expect("file should copy");
        }
    }
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
fn candidate_205_package_is_structurally_valid_at_review_stage() {
    let output = run_validator(
        "docs/mechanical-process-package/candidate-205-smoke-tag-ci",
        "review",
    );
    assert!(
        output.status.success(),
        "candidate #205 package should be structurally valid at review stage; output:\n{}",
        combined_output(&output)
    );
    let text = combined_output(&output);
    assert!(text.contains("STATUS: PASS"), "{text}");
    assert!(!text.contains("STATUS: BLOCK"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_execution_target_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let execution_target = dst.join("execution_target.toml");
    if execution_target.exists() {
        fs::remove_file(&execution_target).expect("execution_target should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without execution_target.toml; output:\n{text}"
    );
    assert!(text.contains("execution_target.toml"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_ci_surface_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let ci_surface = dst.join("ci_surface.toml");
    if ci_surface.exists() {
        fs::remove_file(&ci_surface).expect("ci_surface should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without ci_surface.toml; output:\n{text}"
    );
    assert!(text.contains("ci_surface.toml"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_claim_enforcement_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let claim_enforcement = dst.join("claim_enforcement.toml");
    if claim_enforcement.exists() {
        fs::remove_file(&claim_enforcement).expect("claim_enforcement should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without claim_enforcement.toml; output:\n{text}"
    );
    assert!(text.contains("claim_enforcement.toml"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_assumption_register_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let assumption_register = dst.join("assumption_register.toml");
    if assumption_register.exists() {
        fs::remove_file(&assumption_register).expect("assumption_register should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without assumption_register.toml when trust assumptions exist; output:\n{text}"
    );
    assert!(text.contains("assumption_register.toml"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_review_rounds_are_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let review_rounds = dst.join("review_rounds");
    if review_rounds.exists() {
        fs::remove_dir_all(&review_rounds).expect("review_rounds should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without review_rounds when external review evidence exists; output:\n{text}"
    );
    assert!(text.contains("review_rounds"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_stage_promotion_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let stage_promotion = dst.join("stage_promotion.toml");
    if stage_promotion.exists() {
        fs::remove_file(&stage_promotion).expect("stage_promotion should remove");
    }

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "review-stage package must fail closed without stage_promotion.toml; output:\n{text}"
    );
    assert!(text.contains("stage_promotion.toml"), "{text}");
}
