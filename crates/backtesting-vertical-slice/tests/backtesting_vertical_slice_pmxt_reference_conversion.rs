use backtesting_vertical_slice::conversion_boundary::{
    ConversionCatalogMetadata, ConversionCheckpoint, ConversionManifest,
};
use nautilus_model::data::{OrderBookDelta, TradeTick};
use nautilus_persistence::backend::catalog::ParquetDataCatalog;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

const PMXT_REFERENCE_ROOT: &str = "../../specs/023-nt-research-analytics-platform/reference/pmxt-polymarket-selected-source-conversion";
const INSTRUMENT_ID: &str = "0x92889d49761307073d461289d01208c3b19292d17da937c0f57501c7b7efa50d-73895424095742155573626958367283533358717984717096075221396743226794070701077.POLYMARKET";
const SELECTED_SOURCE_SHA256: &str =
    "0102068effdcdbb308d9390746afa6a75dfda1b3ba8fc3239ecdb4c74d9ae99e";
const CATALOG_HASH: &str = "3a26bebf03e4a2c4eef1bd344a8b1c6f1b78ef7d3c7f43d6279ac9d029fab236";

#[test]
fn pmxt_selected_source_conversion_reference_artifact_is_usable_nt_catalog() {
    let root = reference_root();
    let catalog_root = root.join("backtests/pmxt-run/nt-catalog");
    let selected_source = root.join("selected-source/selected-source.parquet");
    let conversion_manifest_path = root.join("backtests/pmxt-run/conversion-manifest.json");
    let conversion_checkpoint_path = root.join("backtests/pmxt-run/conversion-checkpoint.json");
    let catalog_metadata_path = root.join("backtests/pmxt-run/catalog-metadata.json");
    let result_contract_path = root.join("backtests/pmxt-run/backtest-result-contract.json");

    assert!(
        root.join("artifact-root-run.toml").exists(),
        "PMXT conversion run spec must be committed"
    );
    assert!(
        root.join("manifest.toml").exists(),
        "PMXT conversion manifest spec must be committed"
    );
    assert!(
        root.join("metadata/gamma-markets.json").exists(),
        "PMXT Gamma metadata must be committed"
    );
    assert!(
        selected_source.exists(),
        "PMXT selected source parquet must be committed"
    );
    assert!(
        root.join("selected-source/selected-source-report.json")
            .exists(),
        "PMXT selected source report must be committed"
    );
    assert!(
        root.join("selector-gamma-candidates/first-proof-selector-report.json")
            .exists(),
        "PMXT selector report must be committed"
    );
    assert!(
        conversion_manifest_path.exists(),
        "PMXT conversion manifest must be committed"
    );
    assert!(
        catalog_metadata_path.exists(),
        "PMXT catalog metadata must be committed"
    );
    assert!(
        result_contract_path.exists(),
        "PMXT result contract must be committed"
    );
    assert_eq!(sha256_file(&selected_source), SELECTED_SOURCE_SHA256);

    let conversion_manifest = read_json(&conversion_manifest_path);
    assert_eq!(
        conversion_manifest["manifest_version"],
        "conversion-manifest.v1"
    );
    assert_eq!(conversion_manifest["nt_data_type"], "OrderBookDelta");
    assert_eq!(conversion_manifest["nt_instrument_id"], INSTRUMENT_ID);
    assert_eq!(conversion_manifest["canonical_rows"], 103);
    assert_eq!(
        conversion_manifest["catalog_rows_by_nt_data_type"]["OrderBookDelta"],
        103
    );
    assert_eq!(
        conversion_manifest["catalog_rows_by_nt_data_type"]["TradeTick"],
        1
    );
    assert_eq!(conversion_manifest["catalog_hash"], CATALOG_HASH);

    let catalog_metadata = read_json(&catalog_metadata_path);
    assert_eq!(catalog_metadata["metadata_version"], "catalog-metadata.v1");
    assert_eq!(catalog_metadata["catalog_hash"], CATALOG_HASH);
    assert_eq!(
        catalog_metadata["catalog_rows_by_nt_data_type"]["OrderBookDelta"],
        103
    );
    assert_eq!(
        catalog_metadata["catalog_rows_by_nt_data_type"]["TradeTick"],
        1
    );

    let result_contract = read_json(&result_contract_path);
    assert_eq!(
        result_contract["source_proof_id"],
        "source-proof-polymarket-pmxt-v2-orderbook-binary-option-pending-2026-06-08"
    );
    assert_eq!(
        result_contract["accepted_object_sha256"],
        SELECTED_SOURCE_SHA256
    );
    assert_eq!(result_contract["nt_result"]["iterations"], 104);

    // These bytes are an immutable pre-boundary reference catalog, not a
    // reusable current conversion. Current readers must reject all three
    // legacy control artifacts rather than silently granting v4 identity.
    assert!(
        serde_json::from_slice::<ConversionManifest>(
            &fs::read(&conversion_manifest_path).expect("read legacy conversion manifest")
        )
        .is_err()
    );
    assert!(
        serde_json::from_slice::<ConversionCheckpoint>(
            &fs::read(&conversion_checkpoint_path).expect("read legacy conversion checkpoint")
        )
        .is_err()
    );
    assert!(
        serde_json::from_slice::<ConversionCatalogMetadata>(
            &fs::read(&catalog_metadata_path).expect("read legacy catalog metadata")
        )
        .is_err()
    );

    let mut catalog = ParquetDataCatalog::new(&catalog_root, None, None, None, None);
    let order_book_deltas: Vec<OrderBookDelta> = catalog
        .query_typed_data::<OrderBookDelta>(
            Some(vec![INSTRUMENT_ID.to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .expect("read committed PMXT OrderBookDelta catalog");
    let trades: Vec<TradeTick> = catalog
        .query_typed_data::<TradeTick>(
            Some(vec![INSTRUMENT_ID.to_string()]),
            None,
            None,
            None,
            None,
            false,
        )
        .expect("read committed PMXT TradeTick catalog");

    assert_eq!(order_book_deltas.len(), 103);
    assert_eq!(trades.len(), 1);
}

fn reference_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PMXT_REFERENCE_ROOT)
}

fn read_json(path: &PathBuf) -> Value {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()))
}

fn sha256_file(path: &PathBuf) -> String {
    let bytes = fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
