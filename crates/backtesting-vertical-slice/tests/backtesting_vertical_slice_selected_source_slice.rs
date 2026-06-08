use std::{fs::File, sync::Arc};

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
use parquet::arrow::{ArrowWriter, arrow_reader::ParquetRecordBatchReaderBuilder};

#[test]
fn selected_source_slice_writes_only_selector_assets_and_configured_columns() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let selector_path = dir.path().join("selector.json");
    let output_path = dir.path().join("selected.parquet");
    let report_path = dir.path().join("selected-report.json");
    let spec_path = dir.path().join("selected.toml");
    write_source_parquet(&source_path);
    std::fs::write(
        &selector_path,
        serde_json::to_vec_pretty(&FirstProofSelectorReport {
            schema_version: FIRST_PROOF_SELECTOR_SCHEMA_VERSION.to_string(),
            selector_id: "selector-synthetic".to_string(),
            status: FirstProofSelectorStatus::Selected,
            selection: FirstProofSelection {
                required_event_families: vec!["book".to_string()],
                excluded_event_families: vec!["tick_size_change".to_string()],
                row_budget: 10,
                max_selected_assets: 1,
            },
            event_count_ledger_hash: "event-ledger-hash".to_string(),
            total_assets: 2,
            eligible_assets: 1,
            selected_assets: vec![SelectedFirstProofAsset {
                asset_id: "asset-a".to_string(),
                replay_rows: 2,
            }],
            selected_asset_ids_hash: "selected-assets-hash".to_string(),
            excluded_event_asset_count: 0,
            excluded_event_row_count: 0,
            blocking_issues: vec![],
        })
        .expect("selector json"),
    )
    .expect("write selector");
    std::fs::write(
        &spec_path,
        format!(
            r#"source_parquet_path = "{}"
selector_report_path = "{}"
output_parquet_path = "{}"
report_path = "{}"
asset_id_column = "asset"
projected_columns = ["asset", "event_type", "payload"]
"#,
            source_path.display(),
            selector_path.display(),
            output_path.display(),
            report_path.display()
        ),
    )
    .expect("write spec");

    let artifact = write_selected_source_slice_from_spec_file(&spec_path).expect("source slice");

    assert_eq!(artifact.output_parquet_path, output_path);
    assert_eq!(artifact.source_rows, 4);
    assert_eq!(artifact.selected_rows, 2);
    assert_eq!(artifact.selected_asset_count, 1);
    assert_eq!(artifact.selected_asset_ids_hash, "selected-assets-hash");
    assert!(!artifact.output_parquet_sha256.is_empty());
    assert!(!artifact.report_hash.is_empty());

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
                "asset-b", "asset-a", "asset-a", "asset-b",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "book",
                "book",
                "price_change",
                "price_change",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "payload-b-book",
                "payload-a-book",
                "payload-a-price",
                "payload-b-price",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "ignored-b1",
                "ignored-a1",
                "ignored-a2",
                "ignored-b2",
            ])) as ArrayRef,
        ],
    )
    .expect("record batch");
    let file = File::create(path).expect("create source parquet");
    let mut writer = ArrowWriter::try_new(file, schema, None).expect("parquet writer");
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
