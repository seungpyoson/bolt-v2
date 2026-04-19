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

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent dir should create");
    }
    fs::write(path, contents).expect("file should write");
}

fn write_minimal_review_package(dst: &Path) {
    write_file(
        &dst.join("issue_contract.toml"),
        r#"
issue_id = 999
title = "synthetic promotion gate package"
repo = "seungpyoson/bolt-v2"
slice_id = "synthetic-gate"
status = "frozen"
problem_statement = "freeze one synthetic review-stage package"
required_outcomes = ["one stage gate only"]
non_goals = ["real issue delivery"]
allowed_surfaces = ["docs/mechanical-process-package/**"]
forbidden_surfaces = ["src/**"]
assumptions = []
semantic_terms = ["synthetic_term"]
"#,
    );
    write_file(
        &dst.join("seam_contract.toml"),
        r#"
status = "locked"

[[seams]]
seam_id = "SEAM-SYN-1"
semantic_term = "synthetic_term"
writer_path = "docs/synthetic"
writer_symbol = "writer"
reader_path = "docs/synthetic"
reader_symbol = "reader"
storage_field = "synthetic.field"
authoritative_source = "review_target.toml"
allowed_sources = ["review_target.toml"]
forbidden_sources = []
fallback_order = []
freshness_clock = "N/A"
status = "frozen"
"#,
    );
    write_file(
        &dst.join("proof_plan.toml"),
        r#"
[[claims]]
claim_id = "CSYN-1"
falsified_by = ["FXSYN-1"]
required_before = "implementation"
"#,
    );
    write_file(
        &dst.join("finding_ledger.toml"),
        "status = \"classified\"\n",
    );
    write_file(&dst.join("evidence_bundle.toml"), "");
    write_file(
        &dst.join("merge_claims.toml"),
        r#"
merge_ready = false
open_blockers = []
required_evidence = []
"#,
    );
    write_file(
        &dst.join("review_target.toml"),
        r#"
repo = "seungpyoson/bolt-v2"
pr_number = 999
base_ref = "main"
head_sha = "abc123"
diff_identity = "synthetic-gate"
round_id = "review-r1"
status = "frozen"
"#,
    );
    write_file(
        &dst.join("execution_target.toml"),
        r#"
repo = "seungpyoson/bolt-v2"
branch = "synthetic-branch"
base_ref = "main"
head_sha = "abc123"
diff_identity = "synthetic-gate"
changed_paths = ["docs/mechanical-process-package/**"]
status = "frozen"
"#,
    );
    write_file(
        &dst.join("ci_surface.toml"),
        r#"
workflow = "synthetic"
head_sha = "abc123"
run_selection_rule = "synthetic"

[required_jobs_by_stage]
review = ["job-review"]
"#,
    );
    write_file(
        &dst.join("claim_enforcement.toml"),
        r#"
[[rows]]
claim_id = "CSYN-1"
enforcement_kind = "synthetic"
enforced_at = "synthetic"
test_ref = ""
ci_ref = ""
evidence_required = []
status = "bound"
"#,
    );
    write_file(
        &dst.join("stage_promotion.toml"),
        r#"
[[promotions]]
from_stage = "proof_locked"
to_stage = "review"
promotion_gate_artifact = "promotion_gate.toml"
status = "satisfied"
"#,
    );
    write_file(
        &dst.join("promotion_gate.toml"),
        r#"
[[gates]]
gate_id = "gate-1"
from_stage = "proof_locked"
to_stage = "review"
comparator_kind = "all_of"
left_ref = ""
right_ref = ""
right_literal = ""
verdict = "pass"
status = "frozen"

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#repo"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#branch"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#base_ref"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#head_sha"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#diff_identity"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#changed_paths"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#status"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "execution_target.toml#head_sha"
right_ref = "review_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#workflow"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#head_sha"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#run_selection_rule"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "ci_surface.toml#head_sha"
right_ref = "execution_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#required_jobs_by_stage.review"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "review_rounds/review-r1.toml#absorbed_by_head"
right_ref = "review_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "review_rounds/review-r1.toml#round_id"
right_ref = "review_target.toml#round_id"
right_literal = ""
"#,
    );
    write_file(
        &dst.join("orchestration_reachability.toml"),
        r#"
[[cases]]
case_id = "reach-1"
subject = "synthetic"
trigger_job = "job-review"
trigger_result = "success"
required_reachable_jobs = ["job-review"]
forbidden_job_results = ["failure"]
proof_ref = "gate-1"
status = "covered"
"#,
    );
    write_file(
        &dst.join("review_rounds/review-r1.toml"),
        r#"
round_id = "review-r1"
source = "synthetic"
review_target_ref = "synthetic"
raw_comment_refs = ["comment-1"]
ingested_findings = []
stale_findings = []
wrong_target_findings = []
absorbed_by_head = "abc123"
status = "ingested"
"#,
    );
}

fn write_stage_gate_files(
    dst: &Path,
    from_stage: &str,
    to_stage: &str,
    left_ref: &str,
    right_ref: &str,
    right_literal: &str,
) {
    write_file(
        &dst.join("stage_promotion.toml"),
        &format!(
            r#"
[[promotions]]
from_stage = "{from_stage}"
to_stage = "{to_stage}"
promotion_gate_artifact = "promotion_gate.toml"
status = "satisfied"
"#
        ),
    );
    write_file(
        &dst.join("promotion_gate.toml"),
        &format!(
            r#"
[[gates]]
gate_id = "gate-1"
from_stage = "{from_stage}"
to_stage = "{to_stage}"
comparator_kind = "string_eq"
left_ref = "{left_ref}"
right_ref = "{right_ref}"
right_literal = "{right_literal}"
verdict = "pass"
status = "frozen"
"#
        ),
    );
}

fn write_review_ci_gate(dst: &Path) {
    write_file(
        &dst.join("stage_promotion.toml"),
        r#"
[[promotions]]
from_stage = "proof_locked"
to_stage = "review"
promotion_gate_artifact = "promotion_gate.toml"
status = "satisfied"
"#,
    );
    write_file(
        &dst.join("promotion_gate.toml"),
        r#"
[[gates]]
gate_id = "gate-1"
from_stage = "proof_locked"
to_stage = "review"
comparator_kind = "all_of"
left_ref = ""
right_ref = ""
right_literal = ""
verdict = "pass"
status = "frozen"

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "execution_target.toml#head_sha"
right_ref = "review_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "ci_surface.toml#head_sha"
right_ref = "execution_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#required_jobs_by_stage.review"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "review_rounds/review-r1.toml#absorbed_by_head"
right_ref = "review_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "review_rounds/review-r1.toml#round_id"
right_ref = "review_target.toml#round_id"
right_literal = ""
"#,
    );
}

fn write_merge_candidate_ci_gate(dst: &Path) {
    write_file(
        &dst.join("stage_promotion.toml"),
        r#"
[[promotions]]
from_stage = "review"
to_stage = "merge_candidate"
promotion_gate_artifact = "promotion_gate.toml"
status = "satisfied"
"#,
    );
    write_file(
        &dst.join("promotion_gate.toml"),
        r#"
[[gates]]
gate_id = "gate-1"
from_stage = "review"
to_stage = "merge_candidate"
comparator_kind = "all_of"
left_ref = ""
right_ref = ""
right_literal = ""
verdict = "pass"
status = "frozen"

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#repo"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#branch"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#base_ref"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#head_sha"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#diff_identity"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#changed_paths"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "execution_target.toml#status"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "scalar_eq"
left_ref = "merge_claims.toml#merge_ready"
right_ref = ""
right_literal = "true"

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#workflow"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#head_sha"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#run_selection_rule"
right_ref = ""
right_literal = ""

[[gates.clauses]]
comparator_kind = "string_eq"
left_ref = "ci_surface.toml#head_sha"
right_ref = "execution_target.toml#head_sha"
right_literal = ""

[[gates.clauses]]
comparator_kind = "nonempty"
left_ref = "ci_surface.toml#required_jobs_by_stage.merge_candidate"
right_ref = ""
right_literal = ""
"#,
    );
}

fn write_minimal_stage_package(dst: &Path, stage: &str) {
    write_file(
        &dst.join("issue_contract.toml"),
        r#"
issue_id = 999
title = "synthetic stage package"
repo = "seungpyoson/bolt-v2"
slice_id = "synthetic-stage"
status = "frozen"
problem_statement = "synthetic package"
required_outcomes = ["one stage gate only"]
non_goals = ["real issue delivery"]
allowed_surfaces = ["docs/mechanical-process-package/**"]
forbidden_surfaces = ["src/**"]
assumptions = []
semantic_terms = ["synthetic_term"]
"#,
    );
    write_file(
        &dst.join("finding_ledger.toml"),
        "status = \"classified\"\n",
    );
    write_file(&dst.join("evidence_bundle.toml"), "");
    write_file(
        &dst.join("merge_claims.toml"),
        r#"
merge_ready = false
open_blockers = []
required_evidence = []
"#,
    );

    match stage {
        "intake" => {
            write_stage_gate_files(
                dst,
                "none",
                "intake",
                "issue_contract.toml#problem_statement",
                "",
                "synthetic package",
            );
        }
        "seam_locked" => {
            write_file(
                &dst.join("seam_contract.toml"),
                r#"
status = "locked"

[[seams]]
seam_id = "SEAM-SYN-1"
semantic_term = "synthetic_term"
writer_path = "docs/synthetic"
writer_symbol = "writer"
reader_path = "docs/synthetic"
reader_symbol = "reader"
storage_field = "synthetic.field"
authoritative_source = "issue_contract.toml"
allowed_sources = ["issue_contract.toml"]
forbidden_sources = []
fallback_order = []
freshness_clock = "N/A"
status = "frozen"
"#,
            );
            write_stage_gate_files(
                dst,
                "intake",
                "seam_locked",
                "seam_contract.toml#status",
                "",
                "locked",
            );
        }
        "proof_locked" => {
            write_minimal_stage_package(dst, "seam_locked");
            write_file(
                &dst.join("proof_plan.toml"),
                r#"
status = "locked"

[[claims]]
claim_id = "CSYN-1"
falsified_by = ["FXSYN-1"]
required_before = "implementation"
"#,
            );
            write_stage_gate_files(
                dst,
                "seam_locked",
                "proof_locked",
                "proof_plan.toml#status",
                "",
                "locked",
            );
        }
        "review" => {
            write_minimal_stage_package(dst, "proof_locked");
            write_file(
                &dst.join("review_target.toml"),
                r#"
repo = "seungpyoson/bolt-v2"
pr_number = 999
base_ref = "main"
head_sha = "abc123"
diff_identity = "synthetic-gate"
round_id = "review-r1"
status = "frozen"
"#,
            );
            write_file(
                &dst.join("execution_target.toml"),
                r#"
repo = "seungpyoson/bolt-v2"
branch = "synthetic-branch"
base_ref = "main"
head_sha = "abc123"
diff_identity = "synthetic-gate"
changed_paths = ["docs/mechanical-process-package/**"]
status = "frozen"
"#,
            );
            write_file(
                &dst.join("ci_surface.toml"),
                r#"
workflow = "synthetic"
head_sha = "abc123"
run_selection_rule = "synthetic"

[required_jobs_by_stage]
review = ["job-review"]
"#,
            );
            write_file(
                &dst.join("claim_enforcement.toml"),
                r#"
[[rows]]
claim_id = "CSYN-1"
enforcement_kind = "synthetic"
enforced_at = "synthetic"
test_ref = ""
ci_ref = ""
evidence_required = []
status = "bound"
"#,
            );
            write_file(
                &dst.join("orchestration_reachability.toml"),
                r#"
[[cases]]
case_id = "reach-1"
subject = "synthetic"
trigger_job = "job-review"
trigger_result = "success"
required_reachable_jobs = ["job-review"]
forbidden_job_results = ["failure"]
proof_ref = "gate-1"
status = "covered"
"#,
            );
            write_file(
                &dst.join("review_rounds/review-r1.toml"),
                r#"
round_id = "review-r1"
source = "synthetic"
review_target_ref = "synthetic"
raw_comment_refs = ["comment-1"]
ingested_findings = []
stale_findings = []
wrong_target_findings = []
absorbed_by_head = "abc123"
status = "ingested"
"#,
            );
            write_review_ci_gate(dst);
        }
        "merge_candidate" => {
            write_minimal_stage_package(dst, "review");
            write_file(
                &dst.join("merge_claims.toml"),
                r#"
merge_ready = true
open_blockers = []
required_evidence = []
"#,
            );
            write_file(
                &dst.join("ci_surface.toml"),
                r#"
workflow = "synthetic"
head_sha = "abc123"
run_selection_rule = "synthetic"

[required_jobs_by_stage]
review = ["job-review"]
merge_candidate = ["job-merge"]
"#,
            );
            write_file(
                &dst.join("orchestration_reachability.toml"),
                r#"
[[cases]]
case_id = "reach-1"
subject = "synthetic"
trigger_job = "job-merge"
trigger_result = "success"
required_reachable_jobs = ["job-merge"]
forbidden_job_results = ["failure"]
proof_ref = "gate-1"
status = "covered"
"#,
            );
            write_merge_candidate_ci_gate(dst);
        }
        other => panic!("unsupported synthetic stage {other}"),
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
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root()
        .join("docs/mechanical-process-package/experiments/exp-finding-canonicalization");
    let dst = temp.path().join("exp-finding-canonicalization");
    copy_dir_all(&src, &dst);
    write_stage_gate_files(
        &dst,
        "proof_locked",
        "review",
        "review_target.toml#head_sha",
        "",
        "23659d3a5a45681abaee4a0afe20d79ea4455183",
    );

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
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
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root()
        .join("docs/mechanical-process-package/experiments/exp-proof-plan-selector-path");
    let dst = temp.path().join("exp-proof-plan-selector-path");
    copy_dir_all(&src, &dst);
    write_stage_gate_files(
        &dst,
        "proof_locked",
        "review",
        "issue_contract.toml#problem_statement",
        "",
        "Selector-path review history produced late blocker classes around schema-boundary behavior, legacy compatibility, and unbounded slug fan-out. The experiment tests whether those classes can be forced into explicit proof obligations before review.",
    );

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("review")
        .output()
        .expect("validator command should execute");
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
fn synthetic_stage_packages_pass_for_all_stages() {
    for stage in [
        "intake",
        "seam_locked",
        "proof_locked",
        "review",
        "merge_candidate",
    ] {
        let temp = tempdir().expect("tempdir should create");
        let dst = temp.path().join(format!("synthetic-{stage}"));
        write_minimal_stage_package(&dst, stage);

        let mut command = validator_command();
        let output = command
            .current_dir(repo_root())
            .arg("--delivery-dir")
            .arg(&dst)
            .arg("--stage")
            .arg(stage)
            .output()
            .expect("validator command should execute");
        let text = combined_output(&output);
        assert!(
            output.status.success(),
            "synthetic {stage} package with one valid promotion gate should pass; output:\n{text}"
        );
        assert!(text.contains("STATUS: PASS"), "{text}");
    }
}

#[test]
fn synthetic_stage_packages_block_when_promotion_gate_is_missing_for_all_stages() {
    for stage in [
        "intake",
        "seam_locked",
        "proof_locked",
        "review",
        "merge_candidate",
    ] {
        let temp = tempdir().expect("tempdir should create");
        let dst = temp.path().join(format!("synthetic-{stage}"));
        write_minimal_stage_package(&dst, stage);
        fs::remove_file(dst.join("promotion_gate.toml")).expect("promotion_gate should remove");

        let mut command = validator_command();
        let output = command
            .current_dir(repo_root())
            .arg("--delivery-dir")
            .arg(&dst)
            .arg("--stage")
            .arg(stage)
            .output()
            .expect("validator command should execute");
        let text = combined_output(&output);
        assert!(
            !output.status.success(),
            "synthetic {stage} package must fail closed when promotion_gate.toml is missing; output:\n{text}"
        );
        assert!(text.contains("promotion_gate.toml"), "{text}");
    }
}

#[test]
fn synthetic_stage_packages_block_when_promotion_gate_has_multiple_gates_for_all_stages() {
    for stage in [
        "intake",
        "seam_locked",
        "proof_locked",
        "review",
        "merge_candidate",
    ] {
        let temp = tempdir().expect("tempdir should create");
        let dst = temp.path().join(format!("synthetic-{stage}"));
        write_minimal_stage_package(&dst, stage);
        let gate_path = dst.join("promotion_gate.toml");
        let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
        fs::write(&gate_path, format!("{original}\n{original}"))
            .expect("duplicated promotion_gate should write");

        let mut command = validator_command();
        let output = command
            .current_dir(repo_root())
            .arg("--delivery-dir")
            .arg(&dst)
            .arg("--stage")
            .arg(stage)
            .output()
            .expect("validator command should execute");
        let text = combined_output(&output);
        assert!(
            !output.status.success(),
            "synthetic {stage} package must fail closed when promotion_gate.toml defines multiple gates; output:\n{text}"
        );
        assert!(text.contains("multiple gates"), "{text}");
    }
}

#[test]
fn synthetic_stage_packages_block_when_promotion_gate_stage_binding_is_wrong_for_all_stages() {
    for (stage, wrong_to_stage) in [
        ("intake", "review"),
        ("seam_locked", "review"),
        ("proof_locked", "review"),
        ("review", "merge_candidate"),
        ("merge_candidate", "review"),
    ] {
        let temp = tempdir().expect("tempdir should create");
        let dst = temp.path().join(format!("synthetic-{stage}"));
        write_minimal_stage_package(&dst, stage);
        let gate_path = dst.join("promotion_gate.toml");
        let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
        fs::write(
            &gate_path,
            original.replace(
                &format!("to_stage = \"{stage}\""),
                &format!("to_stage = \"{wrong_to_stage}\""),
            ),
        )
        .expect("mutated promotion_gate should write");

        let mut command = validator_command();
        let output = command
            .current_dir(repo_root())
            .arg("--delivery-dir")
            .arg(&dst)
            .arg("--stage")
            .arg(stage)
            .output()
            .expect("validator command should execute");
        let text = combined_output(&output);
        assert!(
            !output.status.success(),
            "synthetic {stage} package must fail closed when promotion gate stage binding is wrong; output:\n{text}"
        );
        assert!(text.contains("does not match stage transition"), "{text}");
    }
}

#[test]
fn synthetic_stage_packages_block_when_promotion_gate_verdict_is_not_pass_for_all_stages() {
    for stage in [
        "intake",
        "seam_locked",
        "proof_locked",
        "review",
        "merge_candidate",
    ] {
        let temp = tempdir().expect("tempdir should create");
        let dst = temp.path().join(format!("synthetic-{stage}"));
        write_minimal_stage_package(&dst, stage);
        let gate_path = dst.join("promotion_gate.toml");
        let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
        fs::write(
            &gate_path,
            original.replace("verdict = \"pass\"", "verdict = \"block\""),
        )
        .expect("mutated promotion_gate should write");

        let mut command = validator_command();
        let output = command
            .current_dir(repo_root())
            .arg("--delivery-dir")
            .arg(&dst)
            .arg("--stage")
            .arg(stage)
            .output()
            .expect("validator command should execute");
        let text = combined_output(&output);
        assert!(
            !output.status.success(),
            "synthetic {stage} package must fail closed when promotion gate verdict is not pass; output:\n{text}"
        );
        assert!(text.contains("verdict"), "{text}");
    }
}

#[test]
fn synthetic_merge_candidate_blocks_when_merge_ready_is_false_via_scalar_gate() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-merge-candidate");
    write_minimal_stage_package(&dst, "merge_candidate");
    write_file(
        &dst.join("merge_claims.toml"),
        r#"
merge_ready = false
open_blockers = []
required_evidence = []
"#,
    );

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("merge_candidate")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "merge_candidate must fail closed when merge_ready is false through the scalar gate; output:\n{text}"
    );
    assert!(text.contains("clause"), "{text}");
}

#[test]
fn synthetic_review_package_passes_with_single_promotion_gate() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);

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
        output.status.success(),
        "synthetic review package with one valid promotion gate should pass; output:\n{text}"
    );
    assert!(text.contains("STATUS: PASS"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    fs::remove_file(dst.join("promotion_gate.toml")).expect("promotion_gate should remove");

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
        "synthetic review package must fail closed when promotion_gate.toml is missing; output:\n{text}"
    );
    assert!(text.contains("promotion_gate.toml"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_has_zero_gates() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    fs::write(dst.join("promotion_gate.toml"), "gates = []\n")
        .expect("empty promotion_gate should write");

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
        "synthetic review package must fail closed when promotion_gate.toml contains zero gates; output:\n{text}"
    );
    assert!(text.contains("contains no gates"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_has_multiple_gates() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let gate_path = dst.join("promotion_gate.toml");
    let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
    fs::write(&gate_path, format!("{original}\n{original}"))
        .expect("duplicated promotion_gate should write");

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
        "synthetic review package must fail closed when promotion_gate.toml defines multiple gates; output:\n{text}"
    );
    assert!(text.contains("multiple gates"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_stage_binding_is_wrong() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let gate_path = dst.join("promotion_gate.toml");
    let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
    fs::write(
        &gate_path,
        original.replace("to_stage = \"review\"", "to_stage = \"merge_candidate\""),
    )
    .expect("mutated promotion_gate should write");

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
        "synthetic review package must fail closed when promotion gate stage binding is wrong; output:\n{text}"
    );
    assert!(text.contains("does not match stage transition"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_verdict_is_not_pass() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let gate_path = dst.join("promotion_gate.toml");
    let original = fs::read_to_string(&gate_path).expect("promotion_gate should read");
    fs::write(
        &gate_path,
        original.replace("verdict = \"pass\"", "verdict = \"block\""),
    )
    .expect("mutated promotion_gate should write");

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
        "synthetic review package must fail closed when promotion gate verdict is not pass; output:\n{text}"
    );
    assert!(text.contains("verdict"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_promotion_gate_subject_mismatches_expected() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let round_path = dst.join("review_rounds/review-r1.toml");
    let original = fs::read_to_string(&round_path).expect("review_round should read");
    fs::write(
        &round_path,
        original.replace(
            "absorbed_by_head = \"abc123\"",
            "absorbed_by_head = \"wrong-head\"",
        ),
    )
    .expect("mutated review_round should write");

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
        "synthetic review package must fail closed when promotion gate subject mismatches expected value; output:\n{text}"
    );
    assert!(text.contains("clause"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_execution_head_mismatches_review_head() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let execution_target = dst.join("execution_target.toml");
    let original = fs::read_to_string(&execution_target).expect("execution_target should read");
    fs::write(
        &execution_target,
        original.replace("head_sha = \"abc123\"", "head_sha = \"wrong-head\""),
    )
    .expect("mutated execution_target should write");

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
        "review package must fail closed when execution head mismatches review head through the all_of gate; output:\n{text}"
    );
    assert!(text.contains("clause"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_ci_surface_head_mismatches_execution_head() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let ci_surface = dst.join("ci_surface.toml");
    let original = fs::read_to_string(&ci_surface).expect("ci_surface should read");
    fs::write(
        &ci_surface,
        original.replace("head_sha = \"abc123\"", "head_sha = \"wrong-head\""),
    )
    .expect("mutated ci_surface should write");

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
        "review package must fail closed when ci_surface head mismatches execution head through the gate; output:\n{text}"
    );
    assert!(text.contains("clause"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_execution_target_repo_is_empty() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let execution_target = dst.join("execution_target.toml");
    let original = fs::read_to_string(&execution_target).expect("execution_target should read");
    fs::write(
        &execution_target,
        original.replace("repo = \"seungpyoson/bolt-v2\"", "repo = \"\""),
    )
    .expect("mutated execution_target should write");

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
        "review package must fail closed when execution_target.repo is empty through the gate; output:\n{text}"
    );
    assert!(text.contains("nonempty"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_ci_surface_workflow_is_empty() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let ci_surface = dst.join("ci_surface.toml");
    let original = fs::read_to_string(&ci_surface).expect("ci_surface should read");
    fs::write(
        &ci_surface,
        original.replace("workflow = \"synthetic\"", "workflow = \"\""),
    )
    .expect("mutated ci_surface should write");

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
        "review package must fail closed when ci_surface.workflow is empty through the gate; output:\n{text}"
    );
    assert!(text.contains("nonempty"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_review_jobs_missing_from_ci_surface() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let ci_surface = dst.join("ci_surface.toml");
    let original = fs::read_to_string(&ci_surface).expect("ci_surface should read");
    fs::write(
        &ci_surface,
        original.replace("review = [\"job-review\"]", "review = []"),
    )
    .expect("mutated ci_surface should write");

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
        "review package must fail closed when review jobs are missing from ci_surface through the gate; output:\n{text}"
    );
    assert!(text.contains("nonempty"), "{text}");
}

#[test]
fn synthetic_review_package_blocks_when_review_round_id_mismatches_review_target() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-review-package");
    write_minimal_review_package(&dst);
    let round_path = dst.join("review_rounds/review-r1.toml");
    let original = fs::read_to_string(&round_path).expect("review_round should read");
    fs::write(
        &round_path,
        original.replace("round_id = \"review-r1\"", "round_id = \"review-r2\""),
    )
    .expect("mutated review_round should write");

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
        "review package must fail closed when review round id mismatches review target through the all_of gate; output:\n{text}"
    );
    assert!(text.contains("clause"), "{text}");
}

#[test]
fn synthetic_merge_candidate_blocks_when_execution_target_repo_is_empty() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-merge-candidate");
    write_minimal_stage_package(&dst, "merge_candidate");
    let execution_target = dst.join("execution_target.toml");
    let original = fs::read_to_string(&execution_target).expect("execution_target should read");
    fs::write(
        &execution_target,
        original.replace("repo = \"seungpyoson/bolt-v2\"", "repo = \"\""),
    )
    .expect("mutated execution_target should write");

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("merge_candidate")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "merge_candidate must fail closed when execution_target.repo is empty through the gate; output:\n{text}"
    );
    assert!(text.contains("nonempty"), "{text}");
}

#[test]
fn synthetic_merge_candidate_blocks_when_merge_jobs_missing_from_ci_surface() {
    let temp = tempdir().expect("tempdir should create");
    let dst = temp.path().join("synthetic-merge-candidate");
    write_minimal_stage_package(&dst, "merge_candidate");
    let ci_surface = dst.join("ci_surface.toml");
    let original = fs::read_to_string(&ci_surface).expect("ci_surface should read");
    fs::write(
        &ci_surface,
        original.replace("merge_candidate = [\"job-merge\"]", "merge_candidate = []"),
    )
    .expect("mutated ci_surface should write");

    let mut command = validator_command();
    let output = command
        .current_dir(repo_root())
        .arg("--delivery-dir")
        .arg(&dst)
        .arg("--stage")
        .arg("merge_candidate")
        .output()
        .expect("validator command should execute");
    let text = combined_output(&output);
    assert!(
        !output.status.success(),
        "merge_candidate must fail closed when merge jobs are missing from ci_surface through the gate; output:\n{text}"
    );
    assert!(text.contains("nonempty"), "{text}");
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

#[test]
fn review_stage_package_blocks_when_stage_promotion_has_multiple_rows_for_stage() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let stage_promotion = dst.join("stage_promotion.toml");
    let original = fs::read_to_string(&stage_promotion).expect("stage_promotion should read");
    fs::write(&stage_promotion, format!("{original}\n{original}"))
        .expect("duplicated stage_promotion should write");

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
        "review-stage package must fail closed when multiple promotion rows exist for the same stage; output:\n{text}"
    );
    assert!(text.contains("multiple promotion rows"), "{text}");
}

#[test]
fn review_stage_package_blocks_when_orchestration_reachability_is_missing() {
    let temp = tempdir().expect("tempdir should create");
    let src = repo_root().join("docs/mechanical-process-package/candidate-205-smoke-tag-ci");
    let dst = temp.path().join("candidate-205-smoke-tag-ci");
    copy_dir_all(&src, &dst);
    let reachability = dst.join("orchestration_reachability.toml");
    if reachability.exists() {
        fs::remove_file(&reachability).expect("orchestration_reachability should remove");
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
        "review-stage package must fail closed without orchestration_reachability.toml; output:\n{text}"
    );
    assert!(text.contains("orchestration_reachability.toml"), "{text}");
}
