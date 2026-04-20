use std::{fs, path::PathBuf, process::Command};

use serde::Deserialize;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[derive(Debug, Deserialize)]
struct BenchmarkDescriptor {
    benchmark_id: String,
    fixture_ref: String,
    fixture_owner: String,
    runner_ref: String,
    runner_owner: String,
    subject_test_refs_used: bool,
}

fn load_descriptor(relative_path: &str) -> BenchmarkDescriptor {
    let path = repo_root().join(relative_path);
    let bytes = fs::read(&path).expect("descriptor should read");
    toml::from_slice(&bytes).expect("descriptor should parse")
}

fn scientific_validation_descriptor_paths() -> [&'static str; 4] {
    [
        "docs/mechanical-process-package/validation/benchmarks/B4-unsupported-comparator-kinds.toml",
        "docs/mechanical-process-package/validation/benchmarks/B5-scalar-summary-schema-breaks.toml",
        "docs/mechanical-process-package/validation/benchmarks/B6-summary-replay-drift.toml",
        "docs/mechanical-process-package/validation/benchmarks/B7-producer-contract-schema-breaks.toml",
    ]
}

#[test]
fn scientific_validation_descriptors_use_protocol_owned_fixture_and_runner_artifacts() {
    for relative_path in scientific_validation_descriptor_paths() {
        let descriptor = load_descriptor(relative_path);
        assert!(
            !descriptor.fixture_ref.is_empty(),
            "{} must declare fixture_ref",
            descriptor.benchmark_id
        );
        assert_eq!(
            descriptor.fixture_owner, "protocol_validation",
            "{} must be owned by the protocol fixture layer",
            descriptor.benchmark_id
        );
        assert!(
            !descriptor.runner_ref.is_empty(),
            "{} must declare runner_ref",
            descriptor.benchmark_id
        );
        assert_eq!(
            descriptor.runner_owner, "protocol_validation",
            "{} must be executed by the protocol runner layer",
            descriptor.benchmark_id
        );
        assert!(
            !descriptor.subject_test_refs_used,
            "{} must not reuse subject-authored tests as held-out evidence",
            descriptor.benchmark_id
        );
        assert!(
            repo_root().join(&descriptor.fixture_ref).exists(),
            "{} fixture_ref must point to a real protocol artifact",
            descriptor.benchmark_id
        );
        assert!(
            repo_root().join(&descriptor.runner_ref).exists(),
            "{} runner_ref must point to a real protocol artifact",
            descriptor.benchmark_id
        );
    }
}

#[test]
fn scientific_validation_runner_executes_b4_through_b7_descriptors() {
    for relative_path in scientific_validation_descriptor_paths() {
        let output = Command::new("cargo")
            .current_dir(repo_root())
            .args([
                "run",
                "--quiet",
                "--bin",
                "scientific_validation_runner",
                "--",
                "--descriptor",
                relative_path,
                "--subject-root",
                repo_root()
                    .to_str()
                    .expect("repo root should be valid utf-8"),
            ])
            .output()
            .expect("runner command should execute");

        assert!(
            output.status.success(),
            "scientific_validation_runner must pass {}.\nstdout:\n{}\nstderr:\n{}",
            relative_path,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
