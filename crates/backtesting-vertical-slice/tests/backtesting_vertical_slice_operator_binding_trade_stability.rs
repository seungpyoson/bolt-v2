//! Byte-stability regression for the operator-binding slice (the merge gate):
//! an existing trade run-spec still flows through the single-table trade entry
//! (dispatched by `run_operator_from_run_spec`), produces byte-identical
//! `conversion-manifest.json` / `catalog-metadata.json` / `result-contract.json`
//! across same-directory reruns, deterministic conversion artifacts across
//! fresh output directories, and NEVER writes `conversion-tables.json`.
//!
//! Follows the committed-reference pattern of
//! `backtesting_vertical_slice_backfill_gate_reference_artifacts`: the committed
//! trade run-spec is rebound to a locally reproducible synthetic object (the
//! real staged object is not committed), exactly as the operator unit fixtures
//! do.

use std::{fs, io::Write, path::PathBuf};

use backtesting_vertical_slice::{
    canonical_trades::{RawPayloadConfig, RawPayloadContainer},
    conversion_boundary::{
        CATALOG_METADATA_FILE, CONVERSION_MANIFEST_FILE, CONVERSION_TABLES_FILE,
    },
    hashing::sha256_hex,
    operator::{
        OperatorRunArtifacts, RESULT_CONTRACT_FILE, RunArtifacts, RunSpec,
        run_operator_from_run_spec,
    },
    result_contract::BacktestResultContract,
};
use flate2::{Compression, write::GzEncoder};

const COMMITTED_RUN_SPEC: &str = include_str!(
    "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
);

const SAMPLE_CSV: &str = "id,timestamp,price,volume,side,rpi\n\
    1,1772323201665,617.2,0.3,buy,0\n\
    2,1772323312219,617.9,0.1456,sell,0\n\
    3,1772323312236,617,0.1544,sell,0\n";

fn gzip(text: &str) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(text.as_bytes()).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

/// The committed run-spec, with the accepted-object hash rebound to a locally
/// reproducible synthetic object (the real staged object is not committed).
fn run_spec_for(gz_bytes: &[u8]) -> RunSpec {
    let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("committed run-spec parses");
    assert!(
        spec.source_proof.is_accepted(),
        "committed run-spec must carry an accepted source proof"
    );
    spec.source_bindings_path = PathBuf::from(
        "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml",
    );
    let object_hash = sha256_hex(gz_bytes);
    spec.accepted_object.sha256 = object_hash.clone();
    spec.accepted_object.bytes = gz_bytes.len() as u64;
    spec.source_proof.raw_sample_hash = object_hash;
    spec.converter.raw_payload = RawPayloadConfig {
        container: RawPayloadContainer::CsvGzip,
        max_object_bytes: gz_bytes.len() as u64,
        max_decoded_bytes: 4096,
        zip_member: None,
        max_member_bytes: None,
        member_suffix: None,
        jsonl_stream: None,
    };
    spec
}

fn run_trade(spec: &RunSpec, object_bytes: &[u8], dir: &std::path::Path) -> RunArtifacts {
    match run_operator_from_run_spec(spec, object_bytes, dir).expect("trade operator run") {
        OperatorRunArtifacts::Trade(artifacts) => *artifacts,
        OperatorRunArtifacts::MultiTable(_) => {
            panic!("CSV native-trades run-specs must dispatch through the trade entry")
        }
    }
}

fn read_artifact_bytes(dir: &std::path::Path, name: &str) -> Vec<u8> {
    fs::read(dir.join(name)).unwrap_or_else(|error| panic!("read artifact {name}: {error}"))
}

#[test]
fn trade_run_spec_artifacts_are_byte_stable_and_never_write_tables_index() {
    let gz = gzip(SAMPLE_CSV);
    let spec = run_spec_for(&gz);

    // Fresh run in directory A.
    let dir_a = tempfile::TempDir::new().expect("temp dir A");
    let first = run_trade(&spec, &gz, dir_a.path());
    assert_eq!(first.output.canonical_table.rows.len(), 3);
    assert!(
        !dir_a.path().join(CONVERSION_TABLES_FILE).exists(),
        "trade conversions must never write {CONVERSION_TABLES_FILE}"
    );
    let manifest_a = read_artifact_bytes(dir_a.path(), CONVERSION_MANIFEST_FILE);
    let metadata_a = read_artifact_bytes(dir_a.path(), CATALOG_METADATA_FILE);
    let contract_a = read_artifact_bytes(dir_a.path(), RESULT_CONTRACT_FILE);

    // Same-directory rerun reuses the completed output byte-identically.
    let second = run_trade(&spec, &gz, dir_a.path());
    assert_eq!(
        second.output.projection.catalog_hash,
        first.output.projection.catalog_hash
    );
    assert_eq!(
        read_artifact_bytes(dir_a.path(), CONVERSION_MANIFEST_FILE),
        manifest_a,
        "conversion-manifest.json must stay byte-identical across reruns"
    );
    assert_eq!(
        read_artifact_bytes(dir_a.path(), CATALOG_METADATA_FILE),
        metadata_a,
        "catalog-metadata.json must stay byte-identical across reruns"
    );
    assert_eq!(
        read_artifact_bytes(dir_a.path(), RESULT_CONTRACT_FILE),
        contract_a,
        "result contract must stay byte-identical across reruns"
    );
    assert!(
        !dir_a.path().join(CONVERSION_TABLES_FILE).exists(),
        "trade reruns must never write {CONVERSION_TABLES_FILE}"
    );

    // Fresh run in directory B: the conversion artifacts are deterministic
    // byte-for-byte; the result contract is stable up to the run-volatile
    // engine identity fields the completed-output verifier also normalizes.
    let dir_b = tempfile::TempDir::new().expect("temp dir B");
    let third = run_trade(&spec, &gz, dir_b.path());
    assert_eq!(
        read_artifact_bytes(dir_b.path(), CONVERSION_MANIFEST_FILE),
        manifest_a,
        "conversion-manifest.json must be deterministic across output directories"
    );
    assert_eq!(
        read_artifact_bytes(dir_b.path(), CATALOG_METADATA_FILE),
        metadata_a,
        "catalog-metadata.json must be deterministic across output directories"
    );
    assert!(
        !dir_b.path().join(CONVERSION_TABLES_FILE).exists(),
        "trade conversions must never write {CONVERSION_TABLES_FILE}"
    );
    let contract_first: BacktestResultContract =
        serde_json::from_slice(&contract_a).expect("contract A parses");
    let mut contract_third = third.output.contract.clone();
    contract_third.nt_result.machine_id = contract_first.nt_result.machine_id.clone();
    contract_third.nt_result.instance_id = contract_first.nt_result.instance_id.clone();
    contract_third.nt_result.elapsed_time_secs = contract_first.nt_result.elapsed_time_secs;
    assert_eq!(
        contract_third, contract_first,
        "result contract must be stable across output directories up to engine identity"
    );
}
