use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::{
    generate_evicted_binance_operator_inputs, generate_evicted_bybit_operator_inputs,
    rewrite_assignment, tempdir_in_repo_target,
};

use backtesting_vertical_slice::source_universe_conversion_work_order::{
    SourceUniverseConversionWorkOrder, SourceUniverseConversionWorkOrderStatus,
    write_source_universe_conversion_work_order_from_spec_file,
};

fn copy_spec_with_output_dir(source_spec: &Path, target_spec: &Path, output_dir: &Path) {
    let spec = fs::read_to_string(source_spec).expect("read committed source-universe spec");
    let mut replaced = false;
    let updated = spec
        .lines()
        .map(|line| {
            if line.starts_with("output_dir = ") {
                replaced = true;
                format!("output_dir = \"{}\"", output_dir.display())
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(replaced, "committed source-universe spec has output_dir");
    fs::write(target_spec, format!("{updated}\n")).expect("write temp source-universe spec");
}

#[test]
fn source_universe_conversion_work_order_lists_only_executable_operator_inputs() {
    let temp_dir = tempdir_in_repo_target();
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let output_dir = temp_dir.path().join("work-order");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.toml");

    fs::write(
        &operator_inputs_path,
        r#"{
  "schema_version": "source-universe-operator-inputs.v1",
  "input_id": "source-universe-operator-inputs-binance-test",
  "status": "blocked",
  "gate_id": "source-universe-object-gates-binance-test",
  "conversion_run_plan_id": "source-universe-conversion-run-plan-binance-test",
  "universe_id": "backfill-source-universe-binance-test",
  "venue": "binance",
  "source": "data_vision",
  "family": "trades",
  "table_family": "trades",
  "operator_run_id_prefix": "source-universe-operator-run-binance-test",
  "nt_venue": "BINANCE",
  "converter_identity": "csv-native-trades-to-canonical-trades.v1",
  "converter_version": "1",
  "raw_payload_container": "single_csv_zip",
  "max_decoded_bytes": 268435456,
  "max_source_rows": 1000000,
  "max_projected_row_groups": 128,
  "max_wall_seconds": 1800,
  "planned_object_count": 2,
  "planned_source_bytes": 300,
  "conversion_run_count": 1,
  "instrument_spec_count": 1,
  "converter_mapping_count": 1,
  "ready_input_count": 1,
  "blocked_input_count": 1,
  "artifact_refs": [],
  "converter_mappings": [],
  "instrument_specs": [],
  "records": [
    {
      "work_item_id": "binance:BTCUSDT:2026-03-01:hash-a",
      "status": "ready",
      "operator_run_id": "source-universe-operator-run-binance-test-00000",
      "source_binding": "binance-spot-native-trades",
      "category": "spot",
      "symbol": "BTCUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-a.zip",
      "source_url": "https://data.example/BTCUSDT.zip",
      "selected_object_sha256": "hash-a",
      "selected_object_bytes": 100,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "accepted_tranche_id": "tranche-a",
      "output_prefix": "s3://example/backtests/a",
      "instrument_key": "binance-spot-native-trades:spot:BTCUSDT",
      "converter_identity": "csv-native-trades-to-canonical-trades.v1",
      "converter_version": "1",
      "raw_payload_container": "single_csv_zip",
      "zip_member": "BTCUSDT-trades-2026-03-01.csv",
      "max_decoded_bytes": 268435456,
      "max_source_rows": 1000000,
      "max_projected_row_groups": 128,
      "max_wall_seconds": 1800,
      "schema_columns": null,
      "converter_csv": null,
      "blocking_reasons": []
    },
    {
      "work_item_id": "binance:ETHUSDT:2026-03-01:hash-b",
      "status": "blocked",
      "operator_run_id": "source-universe-operator-run-binance-test-00001",
      "source_binding": "binance-spot-native-trades",
      "category": "spot",
      "symbol": "ETHUSDT",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-b.zip",
      "source_url": "https://data.example/ETHUSDT.zip",
      "selected_object_sha256": "hash-b",
      "selected_object_bytes": 200,
      "source_proof_id": "source-proof-binance-test",
      "source_proof_version": 1,
      "accepted_tranche_id": "tranche-b",
      "output_prefix": "s3://example/backtests/b",
      "instrument_key": "binance-spot-native-trades:spot:ETHUSDT",
      "converter_identity": "csv-native-trades-to-canonical-trades.v1",
      "converter_version": "1",
      "raw_payload_container": "single_csv_zip",
      "zip_member": "ETHUSDT-trades-2026-03-01.csv",
      "max_decoded_bytes": 268435456,
      "max_source_rows": 1000000,
      "max_projected_row_groups": 128,
      "max_wall_seconds": 1800,
      "schema_columns": null,
      "converter_csv": null,
      "blocking_reasons": ["missing_instrument_metadata"]
    }
  ],
  "blocking_reasons": ["blocked_operator_input_records"]
}"#,
    )
    .expect("write operator inputs");
    fs::write(
        &spec_path,
        format!(
            r#"work_order_id = "source-universe-conversion-work-order-binance-test"
source_universe_operator_inputs_path = "{operator_inputs_path}"
output_dir = "{output_dir}"
"#,
            operator_inputs_path = operator_inputs_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_conversion_work_order_from_spec_file(&spec_path)
        .expect("work order write succeeds");
    let work_order: SourceUniverseConversionWorkOrder =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read work order"))
            .expect("work order parses");

    assert_eq!(
        work_order.status,
        SourceUniverseConversionWorkOrderStatus::PartiallyReady
    );
    assert_eq!(work_order.planned_object_count, 2);
    assert_eq!(work_order.executable_record_count, 1);
    assert_eq!(work_order.withheld_record_count, 1);
    assert_eq!(work_order.executable_source_bytes, 100);
    assert_eq!(work_order.withheld_source_bytes, 200);
    assert_eq!(work_order.records.len(), 1);
    assert_eq!(
        work_order.records[0].work_item_id,
        "binance:BTCUSDT:2026-03-01:hash-a"
    );
    assert_eq!(
        work_order.records[0].zip_member.as_deref(),
        Some("BTCUSDT-trades-2026-03-01.csv")
    );
    assert_eq!(work_order.withheld_records.len(), 1);
    assert_eq!(
        work_order.withheld_records[0].blocking_reasons,
        ["missing_instrument_metadata"]
    );
}

#[test]
fn source_universe_conversion_work_order_overwrites_existing_artifact_only_when_enabled() {
    let temp_dir = tempdir_in_repo_target();
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let output_dir = temp_dir.path().join("work-order");
    let spec_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.toml");

    fs::write(
        &operator_inputs_path,
        r#"{
  "schema_version": "source-universe-operator-inputs.v1",
  "input_id": "source-universe-operator-inputs-binance-test",
  "status": "ready",
  "gate_id": "source-universe-object-gates-binance-test",
  "conversion_run_plan_id": "source-universe-conversion-run-plan-binance-test",
  "universe_id": "backfill-source-universe-binance-test",
  "venue": "binance",
  "source": "data_vision",
  "family": "trades",
  "table_family": "trades",
  "operator_run_id_prefix": "source-universe-operator-run-binance-test",
  "nt_venue": "BINANCE",
  "converter_identity": "csv-native-trades-to-canonical-trades.v1",
  "converter_version": "1",
  "raw_payload_container": "single_csv_zip",
  "max_decoded_bytes": 268435456,
  "max_source_rows": 1000000,
  "max_projected_row_groups": 128,
  "max_wall_seconds": 1800,
  "planned_object_count": 0,
  "planned_source_bytes": 0,
  "conversion_run_count": 0,
  "instrument_spec_count": 0,
  "converter_mapping_count": 0,
  "ready_input_count": 0,
  "blocked_input_count": 0,
  "artifact_refs": [],
  "converter_mappings": [],
  "instrument_specs": [],
  "records": [],
  "blocking_reasons": []
}"#,
    )
    .expect("write operator inputs");
    fs::create_dir_all(&output_dir).expect("create output dir");
    fs::write(
        output_dir.join("source-universe-conversion-work-order.json"),
        br#"{"stale":true}"#,
    )
    .expect("write stale output");

    fs::write(
        &spec_path,
        format!(
            r#"work_order_id = "source-universe-conversion-work-order-binance-test"
source_universe_operator_inputs_path = "{operator_inputs_path}"
output_dir = "{output_dir}"
"#,
            operator_inputs_path = operator_inputs_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_universe_conversion_work_order_from_spec_file(&spec_path)
        .expect_err("dirty output is protected by default");
    assert!(
        err.to_string()
            .contains("dirty source-universe conversion work-order")
    );

    fs::write(
        &spec_path,
        format!(
            r#"work_order_id = "source-universe-conversion-work-order-binance-test"
source_universe_operator_inputs_path = "{operator_inputs_path}"
output_dir = "{output_dir}"
overwrite_existing_artifacts = true
"#,
            operator_inputs_path = operator_inputs_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write overwrite spec");

    let artifact = write_source_universe_conversion_work_order_from_spec_file(&spec_path)
        .expect("overwrite enabled");
    let work_order: SourceUniverseConversionWorkOrder =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read work order"))
            .expect("work order parses");
    assert_eq!(work_order.records.len(), 0);
}

#[test]
fn committed_bybit_and_binance_source_universe_work_orders_track_executable_scope() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let temp_dir = tempdir_in_repo_target();
    let binance_operator_inputs_path =
        generate_evicted_binance_operator_inputs(&reference_root, temp_dir.path());
    let (bybit_operator_inputs_path, _) =
        generate_evicted_bybit_operator_inputs(&reference_root, temp_dir.path());

    let committed_bybit_spec = reference_root
        .join("source-universe-conversion-work-orders/bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
        .join("source-universe-conversion-work-order.toml");
    let bybit_spec = temp_dir
        .path()
        .join("bybit-source-universe-conversion-work-order.toml");
    copy_spec_with_output_dir(
        &committed_bybit_spec,
        &bybit_spec,
        &temp_dir.path().join("bybit-work-order"),
    );
    let bybit_spec_text = fs::read_to_string(&bybit_spec).expect("read temp Bybit work-order spec");
    fs::write(
        &bybit_spec,
        rewrite_assignment(
            &bybit_spec_text,
            "source_universe_operator_inputs_path",
            &bybit_operator_inputs_path,
        ),
    )
    .expect("write temp Bybit work-order spec with regenerated operator inputs");
    let bybit_artifact = write_source_universe_conversion_work_order_from_spec_file(&bybit_spec)
        .expect("Bybit work order is reproducible");
    let bybit: SourceUniverseConversionWorkOrder =
        serde_json::from_slice(&fs::read(&bybit_artifact.path).expect("read Bybit work order"))
            .expect("Bybit work order parses");

    assert_eq!(bybit.status, SourceUniverseConversionWorkOrderStatus::Ready);
    assert_eq!(bybit.planned_object_count, 5_857);
    assert_eq!(bybit.executable_record_count, 5_857);
    assert_eq!(bybit.withheld_record_count, 0);
    assert_eq!(bybit.records.len(), 5_857);
    assert!(bybit.withheld_records.is_empty());

    let committed_binance_spec = reference_root
        .join("source-universe-conversion-work-orders/binance-data-vision-trades-2026-03-01-all-instruments")
        .join("source-universe-conversion-work-order.toml");
    let binance_spec = temp_dir
        .path()
        .join("binance-source-universe-conversion-work-order.toml");
    copy_spec_with_output_dir(
        &committed_binance_spec,
        &binance_spec,
        &temp_dir.path().join("binance-work-order"),
    );
    let binance_spec_text =
        fs::read_to_string(&binance_spec).expect("read temp Binance work-order spec");
    fs::write(
        &binance_spec,
        rewrite_assignment(
            &binance_spec_text,
            "source_universe_operator_inputs_path",
            &binance_operator_inputs_path,
        ),
    )
    .expect("write temp Binance work-order spec with regenerated operator inputs");
    let binance_artifact =
        write_source_universe_conversion_work_order_from_spec_file(&binance_spec)
            .expect("Binance work order is reproducible");
    let binance: SourceUniverseConversionWorkOrder =
        serde_json::from_slice(&fs::read(&binance_artifact.path).expect("read Binance work order"))
            .expect("Binance work order parses");

    assert_eq!(
        binance.status,
        SourceUniverseConversionWorkOrderStatus::PartiallyReady
    );
    assert_eq!(binance.planned_object_count, 2_051);
    assert_eq!(binance.executable_record_count, 2_035);
    assert_eq!(binance.withheld_record_count, 16);
    assert_eq!(binance.records.len(), 2_035);
    assert_eq!(binance.withheld_records.len(), 16);
    assert!(
        binance
            .withheld_records
            .iter()
            .all(|record| record.blocking_reasons == ["missing_instrument_metadata"])
    );
}
