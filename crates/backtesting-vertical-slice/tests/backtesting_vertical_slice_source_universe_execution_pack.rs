use std::{fs, path::Path};

use crate::backtesting_vertical_slice_test_support::tempdir_in_repo_target;

use backtesting_vertical_slice::{
    backfill_accepted_tranche::BackfillAcceptedTrancheManifest,
    backfill_execution_plan::{BackfillExecutionPlan, BackfillExecutionPlanStatus},
    catalog_projection::CatalogInstrumentSpec,
    operator::{
        RunSpec, VerifiedSourceBindingRegistry, validate_durable_run_spec_preflight,
        validate_run_spec_manifest_for_object_hash_with_verified_registry,
    },
    operator_work_budget::OperatorWorkBudgetGuard,
    source_universe_conversion_work_order::SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION,
    source_universe_execution_pack::{
        SourceUniverseExecutionPack, SourceUniverseExecutionPackStatus,
        write_source_universe_execution_pack_from_spec_file,
    },
    source_universe_operator_inputs::SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION,
};
use serde_json::json;

fn empty_operator_inputs(schema_version: &str) -> serde_json::Value {
    json!({
        "schema_version": schema_version,
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
        "raw_payload_container": "csv_gzip",
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
    })
}

fn empty_work_order(schema_version: &str, artifact_refs: serde_json::Value) -> serde_json::Value {
    json!({
        "schema_version": schema_version,
        "work_order_id": "source-universe-conversion-work-order-binance-test",
        "status": "ready",
        "input_id": "source-universe-operator-inputs-binance-test",
        "gate_id": "source-universe-object-gates-binance-test",
        "conversion_run_plan_id": "source-universe-conversion-run-plan-binance-test",
        "universe_id": "backfill-source-universe-binance-test",
        "venue": "binance",
        "source": "data_vision",
        "family": "trades",
        "table_family": "trades",
        "operator_run_id_prefix": "source-universe-operator-run-binance-test",
        "planned_object_count": 0,
        "planned_source_bytes": 0,
        "operator_input_count": 0,
        "ready_input_count": 0,
        "blocked_input_count": 0,
        "conversion_run_count": 0,
        "executable_record_count": 0,
        "withheld_record_count": 0,
        "executable_source_bytes": 0,
        "withheld_source_bytes": 0,
        "artifact_refs": artifact_refs,
        "records": [],
        "withheld_records": [],
        "blocking_reasons": []
    })
}

fn write_schema_probe_spec(spec_path: &Path, work_order_path: &Path, output_dir: &Path) {
    fs::write(
        spec_path,
        format!(
            r#"pack_id = "source-universe-execution-pack-schema-probe"
source_universe_conversion_work_order_path = "{}"
run_spec_template_path = "unused-for-schema-rejection.toml"
output_dir = "{}"

[venue_policy]
starting_balance_amount = "100000"
spot = "CASH"
crypto_perpetual = "MARGIN"
crypto_future = "MARGIN"
"#,
            work_order_path.display(),
            output_dir.display(),
        ),
    )
    .expect("write schema probe spec");
}

#[test]
fn source_universe_execution_pack_rejects_unsupported_work_order_schema() {
    let temp_dir = tempdir_in_repo_target();
    let work_order_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.json");
    let output_dir = temp_dir.path().join("execution-pack");
    let spec_path = temp_dir.path().join("source-universe-execution-pack.toml");
    let unsupported_schema_version =
        format!("{SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION}.unsupported");

    fs::write(
        &work_order_path,
        serde_json::to_vec_pretty(&empty_work_order(&unsupported_schema_version, json!([])))
            .expect("serialize work order with unsupported schema"),
    )
    .expect("write work order with unsupported schema");
    write_schema_probe_spec(&spec_path, &work_order_path, &output_dir);

    let error = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("an unsupported work-order schema must fail closed");

    assert!(
        error
            .to_string()
            .contains("source-universe conversion work-order schema_version mismatch"),
        "{error:#}"
    );
    assert!(
        !output_dir.exists(),
        "schema rejection precedes publication"
    );
}

#[test]
fn source_universe_execution_pack_rejects_unsupported_operator_input_schema() {
    let temp_dir = tempdir_in_repo_target();
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let work_order_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.json");
    let output_dir = temp_dir.path().join("execution-pack");
    let spec_path = temp_dir.path().join("source-universe-execution-pack.toml");
    let unsupported_schema_version =
        format!("{SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION}.unsupported");

    fs::write(
        &operator_inputs_path,
        serde_json::to_vec_pretty(&empty_operator_inputs(&unsupported_schema_version))
            .expect("serialize operator inputs with unsupported schema"),
    )
    .expect("write operator inputs with unsupported schema");
    fs::write(
        &work_order_path,
        serde_json::to_vec_pretty(&empty_work_order(
            SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION,
            json!([{
                "role": "source_universe_operator_inputs",
                "path": operator_inputs_path.display().to_string(),
                "sha256": sha256_file(&operator_inputs_path)
            }]),
        ))
        .expect("serialize work order"),
    )
    .expect("write work order");
    write_schema_probe_spec(&spec_path, &work_order_path, &output_dir);

    let error = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("an unsupported operator-input schema must fail closed");

    assert!(
        error
            .to_string()
            .contains("source-universe operator inputs schema_version mismatch"),
        "{error:#}"
    );
    assert!(
        !output_dir.exists(),
        "schema rejection precedes publication"
    );
}

#[test]
fn source_universe_execution_pack_materializes_operator_ready_record_inputs() {
    let temp_dir = tempdir_in_repo_target();
    let template_path = temp_dir.path().join("run-spec-template.toml");
    let source_proof_path = temp_dir.path().join("source-proof.json");
    let object_gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let work_order_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.json");
    let output_dir = temp_dir.path().join("execution-pack");
    let spec_path = temp_dir.path().join("source-universe-execution-pack.toml");

    let template = run_spec_template();
    fs::write(&template_path, &template).expect("write template");
    let template_spec: RunSpec = toml::from_str(&template).expect("template parses");
    let mut source_proof = template_spec.source_proof.clone();
    source_proof.accepted_by = Some("source-proof-operator".to_string());
    source_proof.accepted_at = Some("2026-06-10T00:00:00Z".to_string());
    fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&source_proof).expect("serialize proof"),
    )
    .expect("write source proof");

    fs::write(
        &object_gates_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "source-universe-object-gates.v1",
            "gate_id": "source-universe-object-gates-binance-test",
            "status": "ready",
            "queue_id": "source-universe-conversion-queue-binance-test",
            "manifest_id": "source-universe-manifest-binance-test",
            "universe_id": "backfill-source-universe-binance-test",
            "venue": "binance",
            "source": "data_vision",
            "family": "trades",
            "table_family": "trades",
            "queue_path": "queue.json",
            "queue_hash": "queue-hash",
            "work_item_count": 1,
            "accepted_gate_count": 1,
            "source_binding_count": 1,
            "total_accepted_bytes": 100,
            "source_binding_summaries": [],
            "artifact_refs": [
                {
                    "role": "source_proof",
                    "path": source_proof_path,
                    "sha256": sha256_file(&source_proof_path)
                }
            ],
            "records": []
        }))
        .expect("serialize gates"),
    )
    .expect("write object gates");

    fs::write(
        &operator_inputs_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION,
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
            "raw_payload_container": "csv_gzip",
            "max_decoded_bytes": 268435456,
            "max_source_rows": 1000000,
            "max_projected_row_groups": 128,
            "max_wall_seconds": 1800,
            "planned_object_count": 1,
            "planned_source_bytes": 100,
            "conversion_run_count": 1,
            "instrument_spec_count": 1,
            "converter_mapping_count": 1,
            "ready_input_count": 1,
            "blocked_input_count": 0,
            "artifact_refs": [
                {
                    "role": "source_universe_object_gates",
                    "path": object_gates_path,
                    "sha256": sha256_file(&object_gates_path)
                }
            ],
            "converter_mappings": [],
            "instrument_specs": [
                {
                    "instrument_key": "binance-spot-native-trades:spot:BTCUSDT",
                    "source_binding": "binance-spot-native-trades",
                    "category": "spot",
                    "symbol": "BTCUSDT",
                    "nt_instrument_id": "BTCUSDT.BINANCE",
                    "metadata_source_uri": "s3://example/metadata/binance-spot.json",
                    "instrument_spec": crypto_perpetual_instrument_spec_json()
                }
            ],
            "records": [
                {
                    "work_item_id": "binance:BTCUSDT:2026-03-01:object-sha",
                    "status": "ready",
                    "operator_run_id": "source-universe-operator-run-binance-test-00000",
                    "source_binding": "binance-spot-native-trades",
                    "category": "spot",
                    "symbol": "BTCUSDT",
                    "archive_date": "2026-03-01",
                    "source_uri": "s3://example/raw/object-sha.csv.gz",
                    "source_url": "https://data.example.invalid/BTCUSDT.csv.gz",
                    "selected_object_sha256": "object-sha",
                    "selected_object_bytes": 100,
                    "source_proof_id": template_spec.source_proof.source_proof_id,
                    "source_proof_version": template_spec.source_proof.source_proof_version,
                    "accepted_tranche_id": "tranche-object-sha",
                    "output_prefix": "source-universe=synthetic/object=object-sha",
                    "instrument_key": "binance-spot-native-trades:spot:BTCUSDT",
                    "converter_identity": "csv-native-trades-to-canonical-trades.v1",
                    "converter_version": "1",
                    "raw_payload_container": "csv_gzip",
                    "max_decoded_bytes": 268435456,
                    "max_source_rows": 1000000,
                    "max_projected_row_groups": 128,
                    "max_wall_seconds": 1800,
                    "schema_columns": ["id", "timestamp", "price", "volume", "side"],
                    "converter_csv": template_spec.converter.csv,
                    "blocking_reasons": []
                }
            ],
            "blocking_reasons": []
        }))
        .expect("serialize operator inputs"),
    )
    .expect("write operator inputs");

    fs::write(
        &work_order_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION,
            "work_order_id": "source-universe-conversion-work-order-binance-test",
            "status": "ready",
            "input_id": "source-universe-operator-inputs-binance-test",
            "gate_id": "source-universe-object-gates-binance-test",
            "conversion_run_plan_id": "source-universe-conversion-run-plan-binance-test",
            "universe_id": "backfill-source-universe-binance-test",
            "venue": "binance",
            "source": "data_vision",
            "family": "trades",
            "table_family": "trades",
            "operator_run_id_prefix": "source-universe-operator-run-binance-test",
            "planned_object_count": 1,
            "planned_source_bytes": 100,
            "operator_input_count": 1,
            "ready_input_count": 1,
            "blocked_input_count": 0,
            "conversion_run_count": 1,
            "executable_record_count": 1,
            "withheld_record_count": 0,
            "executable_source_bytes": 100,
            "withheld_source_bytes": 0,
            "artifact_refs": [
                {
                    "role": "source_universe_operator_inputs",
                    "path": operator_inputs_path,
                    "sha256": sha256_file(&operator_inputs_path)
                }
            ],
            "records": [
                {
                    "sequence": 0,
                    "work_item_id": "binance:BTCUSDT:2026-03-01:object-sha",
                    "operator_run_id": "source-universe-operator-run-binance-test-00000",
                    "source_binding": "binance-spot-native-trades",
                    "category": "spot",
                    "symbol": "BTCUSDT",
                    "archive_date": "2026-03-01",
                    "source_uri": "s3://example/raw/object-sha.csv.gz",
                    "source_url": "https://data.example.invalid/BTCUSDT.csv.gz",
                    "selected_object_sha256": "object-sha",
                    "selected_object_bytes": 100,
                    "source_proof_id": template_spec.source_proof.source_proof_id,
                    "source_proof_version": template_spec.source_proof.source_proof_version,
                    "accepted_tranche_id": "tranche-object-sha",
                    "output_prefix": "source-universe=synthetic/object=object-sha",
                    "instrument_key": "binance-spot-native-trades:spot:BTCUSDT",
                    "converter_identity": "csv-native-trades-to-canonical-trades.v1",
                    "converter_version": "1",
                    "raw_payload_container": "csv_gzip",
                    "max_decoded_bytes": 268435456,
                    "max_source_rows": 1000000,
                    "max_projected_row_groups": 128,
                    "max_wall_seconds": 1800
                }
            ],
            "withheld_records": [],
            "blocking_reasons": []
        }))
        .expect("serialize work order"),
    )
    .expect("write work order");

    fs::write(
        &spec_path,
        format!(
            r#"pack_id = "source-universe-execution-pack-binance-test"
source_universe_conversion_work_order_path = "{}"
run_spec_template_path = "{}"
output_dir = "{}"
record_limit = 1

[venue_policy]
starting_balance_amount = "250000"
spot = "CASH"
crypto_perpetual = "CASH"
crypto_future = "MARGIN"
"#,
            work_order_path.display(),
            template_path.display(),
            output_dir.display(),
        ),
    )
    .expect("write spec");

    let artifact = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect("execution pack write succeeds");
    let initial_pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read execution pack"))
            .expect("execution pack parses");
    let initial_record = &initial_pack.records[0];
    let initial_run_spec_text =
        fs::read_to_string(resolve_repo_relative(&initial_record.run_spec_path))
            .expect("read initial run spec");
    let initial_run_spec: RunSpec =
        toml::from_str(&initial_run_spec_text).expect("initial run spec parses");
    assert_eq!(initial_run_spec.manifest.venue.account_type, "CASH");
    let initial_execution_plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(resolve_repo_relative(&initial_record.execution_plan_path))
            .expect("read initial plan"),
    )
    .expect("initial execution plan parses");

    fs::write(
        &spec_path,
        format!(
            r#"pack_id = "source-universe-execution-pack-binance-test"
source_universe_conversion_work_order_path = "{}"
run_spec_template_path = "{}"
output_dir = "{}"
record_limit = 1
overwrite_existing_artifacts = true

[venue_policy]
starting_balance_amount = "250000"
spot = "CASH"
crypto_perpetual = "MARGIN"
crypto_future = "MARGIN"
"#,
            work_order_path.display(),
            template_path.display(),
            output_dir.display(),
        ),
    )
    .expect("write overwrite spec");

    let prior_pack_bytes = fs::read(&artifact.path).expect("read pack before stale-set probe");
    let stale_top_level = output_dir.join("stale-top-level-control.json");
    fs::write(&stale_top_level, b"{}").expect("plant stale top-level pack control");
    let error = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("pilot regeneration must reject a stale top-level output");
    assert!(
        error.to_string().contains("stale or unexpected entry"),
        "{error:#}"
    );
    assert_eq!(
        fs::read(&artifact.path).expect("read pack after top-level exact-set rejection"),
        prior_pack_bytes,
        "top-level exact-set rejection must precede output mutation"
    );
    fs::remove_file(&stale_top_level).expect("remove planted top-level output");

    let stale_run_dir = output_dir.join("runs/00001-stale-full-pack-record");
    fs::create_dir(&stale_run_dir).expect("plant stale run from a prior wider pack");
    let error = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("pilot regeneration must reject a stale non-selected run directory");
    assert!(
        error.to_string().contains("stale or unexpected entry"),
        "{error:#}"
    );
    assert_eq!(
        fs::read(&artifact.path).expect("read pack after stale-set rejection"),
        prior_pack_bytes,
        "exact-set rejection must precede output mutation"
    );
    fs::remove_dir(&stale_run_dir).expect("remove planted stale run directory");

    let retained_run_dir = resolve_repo_relative(&initial_record.run_spec_path)
        .parent()
        .expect("retained run spec has parent")
        .to_path_buf();
    let stale_control = retained_run_dir.join("stale-control.json");
    fs::write(&stale_control, b"{}").expect("plant stale control in retained run");
    let error = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("pilot regeneration must reject a stale control in a selected run");
    assert!(
        error.to_string().contains("stale or unexpected entry"),
        "{error:#}"
    );
    assert_eq!(
        fs::read(&artifact.path).expect("read pack after stale-control rejection"),
        prior_pack_bytes,
        "selected-run exact-set rejection must precede output mutation"
    );
    fs::remove_file(&stale_control).expect("remove planted stale control");

    let artifact = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect("execution pack overwrite succeeds");
    let pack: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&artifact.path).expect("read refreshed execution pack"))
            .expect("refreshed execution pack parses");

    assert_eq!(pack.status, SourceUniverseExecutionPackStatus::Ready);
    assert_eq!(pack.materialized_record_count, 1);
    assert_eq!(pack.records.len(), 1);

    let record = &pack.records[0];
    let run_spec_path = resolve_repo_relative(&record.run_spec_path);
    let run_spec_text = fs::read_to_string(&run_spec_path).expect("read run spec");
    let run_spec: RunSpec = toml::from_str(&run_spec_text).expect("run spec parses");
    assert_eq!(run_spec.manifest.run_id, record.operator_run_id);
    assert_eq!(
        run_spec.manifest.output_prefix,
        "s3://synthetic-artifacts/backtests/source-universe=synthetic/object=object-sha"
    );
    assert_eq!(run_spec.accepted_object.sha256, "object-sha");
    assert_eq!(
        run_spec.accepted_object.s3_uri,
        "s3://example/raw/object-sha.csv.gz"
    );
    assert_eq!(run_spec.source_proof.raw_sample_hash, "object-sha");
    assert_eq!(run_spec.accepted_by, "source-proof-operator");
    assert_eq!(run_spec.accepted_at_utc, "2026-06-10T00:00:00Z");
    assert_eq!(
        run_spec.source_proof.accepted_by.as_deref(),
        Some(run_spec.accepted_by.as_str())
    );
    assert_eq!(
        run_spec.source_proof.accepted_at.as_deref(),
        Some(run_spec.accepted_at_utc.as_str())
    );
    assert_eq!(run_spec.converter.raw_payload.max_object_bytes, 100);
    let identity = run_spec
        .identity
        .single()
        .expect("execution-pack run-specs carry a single instrument identity");
    assert_eq!(identity.instrument_id, "BTCUSDT");
    let instrument_spec = run_spec
        .instrument_spec
        .single()
        .expect("execution-pack run-specs carry a single instrument spec");
    assert!(matches!(
        instrument_spec,
        CatalogInstrumentSpec::CryptoPerpetual(_)
    ));
    assert_eq!(run_spec.manifest.venue.account_type, "MARGIN");
    assert_eq!(run_spec.manifest.venue.starting_balances, ["250000 BTC"]);
    assert_composed_operator_preflight(
        &run_spec,
        run_spec_path
            .parent()
            .expect("materialized run spec has parent"),
    );
    let catalog_dispatch = run_spec
        .required_catalog_dispatch()
        .expect("materialized run spec retains catalog dispatch");
    assert_eq!(catalog_dispatch.bindings.len(), 1);
    assert_eq!(
        catalog_dispatch.bindings[0].source_binding,
        record.source_binding
    );
    assert_eq!(
        catalog_dispatch.bindings[0].market_structure_fixture,
        run_spec.manifest.market_structure_fixture
    );
    assert_eq!(
        catalog_dispatch.bindings[0].catalog_projection_id,
        record.operator_run_id
    );

    let execution_plan: BackfillExecutionPlan = serde_json::from_slice(
        &fs::read(resolve_repo_relative(&record.execution_plan_path)).expect("read plan"),
    )
    .expect("execution plan parses");
    assert_ne!(
        execution_plan.run_spec_hash,
        initial_execution_plan.run_spec_hash
    );
    assert_eq!(execution_plan.status, BackfillExecutionPlanStatus::Ready);
    assert_eq!(execution_plan.operator_run_id, record.operator_run_id);
    assert_eq!(execution_plan.objects.len(), 1);
    assert_eq!(execution_plan.objects[0].sha256, "object-sha");

    let mismatched_template_path = temp_dir.path().join("mismatched-run-spec-profile.toml");
    let mismatched_output_dir = temp_dir.path().join("mismatched-execution-pack");
    let mismatched_spec_path = temp_dir
        .path()
        .join("mismatched-source-universe-execution-pack.toml");
    let mismatched_template = template.replacen(
        "[artifact_store]\nartifact_root = \"s3://synthetic-artifacts\"",
        "[artifact_store]\nartifact_root = \"s3://different-artifacts\"",
        1,
    );
    fs::write(&mismatched_template_path, mismatched_template)
        .expect("write mismatched durable profile");
    fs::write(
        &mismatched_spec_path,
        format!(
            r#"pack_id = "source-universe-execution-pack-store-mismatch"
source_universe_conversion_work_order_path = "{}"
run_spec_template_path = "{}"
output_dir = "{}"
record_limit = 1

[venue_policy]
starting_balance_amount = "250000"
spot = "CASH"
crypto_perpetual = "MARGIN"
crypto_future = "MARGIN"
"#,
            work_order_path.display(),
            mismatched_template_path.display(),
            mismatched_output_dir.display(),
        ),
    )
    .expect("write mismatched execution-pack spec");

    let error = write_source_universe_execution_pack_from_spec_file(&mismatched_spec_path)
        .expect_err("artifact-store/profile mismatch fails closed");
    assert!(
        format!("{error:#}").contains("run spec artifact-store root mismatch"),
        "{error:#}"
    );
    assert!(
        !mismatched_output_dir.exists(),
        "durable profile validation precedes execution-pack output"
    );
}

#[test]
fn committed_bybit_and_binance_source_universe_execution_packs_track_materialized_scope() {
    let reference_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../specs/023-nt-research-analytics-platform/reference");

    let bybit_pack_path = reference_root
        .join("source-universe-execution-packs/bybit-public-archive-tick-trades-2025-06-01-2026-06-01")
        .join("execution-pack/source-universe-execution-pack.json");
    let bybit: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&bybit_pack_path).expect("read Bybit execution pack"))
            .expect("Bybit execution pack parses");
    assert_eq!(
        bybit.status,
        SourceUniverseExecutionPackStatus::PartiallyReady
    );
    assert_eq!(bybit.planned_object_count, 5_857);
    assert_eq!(bybit.executable_record_count, 5_857);
    assert_eq!(bybit.withheld_record_count, 0);
    assert_eq!(bybit.selected_record_count, 1);
    assert_eq!(bybit.materialized_record_count, 1);
    assert_eq!(bybit.skipped_executable_record_count, 5_856);
    assert_eq!(bybit.materialized_source_bytes, 124_717);
    assert_eq!(bybit.records.len(), 1);
    assert_eq!(
        bybit.blocking_reasons,
        ["record_limit_skipped_executable_records"]
    );
    assert_eq!(
        bybit.records[0].accepted_tranche_sha256,
        "1d300ad208a5ce6447c132e8795d222f6394f4529fb1dcb35d9f1af957dcd084"
    );
    assert_profile_ref(
        &bybit,
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml",
    );
    assert_declared_record_artifacts_parse(&bybit);
    let bybit_run_spec = read_declared_run_spec(&bybit.records[0]);
    assert_eq!(
        bybit_run_spec.manifest.venue.starting_balances,
        ["1_000_000 AAVE"]
    );

    let binance_pack_path = reference_root
        .join(
            "source-universe-execution-packs/binance-data-vision-trades-2026-03-01-all-instruments",
        )
        .join("execution-pack/source-universe-execution-pack.json");
    let binance: SourceUniverseExecutionPack =
        serde_json::from_slice(&fs::read(&binance_pack_path).expect("read Binance execution pack"))
            .expect("Binance execution pack parses");
    assert_eq!(
        binance.status,
        SourceUniverseExecutionPackStatus::PartiallyReady
    );
    assert_eq!(binance.planned_object_count, 2_051);
    assert_eq!(binance.executable_record_count, 2_035);
    assert_eq!(binance.withheld_record_count, 16);
    assert_eq!(binance.selected_record_count, 1);
    assert_eq!(binance.materialized_record_count, 1);
    assert_eq!(binance.skipped_executable_record_count, 2_034);
    assert_eq!(binance.materialized_source_bytes, 32_220);
    assert_eq!(binance.records.len(), 1);
    assert_eq!(
        binance.blocking_reasons,
        [
            "blocked_operator_input_records",
            "missing_instrument_metadata",
            "ready_operator_inputs_do_not_cover_all_object_gates",
            "record_limit_skipped_executable_records",
        ]
    );
    assert!(binance.records.iter().all(|record| {
        record
            .output_prefix
            .starts_with("s3://bolt-parquet/nt-research-analytics/backtests/")
    }));
    assert_eq!(
        binance.records[0].accepted_tranche_sha256,
        "7bdbaf9f0e73ded80eca6100819f42067f07142b88030bbf043d80c1ef3cb021"
    );
    assert_profile_ref(
        &binance,
        "specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.binance-bnbusdc-2026-03-01.toml",
    );
    assert_declared_record_artifacts_parse(&binance);
    let binance_run_spec = read_declared_run_spec(&binance.records[0]);
    assert_eq!(
        binance_run_spec.manifest.venue.starting_balances,
        ["1_000_000 TRY"]
    );
    assert_ne!(
        bybit_run_spec
            .required_catalog_dispatch()
            .expect("Bybit pilot dispatch")
            .bindings[0]
            .source_binding,
        binance_run_spec
            .required_catalog_dispatch()
            .expect("Binance pilot dispatch")
            .bindings[0]
            .source_binding,
        "the committed pilots retain distinct record-owned dispatch"
    );
}

fn assert_declared_record_artifacts_parse(pack: &SourceUniverseExecutionPack) {
    let profile_refs = pack
        .artifact_refs
        .iter()
        .filter(|artifact| artifact.role == "run_spec_template")
        .collect::<Vec<_>>();
    assert_eq!(profile_refs.len(), 1, "pack retains one active profile");
    let profile_path = resolve_repo_relative(&profile_refs[0].path);
    let profile_bytes = fs::read(&profile_path).expect("read active pack profile");
    assert_eq!(sha256_bytes(&profile_bytes), profile_refs[0].sha256);
    let profile: RunSpec =
        toml::from_str(std::str::from_utf8(&profile_bytes).expect("active pack profile is UTF-8"))
            .expect("active pack profile parses as the current RunSpec schema");
    assert_composed_operator_preflight(
        &profile,
        profile_path.parent().expect("active profile has parent"),
    );

    let source_bindings_refs = pack
        .artifact_refs
        .iter()
        .filter(|artifact| artifact.role == "source_bindings")
        .collect::<Vec<_>>();
    assert_eq!(
        source_bindings_refs.len(),
        1,
        "pack retains one shared source-bindings artifact ref"
    );
    let source_bindings_ref = source_bindings_refs[0];
    assert!(
        !pack.records.is_empty(),
        "execution pack declares a pilot record"
    );
    for record in &pack.records {
        assert_eq!(record.source_bindings_path, source_bindings_ref.path);
        assert_eq!(record.source_bindings_sha256, source_bindings_ref.sha256);
        assert!(
            record
                .artifact_paths()
                .iter()
                .all(|path| *path != record.source_bindings_path.as_path()),
            "shared source bindings must not be a per-record evictable artifact"
        );

        let run_spec_path = resolve_repo_relative(&record.run_spec_path);
        assert!(run_spec_path.is_file(), "run-spec path is a regular file");
        let run_spec_bytes = fs::read(&run_spec_path).expect("read declared run spec");
        assert_eq!(run_spec_bytes.len() as u64, record.run_spec_bytes);
        assert_eq!(sha256_bytes(&run_spec_bytes), record.run_spec_sha256);
        let run_spec: RunSpec = toml::from_str(
            std::str::from_utf8(&run_spec_bytes).expect("declared run spec is UTF-8"),
        )
        .expect("declared run spec parses");
        assert_eq!(run_spec.manifest.run_id, record.operator_run_id);
        assert_eq!(
            run_spec.accepted_object.sha256,
            record.selected_object_sha256
        );
        assert_composed_operator_preflight(
            &run_spec,
            run_spec_path
                .parent()
                .expect("declared run spec has parent"),
        );
        let catalog_dispatch = run_spec
            .required_catalog_dispatch()
            .expect("declared pilot has catalog dispatch");
        assert_eq!(catalog_dispatch.bindings.len(), 1);
        assert_eq!(
            catalog_dispatch.bindings[0].source_binding,
            record.source_binding
        );
        assert_eq!(
            catalog_dispatch.bindings[0].market_structure_fixture,
            run_spec.manifest.market_structure_fixture
        );
        assert_eq!(
            catalog_dispatch.bindings[0].catalog_projection_id,
            record.operator_run_id
        );
        let (funding_currency, account_type) = match run_spec
            .instrument_spec
            .single()
            .expect("declared pilot has one typed instrument")
        {
            CatalogInstrumentSpec::Spot(spec) => (spec.quote_currency.as_str(), "CASH"),
            CatalogInstrumentSpec::CryptoPerpetual(spec) => {
                (spec.settlement_currency.as_str(), "MARGIN")
            }
            CatalogInstrumentSpec::CryptoFuture(spec) => {
                (spec.settlement_currency.as_str(), "MARGIN")
            }
            CatalogInstrumentSpec::BinaryOption(_) => {
                panic!("source-universe execution packs reject binary options")
            }
        };
        assert_eq!(run_spec.manifest.venue.account_type, account_type);
        assert_eq!(
            run_spec.manifest.venue.starting_balances,
            [format!("1_000_000 {funding_currency}")]
        );

        let accepted_tranche_path = resolve_repo_relative(&record.accepted_tranche_path);
        assert!(
            accepted_tranche_path.is_file(),
            "accepted-tranche path is a regular file"
        );
        let accepted_tranche_bytes =
            fs::read(&accepted_tranche_path).expect("read declared accepted tranche");
        assert_eq!(
            accepted_tranche_bytes.len() as u64,
            record.accepted_tranche_bytes
        );
        assert_eq!(
            sha256_bytes(&accepted_tranche_bytes),
            record.accepted_tranche_sha256
        );
        let accepted_tranche: BackfillAcceptedTrancheManifest =
            serde_json::from_slice(&accepted_tranche_bytes)
                .expect("declared accepted tranche parses");
        assert_eq!(accepted_tranche.tranche_id, record.accepted_tranche_id);
        assert_eq!(accepted_tranche.source_proof_id, record.source_proof_id);

        let execution_plan_path = resolve_repo_relative(&record.execution_plan_path);
        assert!(
            execution_plan_path.is_file(),
            "execution-plan path is a regular file"
        );
        let execution_plan_bytes =
            fs::read(&execution_plan_path).expect("read declared execution plan");
        assert_eq!(
            execution_plan_bytes.len() as u64,
            record.execution_plan_bytes
        );
        assert_eq!(
            sha256_bytes(&execution_plan_bytes),
            record.execution_plan_sha256
        );
        let execution_plan: BackfillExecutionPlan =
            serde_json::from_slice(&execution_plan_bytes).expect("declared execution plan parses");
        assert_eq!(execution_plan.status, BackfillExecutionPlanStatus::Ready);
        assert_eq!(execution_plan.operator_run_id, record.operator_run_id);
        assert_eq!(
            execution_plan.objects[0].sha256,
            record.selected_object_sha256
        );
    }
}

fn assert_composed_operator_preflight(spec: &RunSpec, output_dir: &Path) {
    let mut executable_spec = spec.clone();
    executable_spec.source_bindings_path = resolve_repo_relative(&spec.source_bindings_path);
    assert!(
        executable_spec.source_bindings_path.is_file(),
        "composed preflight resolves the declared source-binding registry to one regular file"
    );
    let registry = VerifiedSourceBindingRegistry::from_run_spec_guarded(
        &executable_spec,
        &OperatorWorkBudgetGuard::unbounded(),
    )
    .expect("active RunSpec resolves its sole source-binding registry");
    validate_run_spec_manifest_for_object_hash_with_verified_registry(
        &executable_spec,
        output_dir,
        &executable_spec.accepted_object.sha256,
        &registry,
    )
    .expect("active RunSpec passes ordinary manifest/source-binding preflight");
    validate_durable_run_spec_preflight(&executable_spec)
        .expect("active RunSpec passes durable-only operator preflight");
}

fn assert_profile_ref(pack: &SourceUniverseExecutionPack, expected_path: &str) {
    let profile_refs = pack
        .artifact_refs
        .iter()
        .filter(|artifact| artifact.role == "run_spec_template")
        .collect::<Vec<_>>();
    assert_eq!(profile_refs.len(), 1);
    assert_eq!(profile_refs[0].path, Path::new(expected_path));
}

fn read_declared_run_spec(
    record: &backtesting_vertical_slice::source_universe_execution_pack::SourceUniverseExecutionPackRecord,
) -> RunSpec {
    let bytes = fs::read(resolve_repo_relative(&record.run_spec_path))
        .expect("read declared pilot run spec");
    toml::from_str(std::str::from_utf8(&bytes).expect("pilot run spec is UTF-8"))
        .expect("pilot run spec parses")
}

fn resolve_repo_relative(path: &Path) -> std::path::PathBuf {
    if path.is_absolute() || path.exists() {
        return path.to_path_buf();
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path)
}

fn sha256_file(path: &Path) -> String {
    sha256_bytes(&fs::read(path).expect("read file for hash"))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    format!("{:x}", Sha256::digest(bytes))
}

// Fix 2 — negative test for the BinaryOption bail arm in account_type_for.
// The source-universe execution pack is wired for crypto-venue instrument
// families (spot / perp / future) only. A BinaryOption spec reaching
// account_type_for is a contract violation; the function must bail with an
// informative error rather than silently falling through to a spot account type.
#[test]
fn execution_pack_rejects_binary_option_instrument_spec() {
    let temp_dir = tempdir_in_repo_target();
    let template_path = temp_dir.path().join("run-spec-template.toml");
    let source_proof_path = temp_dir.path().join("source-proof.json");
    let object_gates_path = temp_dir.path().join("source-universe-object-gates.json");
    let operator_inputs_path = temp_dir.path().join("source-universe-operator-inputs.json");
    let work_order_path = temp_dir
        .path()
        .join("source-universe-conversion-work-order.json");
    let output_dir = temp_dir.path().join("execution-pack");
    let spec_path = temp_dir.path().join("source-universe-execution-pack.toml");

    let template = run_spec_template();
    fs::write(&template_path, &template).expect("write template");
    let template_spec: backtesting_vertical_slice::operator::RunSpec =
        toml::from_str(&template).expect("template parses");
    let mut source_proof = template_spec.source_proof.clone();
    source_proof.accepted_by = Some("source-proof-operator".to_string());
    source_proof.accepted_at = Some("2026-06-10T00:00:00Z".to_string());
    fs::write(
        &source_proof_path,
        serde_json::to_vec_pretty(&source_proof).expect("serialize proof"),
    )
    .expect("write source proof");

    fs::write(
        &object_gates_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": "source-universe-object-gates.v1",
            "gate_id": "source-universe-object-gates-binopt-test",
            "status": "ready",
            "queue_id": "source-universe-conversion-queue-binopt-test",
            "manifest_id": "source-universe-manifest-binopt-test",
            "universe_id": "backfill-source-universe-binopt-test",
            "venue": "testvenue",
            "source": "data_vision",
            "family": "trades",
            "table_family": "trades",
            "queue_path": "queue.json",
            "queue_hash": "queue-hash",
            "work_item_count": 1,
            "accepted_gate_count": 1,
            "source_binding_count": 1,
            "total_accepted_bytes": 100,
            "source_binding_summaries": [],
            "artifact_refs": [
                {
                    "role": "source_proof",
                    "path": source_proof_path,
                    "sha256": sha256_file(&source_proof_path)
                }
            ],
            "records": []
        }))
        .expect("serialize gates"),
    )
    .expect("write object gates");

    fs::write(
        &operator_inputs_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": SOURCE_UNIVERSE_OPERATOR_INPUTS_SCHEMA_VERSION,
            "input_id": "source-universe-operator-inputs-binopt-test",
            "status": "ready",
            "gate_id": "source-universe-object-gates-binopt-test",
            "conversion_run_plan_id": "source-universe-conversion-run-plan-binopt-test",
            "universe_id": "backfill-source-universe-binopt-test",
            "venue": "testvenue",
            "source": "data_vision",
            "family": "trades",
            "table_family": "trades",
            "operator_run_id_prefix": "source-universe-operator-run-binopt-test",
            "nt_venue": "TESTVENUE",
            "converter_identity": "csv-native-trades-to-canonical-trades.v1",
            "converter_version": "1",
            "raw_payload_container": "csv_gzip",
            "max_decoded_bytes": 268435456,
            "max_source_rows": 1000000,
            "max_projected_row_groups": 128,
            "max_wall_seconds": 1800,
            "planned_object_count": 1,
            "planned_source_bytes": 100,
            "conversion_run_count": 1,
            "instrument_spec_count": 1,
            "converter_mapping_count": 1,
            "ready_input_count": 1,
            "blocked_input_count": 0,
            "artifact_refs": [
                {
                    "role": "source_universe_object_gates",
                    "path": object_gates_path,
                    "sha256": sha256_file(&object_gates_path)
                }
            ],
            "converter_mappings": [],
            "instrument_specs": [
                {
                    "instrument_key": "testvenue-prediction-market:binary_option:YES",
                    "source_binding": "testvenue-prediction-market",
                    "category": "binary_option",
                    "symbol": "YES",
                    "nt_instrument_id": "YES.TESTVENUE",
                    "metadata_source_uri": "s3://example/metadata/testvenue.json",
                    "instrument_spec": binary_option_instrument_spec_json()
                }
            ],
            "records": [
                {
                    "work_item_id": "testvenue:YES:2026-03-01:object-sha",
                    "status": "ready",
                    "operator_run_id": "source-universe-operator-run-binopt-test-00000",
                    "source_binding": "testvenue-prediction-market",
                    "category": "binary_option",
                    "symbol": "YES",
                    "archive_date": "2026-03-01",
                    "source_uri": "s3://example/raw/object-sha.csv.gz",
                    "source_url": "https://data.example.invalid/YES.csv.gz",
                    "selected_object_sha256": "object-sha",
                    "selected_object_bytes": 100,
                    "source_proof_id": template_spec.source_proof.source_proof_id,
                    "source_proof_version": template_spec.source_proof.source_proof_version,
                    "accepted_tranche_id": "tranche-object-sha",
                    "output_prefix": "source-universe=synthetic/object=object-sha",
                    "instrument_key": "testvenue-prediction-market:binary_option:YES",
                    "converter_identity": "csv-native-trades-to-canonical-trades.v1",
                    "converter_version": "1",
                    "raw_payload_container": "csv_gzip",
                    "max_decoded_bytes": 268435456,
                    "max_source_rows": 1000000,
                    "max_projected_row_groups": 128,
                    "max_wall_seconds": 1800,
                    "schema_columns": ["id", "timestamp", "price", "volume", "side"],
                    "converter_csv": template_spec.converter.csv,
                    "blocking_reasons": []
                }
            ],
            "blocking_reasons": []
        }))
        .expect("serialize operator inputs"),
    )
    .expect("write operator inputs");

    fs::write(
        &work_order_path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": SOURCE_UNIVERSE_CONVERSION_WORK_ORDER_SCHEMA_VERSION,
            "work_order_id": "source-universe-conversion-work-order-binopt-test",
            "status": "ready",
            "input_id": "source-universe-operator-inputs-binopt-test",
            "gate_id": "source-universe-object-gates-binopt-test",
            "conversion_run_plan_id": "source-universe-conversion-run-plan-binopt-test",
            "universe_id": "backfill-source-universe-binopt-test",
            "venue": "testvenue",
            "source": "data_vision",
            "family": "trades",
            "table_family": "trades",
            "operator_run_id_prefix": "source-universe-operator-run-binopt-test",
            "planned_object_count": 1,
            "planned_source_bytes": 100,
            "operator_input_count": 1,
            "ready_input_count": 1,
            "blocked_input_count": 0,
            "conversion_run_count": 1,
            "executable_record_count": 1,
            "withheld_record_count": 0,
            "executable_source_bytes": 100,
            "withheld_source_bytes": 0,
            "artifact_refs": [
                {
                    "role": "source_universe_operator_inputs",
                    "path": operator_inputs_path,
                    "sha256": sha256_file(&operator_inputs_path)
                }
            ],
            "records": [
                {
                    "sequence": 0,
                    "work_item_id": "testvenue:YES:2026-03-01:object-sha",
                    "operator_run_id": "source-universe-operator-run-binopt-test-00000",
                    "source_binding": "testvenue-prediction-market",
                    "category": "binary_option",
                    "symbol": "YES",
                    "archive_date": "2026-03-01",
                    "source_uri": "s3://example/raw/object-sha.csv.gz",
                    "source_url": "https://data.example.invalid/YES.csv.gz",
                    "selected_object_sha256": "object-sha",
                    "selected_object_bytes": 100,
                    "source_proof_id": template_spec.source_proof.source_proof_id,
                    "source_proof_version": template_spec.source_proof.source_proof_version,
                    "accepted_tranche_id": "tranche-object-sha",
                    "output_prefix": "source-universe=synthetic/object=object-sha",
                    "instrument_key": "testvenue-prediction-market:binary_option:YES",
                    "converter_identity": "csv-native-trades-to-canonical-trades.v1",
                    "converter_version": "1",
                    "raw_payload_container": "csv_gzip",
                    "max_decoded_bytes": 268435456,
                    "max_source_rows": 1000000,
                    "max_projected_row_groups": 128,
                    "max_wall_seconds": 1800
                }
            ],
            "withheld_records": [],
            "blocking_reasons": []
        }))
        .expect("serialize work order"),
    )
    .expect("write work order");

    fs::write(
        &spec_path,
        format!(
            r#"pack_id = "source-universe-execution-pack-binopt-test"
source_universe_conversion_work_order_path = "{}"
run_spec_template_path = "{}"
output_dir = "{}"
record_limit = 1

[venue_policy]
starting_balance_amount = "100000"
spot = "CASH"
crypto_perpetual = "CASH"
crypto_future = "MARGIN"
"#,
            work_order_path.display(),
            template_path.display(),
            output_dir.display(),
        ),
    )
    .expect("write spec");

    let err = write_source_universe_execution_pack_from_spec_file(&spec_path)
        .expect_err("BinaryOption spec must be rejected by the execution pack");
    let msg = err.to_string();
    assert!(
        msg.contains(
            "source-universe execution pack does not support binary-option instrument specs"
        ),
        "error must cite the bail message from account_type_for: {msg}"
    );
}

fn binary_option_instrument_spec_json() -> serde_json::Value {
    json!({
        "instrument_kind": "binary_option",
        "nt_instrument_id": "YES.TESTVENUE",
        "raw_symbol": "YES",
        "asset_class": "ALTERNATIVE",
        "currency": "USDC",
        "activation_time_nanos": 1_700_000_000_000_000_000_u64,
        "expiration_time_nanos": 1_700_086_400_000_000_000_u64,
        "price_increment": "0.01",
        "size_increment": "0.001"
    })
}

fn crypto_perpetual_instrument_spec_json() -> serde_json::Value {
    json!({
        "instrument_kind": "crypto_perpetual",
        "nt_instrument_id": "BTCUSDT.BINANCE",
        "raw_symbol": "BTCUSDT",
        "base_currency": "BTC",
        "quote_currency": "USD",
        "settlement_currency": "BTC",
        "is_inverse": true,
        "price_increment": "0.1",
        "size_increment": "0.0001",
        "min_quantity": "0.0001",
        "max_quantity": "100",
        "min_notional": "5",
        "max_notional": "100000",
        "multiplier": "1",
        "lot_size": "0.0001",
        "max_price": "1000000",
        "min_price": "0.1",
        "margin_init": "0.01",
        "margin_maint": "0.005",
        "maker_fee": "0",
        "taker_fee": "0"
    })
}

fn run_spec_template() -> String {
    r#"
capture_time_utc = "2026-06-02T04:27:02Z"
created_at_utc = "2026-06-02T00:00:00Z"
accepted_by = "synthetic-operator"
accepted_at_utc = "2026-06-02T00:00:00Z"
source_bindings_path = "specs/023-nt-research-analytics-platform/reference/backfill-source-bindings.v1.toml"

[accepted_object]
s3_uri = "s3://synthetic-artifacts/raw/object=stale-object-sha.csv.gz"
source_url = "https://data.example.invalid/stale-object.csv.gz"
sha256 = "stale-object-sha"
bytes = 99
archive_date = "2026-01-01"
schema_columns = ["id", "timestamp", "price", "volume", "side"]

[source_proof]
source_proof_id = "source-proof-binance-spot-native-trades"
source_proof_version = 1
contract_version = "backfill-table-contract.v1"
schema_version = "backfill-source-proof.v1"
status = "accepted"
acceptance_mode = "manual"
accepted_by = "synthetic-operator"
accepted_at = "2026-06-02T00:00:00Z"
source_binding = "binance-spot-native-trades"
venue = "binance"
product_family = "spot"
product_category = "spot"
table_family = "trades"
evidence_state = "owner_archive_backfillable"
source_candidate_class = "official_free"
source_selection_status = "ACCEPTED_LOWER_FIDELITY"
fixture_type = "perps-spot"
instrument_universe_id = "synthetic-instrument-universe"
raw_sample_uri = "s3://synthetic-artifacts/raw/object=stale-object-sha.csv.gz"
raw_sample_hash = "stale-object-sha"
schema_sample_uri = "s3://synthetic-artifacts/manifests/synthetic-manifest.json"
schema_sample_hash = "synthetic-schema-hash"
license_ref = "https://data.example.invalid/license"
license_scope = "public"
retention_ref = "https://data.example.invalid/retention"
cost_ref = "cost://synthetic"
nt_mapping_status = "accepted"
fidelity_class = "TRADE_REPLAY"
gap_policy_id = ""
forbidden_claims = []

[source_proof.l2_replay_evidence]

[source_proof.acceptance_scope]
planned_objects = 1
completed_objects = 1
failed_objects = 0
skipped_objects = 0
accepted_bytes = 99
selector_scope_violations = 0

[source_proof.requested_time_range]
start_utc = "2026-03-01T00:00:00Z"
end_utc = "2026-03-02T00:00:00Z"

[source_proof.coverage_time_range]
start_utc = "2026-03-01T00:00:00Z"
end_utc = "2026-03-02T00:00:00Z"

[source_proof.required_checks.source_access]
outcome = "passed"
evidence_ref = "synthetic source access"

[source_proof.required_checks.license]
outcome = "passed"
evidence_ref = "synthetic license"

[source_proof.required_checks.schema]
outcome = "passed"
evidence_ref = "synthetic schema"

[source_proof.required_checks.time_semantics]
outcome = "passed"
evidence_ref = "synthetic time semantics"

[source_proof.required_checks.instrument_universe]
outcome = "passed"
evidence_ref = "synthetic instrument universe"

[source_proof.required_checks.coverage]
outcome = "passed"
evidence_ref = "synthetic coverage"

[source_proof.required_checks.retention_freshness]
outcome = "passed"
evidence_ref = "synthetic retention"

[source_proof.required_checks.granularity]
outcome = "passed"
evidence_ref = "synthetic granularity"

[source_proof.required_checks.completeness]
outcome = "passed"
evidence_ref = "synthetic completeness"

[source_proof.required_checks.nt_mapping]
outcome = "passed"
evidence_ref = "synthetic NT mapping"

[source_proof.required_checks.cost]
outcome = "passed"
evidence_ref = "synthetic cost"

[source_proof.required_checks.storage]
outcome = "passed"
evidence_ref = "synthetic storage"

[instrument_spec]
nt_instrument_id = "BTCUSDT.BINANCE"
raw_symbol = "BTCUSDT"
base_currency = "BTC"
quote_currency = "USDT"
price_increment = "0.1"
size_increment = "0.0001"
min_quantity = "0.0001"
max_quantity = "100"
min_notional = "5"
max_notional = "100000"

[identity]
instrument_id = "STALE"
venue_symbol = "STALE"
nt_instrument_id = "STALE.BINANCE"

[converter]
identity = "csv-native-trades-to-canonical-trades.v1"
version = "1"

[converter.raw_payload]
container = "csv_gzip"
max_object_bytes = 99
max_decoded_bytes = 4096

[converter.csv]
has_headers = true
trade_id_column = "id"
timestamp_column = "timestamp"
timestamp_unit = "milliseconds"
price_column = "price"
size_column = "volume"
side_column = "side"
buyer_side_values = ["buy"]
seller_side_values = ["sell"]

[manifest]
manifest_schema_version = "backtesting-run-manifest.v2"
run_id = "stale-run"
target_bolt_v2_branch = "main"
target_bolt_v2_ref = "refs/heads/main"
resolved_nt_version = "__NT_REVISION__"
market_structure_fixture = "perps-spot"
venue_binding_key = "binance-spot-native-trades"
run_purpose = "normal"
source_proof_id = "source-proof-binance-spot-native-trades"
source_proof_version = 1
pins_non_latest_proof = false
strategy_config_hash = "a99e8a42bfa6df1f790ccc1a3a2c0a5ea7dd122e3ffab73e685be4132bbef396"
nt_streaming_chunk_size = 128
execution_model = "nt_backtest_node"
artifact_root = "s3://synthetic-artifacts"
output_prefix = "s3://synthetic-artifacts/backtests/stale-run"

[manifest.artifact_store]
storage_options = {}
rust_storage_options = { region = "us-east-1", conditional_put = "etag" }

[manifest.artifact_store.ssm_parameters]
region = "us-east-1"
access_key_id = "/synthetic/artifact-store/s3/access-key-id"
secret_access_key = "/synthetic/artifact-store/s3/secret-access-key"

[[manifest.catalog_inputs]]
catalog_path = "overridden-by-binary-at-runtime"
catalog_fs_protocol = "NONE"
catalog_fs_storage_options = {}
catalog_fs_rust_storage_options = {}
data_type = "TradeTick"
nt_instrument_id = "BTCUSDT.BINANCE"

[manifest.strategy]
source_kind = "compiled_rust_registry"
registry_key = "hurst_vpin_directional"

[manifest.strategy.parameters]
trade_size = "0.01"
bar_type = "BTCUSDT.BINANCE-1-MINUTE-LAST-INTERNAL"

[manifest.venue]
nt_venue = "BINANCE"
oms_type = "NETTING"
account_type = "CASH"
book_type = "L1_MBP"
base_currency = "USDT"
default_leverage = "1"
price_protection_points = 0
starting_balances = ["100000 USDT"]
routing = false
frozen_account = false
reject_stop_orders = true
support_gtd_orders = true
support_contingent_orders = true
use_position_ids = true
use_random_ids = false
use_reduce_only = true
bar_execution = true
bar_adaptive_high_low_ordering = false
trade_execution = true
use_market_order_acks = false
liquidity_consumption = false
allow_cash_borrowing = false
queue_position = false
oto_trigger_mode = "PARTIAL"

[artifact_store]
artifact_root = "s3://synthetic-artifacts"
max_final_object_bytes = 1048576
catalog_projection_manifest_object = "catalog-projection-manifest.json"

[artifact_store.s3]
region = "us-east-1"
conditional_put = "etag"
copy_if_not_exists = "multipart"
terminal_commit_timeout_seconds = 60

[artifact_store.create_only_probe]
prefix = ".writer-probe"
object_name = "sentinel"
copy_source_object_name = "copy-source"
copy_dest_object_name = "copy-dest"

[artifact_store.subpaths]
raw = "raw"
nt_catalog = "nt-catalog"
nt_catalog_synthetic_proof = "nt-catalog-synthetic-proof"
source_proofs = "source-proofs"
backtests = "backtests"
artifact_index = "artifact-index"
research_analytics = "research-analytics"

[artifact_store.lifecycle]
retention = "forever"
default_delete_expiration = "disabled"
storage_profiles = ["active", "archive", "deep_archive"]

[artifact_store.lifecycle.quiet_window_seconds]
raw = 7200
nt_catalog = 7200
source_proofs = 7200
backtests = 3600
artifact_index = 0
research_analytics = 7200

[artifact_store.lifecycle.hot_index]
latest_pointer_storage_profile = "active"
current_snapshot_storage_profile = "active"

[[catalog_dispatch.bindings]]
source_binding = "binance-spot-native-trades"
market_structure_fixture = "perps-spot"
catalog_projection_id = "stale-profile-catalog-projection"
"#
    .replace(
        "__NT_REVISION__",
        &backtesting_vertical_slice::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
            .expect("BVS NautilusTrader dependency provenance"),
    )
}
