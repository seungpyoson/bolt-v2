use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::tempdir_in_repo_target;

use backtesting_vertical_slice::reference_fixture_index::{
    EvictedFixtureIndex, TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH, repo_root_from_manifest_dir,
};
use backtesting_vertical_slice::source_universe_conversion_run_plan::write_source_universe_conversion_run_plan_from_spec_file;
use backtesting_vertical_slice::source_universe_operator_inputs::{
    SourceUniverseOperatorInputRecordStatus, SourceUniverseOperatorInputs,
    SourceUniverseOperatorInputsStatus, write_source_universe_operator_inputs_from_spec_file,
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

fn replace_spec_path(spec_text: &str, committed_path: &str, temp_path: &Path) -> String {
    assert!(
        spec_text.contains(committed_path),
        "committed spec contains {committed_path}"
    );
    spec_text.replace(committed_path, &temp_path.display().to_string())
}

fn assert_bytes_match_committed(generated_path: &Path, committed_path: &Path, label: &str) {
    let generated = fs::read(generated_path)
        .unwrap_or_else(|err| panic!("read generated {label} {}: {err}", generated_path.display()));
    let committed = fs::read(committed_path)
        .unwrap_or_else(|err| panic!("read committed {label} {}: {err}", committed_path.display()));
    assert!(
        generated == committed,
        "regenerated {label} bytes must match committed artifact {}; generated_len={} committed_len={}",
        committed_path.display(),
        generated.len(),
        committed.len()
    );
}

#[test]
fn source_universe_operator_inputs_materialize_ready_bybit_spot_object() {
    let temp_dir = tempdir_in_repo_target();
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let run_plan_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.json");
    let conversion_plan_path = temp_dir.path().join("conversion-plan.json");
    let metadata_path = temp_dir.path().join("instrument-metadata.json");
    let output_dir = temp_dir.path().join("operator-inputs");
    let spec_path = temp_dir.path().join("source-universe-operator-inputs.toml");

    fs::write(
        &gates_path,
        r#"{
  "schema_version": "source-universe-object-gates.v1",
  "gate_id": "source-universe-object-gates-bybit-test",
  "status": "ready",
  "queue_id": "source-universe-conversion-queue-bybit-test",
  "manifest_id": "backfill-source-universe-object-manifest-bybit-test",
  "universe_id": "backfill-source-universe-bybit-test",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick-trades",
  "table_family": "trades",
  "queue_path": "queue.json",
  "queue_hash": "queue-hash",
  "work_item_count": 1,
  "accepted_gate_count": 1,
  "source_binding_count": 1,
  "total_accepted_bytes": 100,
  "source_binding_summaries": [],
  "artifact_refs": [],
  "records": [
    {
      "work_item_id": "bybit:spot:BNBUSDC:2026-03-01:hash-a",
      "gate_status": "ready",
      "source_binding": "bybit-spot-native-trades",
      "table_family": "trades",
      "category": "spot",
      "symbol": "BNBUSDC",
      "archive_date": "2026-03-01",
      "source_uri": "s3://example/raw/hash-a.csv.gz",
      "source_url": "https://public.bybit.example/spot/BNBUSDC.csv.gz",
      "selected_object_sha256": "hash-a",
      "selected_object_bytes": 100,
      "source_proof_id": "source-proof-bybit-test",
      "source_proof_version": 1,
      "source_proof_hash": "proof-hash",
      "category_manifest_id": "category-manifest-bybit-test",
      "category_manifest_hash": "manifest-hash",
      "source_proof_scope_report_id": "scope-a",
      "accepted_tranche_id": "tranche-a",
      "output_prefix": "s3://example/backtests/a"
    }
  ]
}"#,
    )
    .expect("write gates");
    fs::write(
        &run_plan_path,
        r#"{
  "schema_version": "source-universe-conversion-run-plan.v1",
  "plan_id": "source-universe-conversion-run-plan-bybit-test",
  "status": "ready",
  "gate_id": "source-universe-object-gates-bybit-test",
  "queue_id": "source-universe-conversion-queue-bybit-test",
  "manifest_id": "backfill-source-universe-object-manifest-bybit-test",
  "universe_id": "backfill-source-universe-bybit-test",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick-trades",
  "table_family": "trades",
  "object_gates_path": "source-universe-object-gates.json",
  "object_gates_hash": "gates-hash",
  "max_objects_per_run": 500,
  "max_source_bytes_per_run": 1000,
  "source_binding_count": 1,
  "object_count": 1,
  "planned_object_count": 1,
  "total_source_bytes": 100,
  "planned_source_bytes": 100,
  "run_count": 1,
  "category_summaries": [],
  "artifact_refs": [],
  "runs": [
    {
      "run_id": "source-universe-conversion-run-plan-bybit-test:run-00001",
      "run_index": 1,
      "source_binding": "bybit-spot-native-trades",
      "table_family": "trades",
      "category": "spot",
      "first_archive_date": "2026-03-01",
      "last_archive_date": "2026-03-01",
      "object_count": 1,
      "source_bytes": 100,
      "work_item_ids": ["bybit:spot:BNBUSDC:2026-03-01:hash-a"],
      "accepted_tranche_ids": ["tranche-a"],
      "output_prefixes": ["s3://example/backtests/a"]
    }
  ]
}"#,
    )
    .expect("write run plan");
    fs::write(
        &conversion_plan_path,
        r#"{
  "category_batches": [
    {
      "category": "spot",
      "source_binding": "bybit-spot-native-trades",
      "status": "converter_mapping_configured",
      "schema_columns": ["id", "timestamp", "price", "volume", "side", "rpi"],
      "converter_csv": {
        "has_headers": true,
        "trade_id_column": "id",
        "timestamp_column": "timestamp",
        "timestamp_unit": "milliseconds",
        "price_column": "price",
        "size_column": "volume",
        "side_column": "side",
        "buyer_side_values": ["buy"],
        "seller_side_values": ["sell"]
      }
    }
  ]
}"#,
    )
    .expect("write conversion plan");
    fs::write(
        &metadata_path,
        r#"{
  "records": [
    {
      "category": "spot",
      "source_binding": "bybit-spot-native-trades",
      "source_uri": "https://api.bybit.example/v5/market/instruments-info?category=spot&symbol=BNBUSDC",
      "symbol": "BNBUSDC",
      "instrument": {
        "symbol": "BNBUSDC",
        "baseCoin": "BNB",
        "quoteCoin": "USDC",
        "priceFilter": {
          "tickSize": "0.1"
        },
        "lotSizeFilter": {
          "basePrecision": "0.0001",
          "minOrderQty": "0.0001",
          "maxOrderQty": "1400",
          "minOrderAmt": "5",
          "maxOrderAmt": "200000"
        }
      }
    }
  ]
}"#,
    )
    .expect("write metadata");
    fs::write(
        &spec_path,
        format!(
            r#"input_id = "source-universe-operator-inputs-bybit-test"
source_universe_object_gates_path = "{gates_path}"
source_universe_conversion_run_plan_path = "{run_plan_path}"
source_universe_conversion_plan_path = "{conversion_plan_path}"
instrument_metadata_snapshot_path = "{metadata_path}"
output_dir = "{output_dir}"
operator_run_id_prefix = "source-universe-operator-run-bybit-test"
nt_venue = "BYBIT"
converter_identity = "csv-native-trades-to-canonical-trades.v1"
converter_version = "1"
raw_payload_container = "csv_gzip"
max_decoded_bytes = 268435456
max_source_rows = 1000000
max_projected_row_groups = 128
max_wall_seconds = 1800
default_spot_max_notional = "1000000000"
default_derivative_max_notional = "1000000000"
default_derivative_multiplier = "1"
default_maker_fee = "0"
default_taker_fee = "0"
"#,
            gates_path = gates_path.display(),
            run_plan_path = run_plan_path.display(),
            conversion_plan_path = conversion_plan_path.display(),
            metadata_path = metadata_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_operator_inputs_from_spec_file(&spec_path)
        .expect("operator inputs write succeeds");
    let inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read inputs"))
            .expect("inputs parse");

    assert_eq!(inputs.status, SourceUniverseOperatorInputsStatus::Ready);
    assert_eq!(inputs.planned_object_count, 1);
    assert_eq!(inputs.ready_input_count, 1);
    assert_eq!(inputs.blocked_input_count, 0);
    assert_eq!(inputs.instrument_spec_count, 1);
    assert_eq!(inputs.converter_mapping_count, 1);
    assert_eq!(inputs.records.len(), 1);
    assert_eq!(
        inputs.records[0].status,
        SourceUniverseOperatorInputRecordStatus::Ready
    );
    assert_eq!(
        inputs.records[0].instrument_key,
        "bybit-spot-native-trades:spot:BNBUSDC"
    );
    assert!(inputs.records[0].schema_columns.is_some());
    assert!(inputs.records[0].converter_csv.is_some());
    assert_eq!(inputs.records[0].zip_member, None);
    assert!(inputs.records[0].blocking_reasons.is_empty());
}

#[test]
fn source_universe_operator_inputs_overwrites_existing_artifact_only_when_enabled() {
    let temp_dir = tempdir_in_repo_target();
    let gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let run_plan_path = temp_dir
        .path()
        .join("source-universe-conversion-run-plan.json");
    let conversion_plan_path = temp_dir.path().join("conversion-plan.json");
    let metadata_path = temp_dir.path().join("instrument-metadata.json");
    let output_dir = temp_dir.path().join("operator-inputs");
    let spec_path = temp_dir.path().join("source-universe-operator-inputs.toml");

    fs::write(
        &gates_path,
        r#"{
  "schema_version": "source-universe-object-gates.v1",
  "gate_id": "source-universe-object-gates-bybit-test",
  "status": "ready",
  "queue_id": "source-universe-conversion-queue-bybit-test",
  "manifest_id": "backfill-source-universe-object-manifest-bybit-test",
  "universe_id": "backfill-source-universe-bybit-test",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick-trades",
  "table_family": "trades",
  "queue_path": "queue.json",
  "queue_hash": "queue-hash",
  "work_item_count": 0,
  "accepted_gate_count": 0,
  "source_binding_count": 0,
  "total_accepted_bytes": 0,
  "source_binding_summaries": [],
  "artifact_refs": [],
  "records": []
}"#,
    )
    .expect("write gates");
    fs::write(
        &run_plan_path,
        r#"{
  "schema_version": "source-universe-conversion-run-plan.v1",
  "plan_id": "source-universe-conversion-run-plan-bybit-test",
  "status": "ready",
  "gate_id": "source-universe-object-gates-bybit-test",
  "queue_id": "source-universe-conversion-queue-bybit-test",
  "manifest_id": "backfill-source-universe-object-manifest-bybit-test",
  "universe_id": "backfill-source-universe-bybit-test",
  "venue": "bybit",
  "source": "public_archive",
  "family": "tick-trades",
  "table_family": "trades",
  "object_gates_path": "source-universe-object-gates.json",
  "object_gates_hash": "gates-hash",
  "max_objects_per_run": 500,
  "max_source_bytes_per_run": 1000,
  "source_binding_count": 0,
  "object_count": 0,
  "planned_object_count": 0,
  "total_source_bytes": 0,
  "planned_source_bytes": 0,
  "run_count": 0,
  "category_summaries": [],
  "artifact_refs": [],
  "runs": []
}"#,
    )
    .expect("write run plan");
    fs::write(&conversion_plan_path, r#"{"category_batches":[]}"#).expect("write conversion plan");
    fs::write(&metadata_path, r#"{"records":[]}"#).expect("write metadata");
    fs::create_dir_all(&output_dir).expect("create output dir");
    fs::write(
        output_dir.join("source-universe-operator-inputs.json"),
        br#"{"stale":true}"#,
    )
    .expect("write stale output");

    fs::write(
        &spec_path,
        format!(
            r#"input_id = "source-universe-operator-inputs-bybit-test"
source_universe_object_gates_path = "{gates_path}"
source_universe_conversion_run_plan_path = "{run_plan_path}"
source_universe_conversion_plan_path = "{conversion_plan_path}"
instrument_metadata_snapshot_path = "{metadata_path}"
output_dir = "{output_dir}"
operator_run_id_prefix = "source-universe-operator-run-bybit-test"
nt_venue = "BYBIT"
converter_identity = "csv-native-trades-to-canonical-trades.v1"
converter_version = "1"
raw_payload_container = "csv_gzip"
max_decoded_bytes = 268435456
max_source_rows = 1000000
max_projected_row_groups = 128
max_wall_seconds = 1800
default_spot_max_notional = "1000000000"
default_derivative_max_notional = "1000000000"
default_derivative_multiplier = "1"
default_maker_fee = "0"
default_taker_fee = "0"
"#,
            gates_path = gates_path.display(),
            run_plan_path = run_plan_path.display(),
            conversion_plan_path = conversion_plan_path.display(),
            metadata_path = metadata_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_universe_operator_inputs_from_spec_file(&spec_path)
        .expect_err("dirty output is protected by default");
    assert!(
        err.to_string()
            .contains("dirty source-universe operator-inputs")
    );

    fs::write(
        &spec_path,
        format!(
            r#"input_id = "source-universe-operator-inputs-bybit-test"
source_universe_object_gates_path = "{gates_path}"
source_universe_conversion_run_plan_path = "{run_plan_path}"
source_universe_conversion_plan_path = "{conversion_plan_path}"
instrument_metadata_snapshot_path = "{metadata_path}"
output_dir = "{output_dir}"
operator_run_id_prefix = "source-universe-operator-run-bybit-test"
nt_venue = "BYBIT"
converter_identity = "csv-native-trades-to-canonical-trades.v1"
converter_version = "1"
raw_payload_container = "csv_gzip"
max_decoded_bytes = 268435456
max_source_rows = 1000000
max_projected_row_groups = 128
max_wall_seconds = 1800
default_spot_max_notional = "1000000000"
default_derivative_max_notional = "1000000000"
default_derivative_multiplier = "1"
default_maker_fee = "0"
default_taker_fee = "0"
overwrite_existing_artifacts = true
"#,
            gates_path = gates_path.display(),
            run_plan_path = run_plan_path.display(),
            conversion_plan_path = conversion_plan_path.display(),
            metadata_path = metadata_path.display(),
            output_dir = output_dir.display(),
        ),
    )
    .expect("write overwrite spec");

    let artifact = write_source_universe_operator_inputs_from_spec_file(&spec_path)
        .expect("overwrite enabled");
    let inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read inputs"))
            .expect("inputs parse");
    assert_eq!(inputs.records.len(), 0);
}

#[test]
fn committed_bybit_source_universe_operator_inputs_track_current_gates() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let spec_path = reference_root
        .join("source-universe-operator-inputs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
        .join("source-universe-operator-inputs.toml");
    let temp_dir = tempdir_in_repo_target();

    let run_plan_spec_path = temp_dir
        .path()
        .join("bybit-source-universe-conversion-run-plan.toml");
    copy_spec_with_output_dir(
        &reference_root
            .join("source-universe-conversion-run-plans/bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
            .join("source-universe-conversion-run-plan.toml"),
        &run_plan_spec_path,
        &temp_dir.path().join("source-universe-conversion-run-plan"),
    );
    let run_plan_artifact =
        write_source_universe_conversion_run_plan_from_spec_file(&run_plan_spec_path)
            .expect("Bybit run plan is reproducible");

    let temp_spec_path = temp_dir
        .path()
        .join("bybit-source-universe-operator-inputs.toml");
    copy_spec_with_output_dir(
        &spec_path,
        &temp_spec_path,
        &temp_dir.path().join("source-universe-operator-inputs"),
    );
    let spec_text = fs::read_to_string(&temp_spec_path).expect("read temp operator-inputs spec");
    fs::write(
        &temp_spec_path,
        replace_spec_path(
            &spec_text,
            TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH,
            &run_plan_artifact.path,
        ),
    )
    .expect("write temp operator-inputs spec with regenerated run plan");

    let artifact = write_source_universe_operator_inputs_from_spec_file(&temp_spec_path)
        .expect("committed Bybit operator inputs are reproducible");
    let mut inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read inputs"))
            .expect("inputs parse");
    let evicted_index =
        EvictedFixtureIndex::load(&repo_root_from_manifest_dir()).expect("load eviction index");
    let bybit_run_plan_sha256 = evicted_index
        .sha256_for(TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH)
        .unwrap_or_else(|| {
            panic!("evicted fixture index does not contain {TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH}")
        });
    assert_eq!(
        run_plan_artifact.content_hash, bybit_run_plan_sha256,
        "regenerated Bybit run-plan bytes must match the evicted fixture index"
    );
    let run_plan_ref = inputs
        .artifact_refs
        .iter_mut()
        .find(|artifact_ref| artifact_ref.role == "source_universe_conversion_run_plan")
        .expect("operator inputs record source-universe conversion run-plan artifact ref");
    run_plan_ref.path = Path::new(TIER1_BYBIT_CONVERSION_RUN_PLAN_PATH).to_path_buf();
    let normalized = serde_json::to_vec_pretty(&inputs).expect("serialize normalized inputs");
    let committed_artifact_path = spec_path
        .parent()
        .expect("operator-inputs spec parent")
        .join("operator-inputs/source-universe-operator-inputs.json");
    let committed = fs::read(&committed_artifact_path).expect("read committed Bybit inputs");
    assert!(
        normalized == committed,
        "regenerated Bybit operator-inputs bytes must match committed artifact {} after replacing the temp-only evicted run-plan identity; generated_len={} committed_len={}",
        committed_artifact_path.display(),
        normalized.len(),
        committed.len()
    );

    assert_eq!(inputs.status, SourceUniverseOperatorInputsStatus::Ready);
    assert_eq!(inputs.planned_object_count, 5_857);
    assert_eq!(inputs.ready_input_count, 5_857);
    assert_eq!(inputs.blocked_input_count, 0);
    assert_eq!(inputs.instrument_spec_count, 106);
    assert_eq!(inputs.converter_mapping_count, 3);
    assert_eq!(inputs.records.len(), 5_857);
    assert!(inputs.blocking_reasons.is_empty());
    assert!(
        inputs
            .records
            .iter()
            .all(|record| record.status == SourceUniverseOperatorInputRecordStatus::Ready)
    );
}

#[test]
fn committed_binance_source_universe_operator_inputs_track_current_gates_without_overclaiming() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");
    let spec_path = reference_root
        .join(
            "source-universe-operator-inputs/binance-data-vision-trades-2026-03-01-all-instruments",
        )
        .join("source-universe-operator-inputs.toml");
    let committed_artifact_path = spec_path
        .parent()
        .expect("operator-inputs spec parent")
        .join("operator-inputs/source-universe-operator-inputs.json");
    let temp_dir = tempdir_in_repo_target();
    let temp_spec_path = temp_dir
        .path()
        .join("binance-source-universe-operator-inputs.toml");
    copy_spec_with_output_dir(
        &spec_path,
        &temp_spec_path,
        &temp_dir.path().join("source-universe-operator-inputs"),
    );
    let artifact = write_source_universe_operator_inputs_from_spec_file(&temp_spec_path)
        .expect("committed Binance operator inputs are reproducible");
    assert_bytes_match_committed(
        &artifact.path,
        &committed_artifact_path,
        "Binance operator-inputs",
    );
    let inputs: SourceUniverseOperatorInputs =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read inputs"))
            .expect("inputs parse");

    assert_eq!(inputs.planned_object_count, 2_051);
    assert_eq!(inputs.status, SourceUniverseOperatorInputsStatus::Blocked);
    assert_eq!(inputs.ready_input_count, 2_035);
    assert_eq!(inputs.blocked_input_count, 16);
    assert_eq!(inputs.instrument_spec_count, 2_035);
    assert_eq!(inputs.converter_mapping_count, 5);
    assert_eq!(inputs.records.len(), 2_051);
    assert_eq!(
        inputs.ready_input_count + inputs.blocked_input_count,
        inputs.planned_object_count
    );
    assert!(
        inputs.records.iter().all(|record| record
            .zip_member
            .as_ref()
            .is_some_and(|value| value.ends_with(".csv"))),
        "single_csv_zip Binance records must identify their CSV member"
    );
    assert!(
        inputs
            .records
            .iter()
            .filter(|record| record.selected_object_bytes > 30_000_000)
            .all(|record| record.max_decoded_bytes == 1_073_741_824),
        "large accepted Binance native-trades objects need the 1 GiB decoded CSV cap used by venue-scale retries"
    );
    assert!(
        inputs
            .records
            .iter()
            .filter(|record| record.status == SourceUniverseOperatorInputRecordStatus::Blocked)
            .all(|record| record.blocking_reasons == ["missing_instrument_metadata"]),
        "blocked Binance records must only reflect missing metadata, not converter/gate gaps"
    );
}
