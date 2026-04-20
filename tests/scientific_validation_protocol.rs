use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;
use toml::Value;

#[derive(Deserialize)]
struct BenchmarkManifest {
    benchmarks: Vec<BenchmarkRow>,
}

#[derive(Deserialize)]
struct BenchmarkRow {
    benchmark_id: String,
    track: String,
    descriptor_ref: Option<String>,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scientific_benchmark_descriptors() -> BTreeMap<String, PathBuf> {
    let manifest_path = repo_root()
        .join("docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml");
    let manifest: BenchmarkManifest = toml::from_str(
        &fs::read_to_string(&manifest_path).expect("benchmark manifest should read"),
    )
    .expect("benchmark manifest should parse");

    let mut descriptors = BTreeMap::new();
    for benchmark_id in ["B4", "B5", "B6", "B7"] {
        let row = manifest
            .benchmarks
            .iter()
            .find(|row| row.benchmark_id == benchmark_id && row.track == "scientific_validation")
            .unwrap_or_else(|| {
                panic!(
                    "benchmark manifest should register {benchmark_id} under scientific_validation"
                )
            });
        let descriptor_ref = row
            .descriptor_ref
            .as_deref()
            .unwrap_or_else(|| panic!("{benchmark_id} should declare descriptor_ref"));
        descriptors.insert(benchmark_id.to_string(), repo_root().join(descriptor_ref));
    }

    descriptors
}

fn parse_descriptor(path: &Path) -> Value {
    toml::from_str(&fs::read_to_string(path).expect("descriptor should read"))
        .expect("descriptor should parse")
}

fn required_string_field<'a>(descriptor: &'a Value, field: &str) -> &'a str {
    descriptor
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| panic!("descriptor should include nonempty {field}"))
}

#[test]
fn scientific_benchmarks_require_protocol_owned_fixture_and_runner_refs() {
    for (benchmark_id, descriptor_path) in scientific_benchmark_descriptors() {
        let descriptor = parse_descriptor(&descriptor_path);
        let legacy_helper_ref = descriptor.get("base_fixture").and_then(Value::as_str);

        assert!(
            legacy_helper_ref.is_none(),
            "{benchmark_id} still points at subject-owned test helper via base_fixture={legacy_helper_ref:?}; expected protocol-owned fixture_ref/runner_ref fields in {}",
            descriptor_path.display()
        );

        let fixture_ref = required_string_field(&descriptor, "fixture_ref");
        let runner_ref = required_string_field(&descriptor, "runner_ref");
        let fixture_owner = required_string_field(&descriptor, "fixture_owner");
        let runner_owner = required_string_field(&descriptor, "runner_owner");
        let subject_test_refs_used = descriptor
            .get("subject_test_refs_used")
            .and_then(Value::as_bool);

        assert_eq!(
            fixture_owner, "protocol_validation",
            "{benchmark_id} fixture_owner must stay protocol-owned"
        );
        assert_eq!(
            runner_owner, "protocol_validation",
            "{benchmark_id} runner_owner must stay protocol-owned"
        );
        assert_eq!(
            subject_test_refs_used,
            Some(false),
            "{benchmark_id} must record that subject-authored test refs are not used"
        );
        assert!(
            fixture_ref != "tests/delivery_validator_cli.rs::write_minimal_review_package",
            "{benchmark_id} fixture_ref must not reuse the subject-owned review-package helper"
        );
        assert!(
            runner_ref != "tests/delivery_validator_cli.rs::write_minimal_review_package",
            "{benchmark_id} runner_ref must not reuse the subject-owned review-package helper"
        );
    }
}
