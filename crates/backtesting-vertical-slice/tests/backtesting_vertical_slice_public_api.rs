//! Public API invariants for the backtesting vertical slice.

use std::{fs, path::Path, process::Command};

#[test]
fn accepted_dataset_cannot_be_constructed_outside_source_proof_gate() {
    assert_external_crate_rejected(
        r#"use backtesting_vertical_slice::source_proof::{
    AcceptedDataset, IngestManifestObjectRecord, SourceProofFidelityClass,
};

pub fn construct() -> AcceptedDataset {
    AcceptedDataset {
        source_proof_id: String::new(),
        source_proof_version: 1,
        source_binding: String::new(),
        venue: String::new(),
        product_family: String::new(),
        product_category: String::new(),
        instrument_universe_id: String::new(),
        fidelity_class: SourceProofFidelityClass::TradeReplay,
        forbidden_claims: Vec::new(),
        object: IngestManifestObjectRecord {
            s3_uri: String::new(),
            source_url: String::new(),
            sha256: String::new(),
            bytes: 1,
            archive_date: String::new(),
            schema_columns: Vec::new(),
        },
    }
}
"#,
        "external AcceptedDataset struct literal unexpectedly compiled",
    );
}

#[test]
fn accepted_dataset_cannot_be_mutated_outside_source_proof_gate() {
    assert_external_crate_rejected(
        r#"use backtesting_vertical_slice::source_proof::AcceptedDataset;

pub fn mutate(mut accepted: AcceptedDataset) {
    accepted.source_proof_id = "forged".to_string();
}
"#,
        "external AcceptedDataset field mutation unexpectedly compiled",
    );
}

fn assert_external_crate_rejected(source: &str, success_message: &str) {
    let crate_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let temp = tempfile::TempDir::new().expect("temp dir");
    fs::create_dir_all(temp.path().join("src")).expect("create src");
    fs::write(
        temp.path().join("Cargo.toml"),
        format!(
            r#"[package]
name = "accepted-dataset-construction-probe"
version = "0.0.0"
edition = "2024"

[dependencies]
backtesting-vertical-slice = {{ path = "{}" }}
"#,
            crate_dir.replace('\\', "\\\\")
        ),
    )
    .expect("write Cargo.toml");
    fs::write(temp.path().join("src/lib.rs"), source).expect("write lib.rs");
    // Seed the probe with the crate's lockfile and resolve offline so the
    // probe compiles the exact dependency tree the crate itself pins.
    // Without this the probe re-resolves against live registry state, and an
    // incompatible upstream release fails compilation before the gate under
    // test is ever reached.
    fs::copy(
        Path::new(&crate_dir).join("Cargo.lock"),
        temp.path().join("Cargo.lock"),
    )
    .expect("copy Cargo.lock");

    let output = Command::new(env!("CARGO"))
        .arg("check")
        .arg("--quiet")
        .arg("--offline")
        .current_dir(temp.path())
        .output()
        .expect("run cargo check");

    assert!(!output.status.success(), "{success_message}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("private") || stderr.contains("cannot construct"),
        "unexpected compile failure:\n{stderr}"
    );
}
