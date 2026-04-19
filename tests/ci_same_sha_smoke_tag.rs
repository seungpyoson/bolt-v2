use std::{fs, path::PathBuf};

use serde_yaml::{Mapping, Value as YamlValue};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ci_workflow() -> String {
    fs::read_to_string(repo_root().join(".github/workflows/ci.yml"))
        .expect("ci workflow should load")
}

fn ci_yaml() -> YamlValue {
    serde_yaml::from_str(&ci_workflow()).expect("ci workflow should parse as YAML")
}

fn yaml_key(name: &str) -> YamlValue {
    YamlValue::String(name.to_string())
}

fn ci_jobs() -> serde_yaml::Mapping {
    ci_yaml()
        .get(yaml_key("jobs"))
        .and_then(YamlValue::as_mapping)
        .cloned()
        .expect("ci workflow should declare jobs")
}

fn ci_job(name: &str) -> Mapping {
    ci_jobs()
        .get(yaml_key(name))
        .and_then(YamlValue::as_mapping)
        .cloned()
        .unwrap_or_else(|| panic!("ci workflow must define a {name} job"))
}

fn job_if(job: &Mapping, job_name: &str) -> String {
    job.get(yaml_key("if"))
        .and_then(YamlValue::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| panic!("{job_name} job must declare an if guard"))
}

fn job_needs(job: &Mapping, job_name: &str) -> Vec<String> {
    match job
        .get(yaml_key("needs"))
        .unwrap_or_else(|| panic!("{job_name} job must declare needs"))
    {
        YamlValue::String(value) => vec![value.clone()],
        YamlValue::Sequence(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("{job_name} job needs entries must be strings"))
                    .to_string()
            })
            .collect(),
        _ => panic!("{job_name} job needs must be a string or sequence"),
    }
}

fn job_outputs(job: &Mapping, job_name: &str) -> Mapping {
    job.get(yaml_key("outputs"))
        .and_then(YamlValue::as_mapping)
        .cloned()
        .unwrap_or_else(|| panic!("{job_name} job must declare outputs"))
}

fn job_steps(job: &Mapping, job_name: &str) -> Vec<Mapping> {
    job.get(yaml_key("steps"))
        .and_then(YamlValue::as_sequence)
        .unwrap_or_else(|| panic!("{job_name} job must declare steps"))
        .iter()
        .map(|step| {
            step.as_mapping()
                .cloned()
                .unwrap_or_else(|| panic!("{job_name} job steps must be mappings"))
        })
        .collect()
}

fn step_field<'a>(step: &'a Mapping, field: &str) -> Option<&'a str> {
    step.get(yaml_key(field)).and_then(YamlValue::as_str)
}

fn step_with_field<'a>(step: &'a Mapping, field: &str, step_name: &str) -> &'a Mapping {
    step.get(yaml_key("with"))
        .and_then(YamlValue::as_mapping)
        .unwrap_or_else(|| panic!("{step_name} step must declare {field} in with"))
}

fn run_step(job: &Mapping, job_name: &str, step_name: &str) -> String {
    job_steps(job, job_name)
        .into_iter()
        .find(|step| step_field(step, "name") == Some(step_name))
        .and_then(|step| step_field(&step, "run").map(ToOwned::to_owned))
        .unwrap_or_else(|| panic!("{job_name} job must define a {step_name} run step"))
}

#[test]
fn ci_workflow_defines_same_sha_proof_job_without_skipping_non_tag_runs() {
    let jobs = ci_jobs();
    assert!(
        jobs.contains_key(yaml_key("same_sha_proof")),
        "ci workflow must define a same_sha_proof job for exact same-SHA smoke-tag proof reuse"
    );

    let same_sha_proof = ci_job("same_sha_proof");
    assert!(
        !same_sha_proof.contains_key(yaml_key("if")),
        "same_sha_proof must not skip the whole job on non-tag events; it should succeed with reuse_available=false so downstream PR/main lanes still run"
    );

    let outputs = job_outputs(&same_sha_proof, "same_sha_proof");
    for output in ["reuse_available", "source_run_id"] {
        assert!(
            outputs.contains_key(yaml_key(output)),
            "same_sha_proof must expose {output} so downstream tag lanes can reason about reuse"
        );
    }

    let resolve_run = run_step(
        &same_sha_proof,
        "same_sha_proof",
        "Resolve same-SHA proof record",
    );
    assert!(
        resolve_run.contains("if [[ \"$GITHUB_REF\" != refs/tags/v* ]]")
            || resolve_run.contains("if [[ \"$GITHUB_REF\" != refs/tags/v"),
        "same_sha_proof must explicitly fast-pass non-tag events inside the resolve step"
    );
}

#[test]
fn same_sha_proof_job_selects_exact_successful_main_push_run_for_same_sha() {
    let workflow = ci_workflow();
    assert!(
        workflow.contains(
            "actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=${GITHUB_SHA}"
        ),
        "same_sha_proof must query main-push CI runs for the exact GITHUB_SHA"
    );
    assert!(
        workflow.contains("&status=success") || workflow.contains(".conclusion == \"success\""),
        "same_sha_proof must restrict reuse to successful source runs"
    );
    assert!(
        workflow.contains("expected exactly one eligible main-push CI run"),
        "same_sha_proof must fail closed unless there is exactly one eligible successful main-push CI run for the SHA"
    );
}

#[test]
fn same_sha_proof_job_downloads_and_verifies_reused_artifact() {
    let same_sha_proof = ci_job("same_sha_proof");
    let steps = job_steps(&same_sha_proof, "same_sha_proof");

    let download_step = steps
        .iter()
        .find_map(|step| {
            let run = step_field(step, "run")?;
            run.contains("gh run download \"$SOURCE_RUN_ID\" --name bolt-v2-binary")
                .then_some(run)
        })
        .expect(
            "same_sha_proof must download the exact proven artifact from the eligible source run",
        );

    let verify_step = steps
        .iter()
        .find_map(|step| {
            let run = step_field(step, "run")?;
            run.contains("sha256sum -c bolt-v2.sha256").then_some(run)
        })
        .expect("same_sha_proof must verify the reused artifact digest before trust");

    let upload_step = steps
        .iter()
        .find(|step| {
            step_field(step, "uses").is_some_and(|uses| uses.contains("actions/upload-artifact"))
        })
        .expect("same_sha_proof must restage the reused artifact into the current run for deploy");
    let upload_with = step_with_field(upload_step, "name", "same_sha_proof upload");

    assert!(
        download_step.contains("SOURCE_RUN_ID"),
        "same_sha_proof must bind artifact download to the selected source run id"
    );
    assert!(
        verify_step.contains("bolt-v2.sha256"),
        "same_sha_proof must verify the restaged bolt-v2 digest file"
    );
    assert_eq!(
        upload_with
            .get(yaml_key("name"))
            .and_then(YamlValue::as_str),
        Some("bolt-v2-binary"),
        "same_sha_proof must restage the reused artifact under the deploy contract name"
    );
}

#[test]
fn tag_fast_path_skips_duplicate_heavy_lanes_only_when_reuse_is_ready() {
    let test = ci_job("test");
    let build = ci_job("build");
    let gate = ci_job("gate");

    assert!(
        job_needs(&test, "test")
            .iter()
            .any(|need| need == "same_sha_proof"),
        "test must depend on same_sha_proof before deciding whether a duplicate tag lane can skip"
    );
    assert!(
        job_if(&test, "test").contains("needs.same_sha_proof.outputs.reuse_available"),
        "test must only skip on tag pushes when same_sha_proof reports reuse_available"
    );

    assert!(
        job_needs(&build, "build")
            .iter()
            .any(|need| need == "same_sha_proof"),
        "build must depend on same_sha_proof before deciding whether a duplicate tag lane can skip"
    );
    let build_if = job_if(&build, "build");
    assert!(
        build_if.contains("needs.detector.outputs.build_required"),
        "build must keep its existing build_required detector gate"
    );
    assert!(
        build_if.contains("needs.same_sha_proof.outputs.reuse_available"),
        "build must only skip duplicate tag work when same_sha_proof reports reuse_available"
    );

    assert!(
        job_needs(&gate, "gate")
            .iter()
            .any(|need| need == "same_sha_proof"),
        "gate must wait for same_sha_proof before deciding whether skipped heavy lanes are acceptable"
    );
    let gate_run = run_step(&gate, "gate", "Check required lanes");
    assert!(
        gate_run.contains("needs.same_sha_proof.result"),
        "gate must account for the same_sha_proof job result explicitly"
    );
    assert!(
        gate_run.contains("needs.same_sha_proof.outputs.reuse_available"),
        "gate must only accept skipped heavy lanes when same_sha_proof says reuse is ready"
    );
}

#[test]
fn deploy_keeps_tag_on_main_and_idempotency_guards() {
    let workflow = ci_workflow();
    let deploy = ci_job("deploy");
    let deploy_if = job_if(&deploy, "deploy");
    assert!(
        deploy_if.contains("always()"),
        "deploy must opt out of skip propagation so the reuse path can still run when build is skipped"
    );
    assert!(
        deploy_if.contains("needs.gate.result == 'success'"),
        "deploy must still require a successful gate result"
    );
    assert!(
        deploy_if.contains("needs.build.result == 'success' || needs.same_sha_proof.outputs.reuse_available == 'true'"),
        "deploy must require either a fresh build or an explicitly reusable same-SHA proof artifact"
    );
    let verify_tag_step = run_step(&deploy, "deploy", "Verify tag is on main");
    assert!(
        verify_tag_step.contains("git merge-base --is-ancestor \"$GITHUB_SHA\" origin/main"),
        "deploy must keep the existing tag-on-main guard"
    );
    let idempotency_step = run_step(&deploy, "deploy", "Check idempotency");
    assert!(
        idempotency_step.contains("aws s3 ls \"$S3_DEPLOY_PATH/$TAG/bolt-v2\" >/dev/null 2>&1"),
        "deploy must keep the existing idempotency guard before upload"
    );
    assert!(
        workflow.contains("echo \"skip=true\" >> \"$GITHUB_OUTPUT\""),
        "deploy must keep the idempotency output contract for already-published tags"
    );
}
