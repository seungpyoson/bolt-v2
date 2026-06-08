use std::{fs::File, process::Command, sync::Arc};

use arrow::{
    array::{ArrayRef, StringArray},
    datatypes::{DataType, Field, Schema},
    record_batch::RecordBatch,
};
use backtesting_vertical_slice::{
    first_proof_selector::{
        FIRST_PROOF_SELECTOR_SCHEMA_VERSION, FirstProofSelection, FirstProofSelectorReport,
        FirstProofSelectorStatus, SelectedFirstProofAsset,
    },
    selected_source_slice::write_selected_source_slice_from_spec_file,
};
use parquet::{
    arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder},
    file::properties::WriterProperties,
};
use sha2::{Digest, Sha256};

#[test]
fn selected_source_slice_writes_only_selector_assets_and_configured_columns() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let selector_path = dir.path().join("selector.json");
    let output_path = dir.path().join("selected.parquet");
    let report_path = dir.path().join("selected-report.json");
    let spec_path = dir.path().join("selected.toml");
    write_source_parquet(&source_path);
    let selector_bytes = write_selector_report(&selector_path);
    write_spec(
        &spec_path,
        &source_path,
        &selector_path,
        &output_path,
        &report_path,
    );

    let artifact = write_selected_source_slice_from_spec_file(&spec_path).expect("source slice");

    assert_eq!(artifact.output_parquet_path, output_path);
    assert_eq!(artifact.source_rows, 4);
    assert_eq!(artifact.source_row_groups, 2);
    assert_eq!(artifact.projected_row_groups, 1);
    assert_eq!(artifact.selected_rows, 2);
    assert_eq!(artifact.selected_asset_count, 1);
    assert_eq!(artifact.selected_asset_ids_hash, "selected-assets-hash");
    assert!(!artifact.output_parquet_sha256.is_empty());
    assert!(!artifact.report_hash.is_empty());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).expect("read report"))
            .expect("parse report");
    assert_eq!(
        report["source_parquet_sha256"],
        sha256_bytes(&std::fs::read(&source_path).expect("read source parquet"))
    );
    assert_eq!(
        report["selector_report_sha256"],
        sha256_bytes(&selector_bytes)
    );
    assert_eq!(report["usage_scope"], "one_off_backfill_data");
    assert_eq!(report["source_row_groups"], 2);
    assert_eq!(report["projected_row_groups"], 1);

    let (columns, rows) = read_selected_rows(&artifact.output_parquet_path);
    assert_eq!(columns, vec!["asset", "event_type", "payload"]);
    assert_eq!(
        rows,
        vec![
            vec!["asset-a", "book", "payload-a-book"],
            vec!["asset-a", "price_change", "payload-a-price"],
        ]
    );

    let second = write_selected_source_slice_from_spec_file(&spec_path).expect("idempotent rerun");
    assert_eq!(second.output_parquet_sha256, artifact.output_parquet_sha256);
    assert_eq!(second.report_hash, artifact.report_hash);

    std::fs::write(&output_path, b"dirty").expect("dirty selected output");
    let err = write_selected_source_slice_from_spec_file(&spec_path)
        .expect_err("dirty output must be rejected");
    assert!(
        err.to_string().contains("dirty selected source artifact"),
        "{err}"
    );
}

#[test]
fn selected_source_slice_cli_writes_artifact_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let selector_path = dir.path().join("selector.json");
    let output_path = dir.path().join("selected.parquet");
    let report_path = dir.path().join("selected-report.json");
    let spec_path = dir.path().join("selected.toml");
    write_source_parquet(&source_path);
    write_selector_report(&selector_path);
    write_spec(
        &spec_path,
        &source_path,
        &selector_path,
        &output_path,
        &report_path,
    );

    let binary =
        std::env::var("CARGO_BIN_EXE_selected_source_slice").expect("selected_source_slice binary");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run selected-source-slice CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("selected_source_parquet = "), "{stdout}");
    assert!(stdout.contains("selected_rows = 2"), "{stdout}");
    assert!(stdout.contains("selected_asset_count = 1"), "{stdout}");
    assert!(report_path.exists());
    assert!(output_path.exists());
}

#[test]
fn selected_source_slice_uses_selector_row_groups_without_full_asset_rescan() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let selector_path = dir.path().join("selector.json");
    let output_path = dir.path().join("selected.parquet");
    let report_path = dir.path().join("selected-report.json");
    let spec_path = dir.path().join("selected.toml");
    write_source_parquet_with_unscannable_non_selected_row_group(&source_path);
    write_selector_report_with_source_row_groups(&selector_path, &[0]);
    write_spec(
        &spec_path,
        &source_path,
        &selector_path,
        &output_path,
        &report_path,
    );

    let artifact = write_selected_source_slice_from_spec_file(&spec_path)
        .expect("source slice uses selector row groups");

    assert_eq!(artifact.source_rows, 4);
    assert_eq!(artifact.source_row_groups, 2);
    assert_eq!(artifact.projected_row_groups, 1);
    assert_eq!(artifact.selected_rows, 2);
    let (_, rows) = read_selected_rows(&artifact.output_parquet_path);
    assert_eq!(
        rows,
        vec![
            vec!["asset-a", "book", "payload-a-book"],
            vec!["asset-a", "price_change", "payload-a-price"],
        ]
    );
}

fn write_selector_report(path: &std::path::Path) -> Vec<u8> {
    let selector_bytes = serde_json::to_vec_pretty(&FirstProofSelectorReport {
        schema_version: FIRST_PROOF_SELECTOR_SCHEMA_VERSION.to_string(),
        selector_id: "selector-synthetic".to_string(),
        status: FirstProofSelectorStatus::Selected,
        selection: FirstProofSelection {
            required_event_families: vec!["book".to_string()],
            excluded_event_families: vec!["tick_size_change".to_string()],
            candidate_asset_ids: Vec::new(),
            row_budget: 10,
            max_selected_assets: 1,
        },
        event_count_ledger_hash: "event-ledger-hash".to_string(),
        total_assets: 2,
        eligible_assets: 1,
        selected_assets: vec![SelectedFirstProofAsset {
            asset_id: "asset-a".to_string(),
            replay_rows: 2,
            source_row_groups: vec![1],
        }],
        selected_asset_ids_hash: "selected-assets-hash".to_string(),
        excluded_event_asset_count: 0,
        excluded_event_row_count: 0,
        blocking_issues: vec![],
    })
    .expect("selector json");
    std::fs::write(path, &selector_bytes).expect("write selector");
    selector_bytes
}

fn write_selector_report_with_source_row_groups(path: &std::path::Path, row_groups: &[u64]) {
    let selector = serde_json::json!({
        "schema_version": FIRST_PROOF_SELECTOR_SCHEMA_VERSION,
        "selector_id": "selector-synthetic",
        "status": "selected",
        "selection": {
            "required_event_families": ["book"],
            "excluded_event_families": ["tick_size_change"],
            "row_budget": 10,
            "max_selected_assets": 1
        },
        "event_count_ledger_hash": "event-ledger-hash",
        "total_assets": 2,
        "eligible_assets": 1,
        "selected_assets": [{
            "asset_id": "asset-a",
            "replay_rows": 2,
            "source_row_groups": row_groups
        }],
        "selected_asset_ids_hash": "selected-assets-hash",
        "excluded_event_asset_count": 0,
        "excluded_event_row_count": 0,
        "blocking_issues": []
    });
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&selector).expect("selector json"),
    )
    .expect("write selector");
}

fn write_spec(
    spec_path: &std::path::Path,
    source_path: &std::path::Path,
    selector_path: &std::path::Path,
    output_path: &std::path::Path,
    report_path: &std::path::Path,
) {
    std::fs::write(
        spec_path,
        format!(
            r#"source_parquet_path = "{}"
selector_report_path = "{}"
output_parquet_path = "{}"
report_path = "{}"
asset_id_column = "asset"
usage_scope = "one_off_backfill_data"
projected_columns = ["asset", "event_type", "payload"]
"#,
            source_path.display(),
            selector_path.display(),
            output_path.display(),
            report_path.display()
        ),
    )
    .expect("write spec");
}

fn write_source_parquet(path: &std::path::Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("asset", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, false),
        Field::new("ignored", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![
                "asset-b", "asset-b", "asset-a", "asset-a",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "book",
                "price_change",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "payload-b-book",
                "payload-b-price",
                "payload-a-book",
                "payload-a-price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "ignored-b1",
                "ignored-b2",
                "ignored-a1",
                "ignored-a2",
            ])) as ArrayRef,
        ],
    )
    .expect("record batch");
    let file = File::create(path).expect("create source parquet");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(2))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("parquet writer");
    writer.write(&batch).expect("write batch");
    writer.close().expect("close parquet");
}

fn write_source_parquet_with_unscannable_non_selected_row_group(path: &std::path::Path) {
    let schema = Arc::new(Schema::new(vec![
        Field::new("asset", DataType::Utf8, true),
        Field::new("event_type", DataType::Utf8, false),
        Field::new("payload", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![
                Some("asset-a"),
                Some("asset-a"),
                None,
                Some("asset-b"),
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "price_change",
                "book",
                "price_change",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "payload-a-book",
                "payload-a-price",
                "payload-null-book",
                "payload-b-price",
            ])) as ArrayRef,
        ],
    )
    .expect("record batch");
    let file = File::create(path).expect("create source parquet");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(2))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("parquet writer");
    writer.write(&batch).expect("write batch");
    writer.close().expect("close parquet");
}

fn read_selected_rows(path: &std::path::Path) -> (Vec<String>, Vec<Vec<String>>) {
    let file = File::open(path).expect("open selected parquet");
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .expect("reader")
        .build()
        .expect("build reader");
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch.expect("batch");
        if columns.is_empty() {
            columns = batch
                .schema()
                .fields()
                .iter()
                .map(|field| field.name().clone())
                .collect();
        }
        let asset = batch
            .column_by_name("asset")
            .expect("asset")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("asset strings");
        let event_type = batch
            .column_by_name("event_type")
            .expect("event_type")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("event strings");
        let payload = batch
            .column_by_name("payload")
            .expect("payload")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("payload strings");
        for row in 0..batch.num_rows() {
            rows.push(vec![
                asset.value(row).to_string(),
                event_type.value(row).to_string(),
                payload.value(row).to_string(),
            ]);
        }
    }
    (columns, rows)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
