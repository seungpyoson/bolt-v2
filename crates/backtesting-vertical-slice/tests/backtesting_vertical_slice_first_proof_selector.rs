use std::process::Command;

use backtesting_vertical_slice::first_proof_selector::{
    AssetEventCount, FIRST_PROOF_SELECTOR_REPORT_FILE, FirstProofEventCountLedger,
    FirstProofEventCountLedgerReport, FirstProofSelection, FirstProofSelectorReport,
    FirstProofSelectorStatus, evaluate_first_proof_selector,
    write_first_proof_event_count_ledger_from_spec_file,
    write_first_proof_selector_report_from_spec_file,
};

#[test]
fn first_proof_selector_uses_configured_event_roles_without_asset_constants() {
    let selection = FirstProofSelection {
        required_event_families: vec![
            "snapshot".to_string(),
            "update".to_string(),
            "execution".to_string(),
        ],
        excluded_event_families: vec!["instrument_epoch".to_string()],
        candidate_asset_ids: Vec::new(),
        row_budget: 10,
        max_selected_assets: 2,
    };
    let event_counts = vec![
        count("asset-over-budget", "snapshot", 1),
        count("asset-over-budget", "update", 10),
        count("asset-over-budget", "execution", 1),
        count("asset-excluded", "snapshot", 1),
        count("asset-excluded", "update", 2),
        count("asset-excluded", "execution", 1),
        count("asset-excluded", "instrument_epoch", 1),
        count("asset-missing-required", "snapshot", 1),
        count("asset-missing-required", "update", 2),
        count("asset-two", "snapshot", 1),
        count("asset-two", "update", 3),
        count("asset-two", "execution", 1),
        count("asset-one", "snapshot", 1),
        count("asset-one", "update", 1),
        count("asset-one", "execution", 1),
    ];

    let report = evaluate_first_proof_selector("bounded-l2-first-proof", &event_counts, &selection);

    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert!(report.blocking_issues.is_empty());
    assert_eq!(report.total_assets, 5);
    assert_eq!(report.eligible_assets, 2);
    assert_eq!(report.excluded_event_asset_count, 1);
    assert_eq!(report.excluded_event_row_count, 1);
    assert!(!report.event_count_ledger_hash.is_empty());
    assert_eq!(
        report
            .selected_assets
            .iter()
            .map(|asset| (asset.asset_id.as_str(), asset.replay_rows))
            .collect::<Vec<_>>(),
        vec![("asset-one", 3), ("asset-two", 5)]
    );
    assert!(!report.selected_asset_ids_hash.is_empty());

    let second = evaluate_first_proof_selector("bounded-l2-first-proof", &event_counts, &selection);
    assert_eq!(
        report.selected_asset_ids_hash,
        second.selected_asset_ids_hash
    );
}

#[test]
fn first_proof_selector_honors_configured_candidate_universe() {
    let selection = FirstProofSelection {
        required_event_families: vec![
            "snapshot".to_string(),
            "update".to_string(),
            "execution".to_string(),
        ],
        excluded_event_families: vec!["instrument_epoch".to_string()],
        candidate_asset_ids: vec!["asset-two".to_string()],
        row_budget: 10,
        max_selected_assets: 1,
    };
    let event_counts = vec![
        count("asset-one", "snapshot", 1),
        count("asset-one", "update", 1),
        count("asset-one", "execution", 1),
        count("asset-two", "snapshot", 1),
        count("asset-two", "update", 3),
        count("asset-two", "execution", 1),
    ];

    let report = evaluate_first_proof_selector("bounded-l2-first-proof", &event_counts, &selection);

    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert_eq!(report.eligible_assets, 1);
    assert_eq!(
        report
            .selected_assets
            .iter()
            .map(|asset| (asset.asset_id.as_str(), asset.replay_rows))
            .collect::<Vec<_>>(),
        vec![("asset-two", 5)]
    );
}

#[test]
fn first_proof_selector_writer_is_config_and_ledger_driven_and_idempotent() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("selector.toml");
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&FirstProofEventCountLedger {
            event_counts: vec![
                count("asset-two", "snapshot", 1),
                count("asset-two", "update", 3),
                count("asset-two", "execution", 1),
                count("asset-one", "snapshot", 1),
                count("asset-one", "update", 1),
                count("asset-one", "execution", 1),
            ],
        })
        .expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"selector_id = "bounded-l2-first-proof"
event_count_ledger_path = "{}"
output_dir = "{}"

[selection]
required_event_families = ["snapshot", "update", "execution"]
excluded_event_families = ["instrument_epoch"]
row_budget = 10
max_selected_assets = 1
"#,
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write selector spec");

    let first = write_first_proof_selector_report_from_spec_file(&spec_path).expect("first");
    let second = write_first_proof_selector_report_from_spec_file(&spec_path).expect("second");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(
        first.path,
        output_dir.join(FIRST_PROOF_SELECTOR_REPORT_FILE)
    );
    assert_eq!(first.selected_asset_count, 1);

    let report: FirstProofSelectorReport =
        serde_json::from_slice(&std::fs::read(first.path).expect("read report"))
            .expect("selector report json");
    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert_eq!(
        report
            .selected_assets
            .iter()
            .map(|asset| asset.asset_id.as_str())
            .collect::<Vec<_>>(),
        vec!["asset-one"]
    );
    assert!(!report.event_count_ledger_hash.is_empty());
    assert!(!report.selected_asset_ids_hash.is_empty());
}

#[test]
fn first_proof_selector_cli_writes_artifact_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let output_dir = dir.path().join("out");
    let spec_path = dir.path().join("selector.toml");
    std::fs::write(
        &ledger_path,
        serde_json::to_vec_pretty(&FirstProofEventCountLedger {
            event_counts: vec![
                count("asset-two", "snapshot", 1),
                count("asset-two", "update", 3),
                count("asset-two", "execution", 1),
                count("asset-one", "snapshot", 1),
                count("asset-one", "update", 1),
                count("asset-one", "execution", 1),
            ],
        })
        .expect("ledger json"),
    )
    .expect("write ledger");
    std::fs::write(
        &spec_path,
        format!(
            r#"selector_id = "bounded-l2-first-proof"
event_count_ledger_path = "{}"
output_dir = "{}"

[selection]
required_event_families = ["snapshot", "update", "execution"]
excluded_event_families = ["instrument_epoch"]
row_budget = 10
max_selected_assets = 1
"#,
            ledger_path.display(),
            output_dir.display()
        ),
    )
    .expect("write selector spec");

    let binary =
        std::env::var("CARGO_BIN_EXE_first_proof_selector").expect("first_proof_selector binary");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run first-proof selector CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(
        stdout.contains("first_proof_selector_report = "),
        "{stdout}"
    );
    assert!(stdout.contains("status = selected"), "{stdout}");
    assert!(stdout.contains("selected_asset_count = 1"), "{stdout}");

    let report_path = output_dir.join(FIRST_PROOF_SELECTOR_REPORT_FILE);
    let report: FirstProofSelectorReport =
        serde_json::from_slice(&std::fs::read(report_path).expect("read report"))
            .expect("selector report json");
    assert_eq!(report.status, FirstProofSelectorStatus::Selected);
    assert_eq!(report.selected_assets[0].asset_id, "asset-one");
}

#[test]
fn first_proof_event_count_ledger_scans_configured_parquet_columns() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let spec_path = dir.path().join("ledger.toml");
    write_event_count_source_parquet(&source_path);
    let max_source_parquet_bytes = std::fs::metadata(&source_path)
        .expect("source parquet metadata")
        .len();
    std::fs::write(
        &spec_path,
        format!(
            r#"source_parquet_path = "{}"
output_path = "{}"
max_source_parquet_bytes = {max_source_parquet_bytes}
asset_id_column = "asset"
event_family_column = "event_type"
"#,
            source_path.display(),
            ledger_path.display()
        ),
    )
    .expect("write ledger spec");

    let artifact =
        write_first_proof_event_count_ledger_from_spec_file(&spec_path).expect("write ledger");

    assert_eq!(artifact.path, ledger_path);
    assert_eq!(artifact.source_rows, 7);
    assert_eq!(artifact.event_count_rows, 6);
    assert!(!artifact.content_hash.is_empty());

    let ledger: FirstProofEventCountLedgerReport =
        serde_json::from_slice(&std::fs::read(&artifact.path).expect("read ledger"))
            .expect("ledger json");
    assert_eq!(ledger.source_rows, 7);
    assert_eq!(
        ledger.event_counts,
        vec![
            count_with_row_groups("asset-a", "book", 1, &[0]),
            count_with_row_groups("asset-a", "last_trade_price", 1, &[1]),
            count_with_row_groups("asset-a", "price_change", 1, &[1]),
            count_with_row_groups("asset-b", "book", 1, &[2]),
            count_with_row_groups("asset-b", "price_change", 2, &[0]),
            count_with_row_groups("asset-b", "tick_size_change", 1, &[1]),
        ]
    );

    let ledger_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&artifact.path).expect("read ledger"))
            .expect("ledger json value");
    let event_counts = ledger_json["event_counts"]
        .as_array()
        .expect("event count rows");
    let source_row_groups_for = |asset_id: &str, event_family: &str| -> Vec<u64> {
        event_counts
            .iter()
            .find(|row| row["asset_id"] == asset_id && row["event_family"] == event_family)
            .expect("event count row")["source_row_groups"]
            .as_array()
            .expect("source row groups")
            .iter()
            .map(|value| value.as_u64().expect("row group id"))
            .collect()
    };
    assert_eq!(source_row_groups_for("asset-a", "book"), vec![0]);
    assert_eq!(
        source_row_groups_for("asset-a", "last_trade_price"),
        vec![1]
    );
    assert_eq!(source_row_groups_for("asset-a", "price_change"), vec![1]);
    assert_eq!(source_row_groups_for("asset-b", "book"), vec![2]);
    assert_eq!(source_row_groups_for("asset-b", "price_change"), vec![0]);
    assert_eq!(
        source_row_groups_for("asset-b", "tick_size_change"),
        vec![1]
    );
}

#[test]
fn first_proof_event_count_ledger_cli_writes_artifact_from_config_owned_spec() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let spec_path = dir.path().join("ledger.toml");
    write_event_count_source_parquet(&source_path);
    let max_source_parquet_bytes = std::fs::metadata(&source_path)
        .expect("source parquet metadata")
        .len();
    std::fs::write(
        &spec_path,
        format!(
            r#"source_parquet_path = "{}"
output_path = "{}"
max_source_parquet_bytes = {max_source_parquet_bytes}
asset_id_column = "asset"
event_family_column = "event_type"
"#,
            source_path.display(),
            ledger_path.display()
        ),
    )
    .expect("write ledger spec");

    let binary = std::env::var("CARGO_BIN_EXE_first_proof_event_count_ledger")
        .expect("first_proof_event_count_ledger binary path");
    let output = Command::new(binary)
        .arg("--spec")
        .arg(&spec_path)
        .output()
        .expect("run first-proof event-count ledger CLI");

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf8");
    assert!(stdout.contains("event_count_ledger = "), "{stdout}");
    assert!(stdout.contains("source_rows = 7"), "{stdout}");
    assert!(stdout.contains("event_count_rows = 6"), "{stdout}");

    let ledger: FirstProofEventCountLedgerReport =
        serde_json::from_slice(&std::fs::read(ledger_path).expect("read ledger"))
            .expect("ledger json");
    assert_eq!(ledger.source_rows, 7);
}

#[test]
fn first_proof_event_count_ledger_rejects_source_above_configured_byte_budget_before_output() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let source_path = dir.path().join("source.parquet");
    let ledger_path = dir.path().join("event-count-ledger.json");
    let spec_path = dir.path().join("ledger.toml");
    write_event_count_source_parquet(&source_path);
    std::fs::write(
        &spec_path,
        format!(
            r#"source_parquet_path = "{}"
output_path = "{}"
max_source_parquet_bytes = 1
asset_id_column = "asset"
event_family_column = "event_type"
"#,
            source_path.display(),
            ledger_path.display()
        ),
    )
    .expect("write ledger spec");

    let err = write_first_proof_event_count_ledger_from_spec_file(&spec_path)
        .expect_err("source above budget must be rejected");

    assert!(
        err.to_string().contains("exceeds max_source_parquet_bytes"),
        "{err}"
    );
    assert!(
        !ledger_path.exists(),
        "oversized source must be rejected before output"
    );
}

fn count(asset_id: &str, event_family: &str, rows: u64) -> AssetEventCount {
    count_with_row_groups(asset_id, event_family, rows, &[])
}

fn count_with_row_groups(
    asset_id: &str,
    event_family: &str,
    rows: u64,
    source_row_groups: &[u64],
) -> AssetEventCount {
    AssetEventCount {
        asset_id: asset_id.to_string(),
        event_family: event_family.to_string(),
        rows,
        source_row_groups: source_row_groups.to_vec(),
    }
}

fn write_event_count_source_parquet(path: &std::path::Path) {
    use std::{fs::File, sync::Arc};

    use arrow::{
        array::{ArrayRef, StringArray},
        datatypes::{DataType, Field, Schema},
        record_batch::RecordBatch,
    };
    use parquet::{arrow::ArrowWriter, file::properties::WriterProperties};

    let schema = Arc::new(Schema::new(vec![
        Field::new("asset", DataType::Utf8, false),
        Field::new("event_type", DataType::Utf8, false),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![
                "asset-b", "asset-a", "asset-b", "asset-a", "asset-b", "asset-a", "asset-b",
            ])) as ArrayRef,
            Arc::new(StringArray::from(vec![
                "price_change",
                "book",
                "price_change",
                "last_trade_price",
                "tick_size_change",
                "price_change",
                "book",
            ])) as ArrayRef,
        ],
    )
    .expect("record batch");
    let file = File::create(path).expect("create source parquet");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(3))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props)).expect("parquet writer");
    writer.write(&batch).expect("write batch");
    writer.close().expect("close parquet");
}
