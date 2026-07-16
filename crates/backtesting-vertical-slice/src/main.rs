//! Binary: run the NautilusTrader backtesting vertical slice over an accepted
//! dataset object.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven run-spec TOML; the only
//! command-line inputs are filesystem paths. The object container is declared in
//! the run-spec converter config. The orchestration itself lives in
//! [`backtesting_vertical_slice::operator`] so it is unit-testable; this binary
//! is a thin CLI shim that reads the inputs, runs the operator path, and prints
//! the produced artifacts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;

use bolt_v2::bolt_v3_config::BacktestConfigOverrideReport;

use backtesting_vertical_slice::{
    backfill_execution_plan::{BackfillExecutionPlan, validate_execution_plan_for_run_spec},
    operator::{
        MultiTableRunArtifacts, OperatorRunArtifacts, RunArtifacts, RunSpec,
        VerifiedSourceBindingRegistry, run_operator_from_run_spec_guarded,
        validate_run_spec_manifest_for_object_hash_with_verified_registry,
    },
    operator_work_budget::{
        OperatorWorkBudget, OperatorWorkBudgetGuard, OperatorWorkBudgetStage,
        read_exact_sized_file_guarded,
    },
    result_contract::{BacktestFeedLabel, BacktestRunGuardReport},
};

#[derive(Debug, Parser)]
#[command(about = "Run the NautilusTrader backtesting vertical slice over an accepted dataset.")]
struct Cli {
    /// Path to the run-spec TOML (dataset facts: object, source proof, manifest).
    #[arg(long)]
    run_spec: PathBuf,
    /// Pre-payload execution plan that must match the run-spec.
    #[arg(long)]
    execution_plan: PathBuf,
    /// Local path to the accepted object whose SHA-256 the run-spec pins.
    #[arg(long = "object")]
    object_path: PathBuf,
    /// Output directory for produced artifacts.
    #[arg(long)]
    output_dir: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut object_reader =
        |path: &Path, expected_bytes: u64, work_budget: &OperatorWorkBudgetGuard| {
            read_object_checked(path, expected_bytes, work_budget)
        };
    let (spec, run_spec_hash) = read_run_spec_with_hash(&cli.run_spec)?;
    run_cli_with_spec_object_reader(&cli, spec, &run_spec_hash, &mut object_reader)
}

#[cfg(test)]
fn run_cli_with_object_reader<F>(cli: &Cli, object_reader: &mut F) -> Result<()>
where
    F: FnMut(&Path, u64, &OperatorWorkBudgetGuard) -> Result<Vec<u8>>,
{
    let (spec, run_spec_hash) = read_run_spec_with_hash(&cli.run_spec)?;
    run_cli_with_spec_object_reader(cli, spec, &run_spec_hash, object_reader)
}

fn run_cli_with_spec_object_reader<F>(
    cli: &Cli,
    spec: RunSpec,
    run_spec_hash: &str,
    object_reader: &mut F,
) -> Result<()>
where
    F: FnMut(&Path, u64, &OperatorWorkBudgetGuard) -> Result<Vec<u8>>,
{
    ensure!(
        spec.artifact_store.is_none(),
        "run-spec [artifact_store] must use source_universe_batch_execution; the direct binary is local-only"
    );
    let execution_plan = read_execution_plan(&cli.execution_plan)?;
    validate_execution_plan_for_run_spec(&execution_plan, run_spec_hash, &spec)
        .with_context(|| format!("execution plan {}", cli.execution_plan.display()))?;
    let work_budget =
        OperatorWorkBudgetGuard::new(OperatorWorkBudget::from_execution_plan(&execution_plan))?;
    let source_binding_registry =
        VerifiedSourceBindingRegistry::from_run_spec_guarded(&spec, &work_budget)?;
    validate_run_spec_manifest_for_object_hash_with_verified_registry(
        &spec,
        &cli.output_dir,
        &spec.accepted_object.sha256,
        &source_binding_registry,
    )
    .with_context(|| format!("run-manifest {}", cli.run_spec.display()))?;
    backtesting_vertical_slice::research_analytics::ensure_object_read_within_raw_payload_limit(
        &spec,
    )?;
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;
    let object_bytes = object_reader(&cli.object_path, spec.accepted_object.bytes, &work_budget)?;
    work_budget.check_deadline(OperatorWorkBudgetStage::ObjectVerification)?;

    match run_operator_from_run_spec_guarded(
        &spec,
        &object_bytes,
        &cli.output_dir,
        &source_binding_registry,
        &work_budget,
    )? {
        OperatorRunArtifacts::Trade(artifacts) => print_trade_run(&artifacts),
        OperatorRunArtifacts::MultiTable(artifacts) => print_multi_table_run(&artifacts),
    }
    Ok(())
}

fn print_trade_run(artifacts: &RunArtifacts) {
    let output = &artifacts.output;
    println!("accepted_object_sha256 = {}", artifacts.verified_sha256);
    println!(
        "source_proof_id = {}",
        artifacts.accepted_source_proof.source_proof_id
    );
    println!(
        "canonical_trades_rows = {}",
        output.canonical_table.rows.len()
    );
    println!(
        "canonical_artifact = {}",
        artifacts.canonical_artifact_path.display()
    );
    if let Some(canonical_catalog_uri) = &artifacts.canonical_catalog_uri {
        println!("nt_catalog_uri = {canonical_catalog_uri}");
    }
    println!("catalog_hash = {}", output.projection.catalog_hash);
    println!("catalog_read_back_trade_ticks = {}", output.read_back_count);
    println!("nt_version = {}", output.contract.nt_version);
    println!(
        "strategy_config_hash = {}",
        output.contract.strategy_config_hash
    );
    println!(
        "backtest_run_config_id = {:?}",
        output.nt_result.run_config_id
    );
    println!(
        "nt_iterations = {} (market-data points processed by the engine)",
        output.nt_result.iterations
    );
    println!(
        "nt_backtest_start = {:?}, nt_backtest_end = {:?}",
        output.nt_result.backtest_start, output.nt_result.backtest_end
    );
    println!(
        "nt_total_events = {}, nt_total_orders = {}, nt_total_positions = {}",
        output.nt_result.total_events,
        output.nt_result.total_orders,
        output.nt_result.total_positions
    );
    println!("fidelity_class = {:?}", output.contract.fidelity_class);
    if let Some(report) = &output.contract.config_override_report {
        print_config_override_report(report);
    }
    if let Some(report) = &output.contract.run_guard_report {
        print_run_guard_report(report);
    }
    print_feed_labels(&output.contract.feed_labels);
    println!("result_contract = {}", artifacts.contract_path.display());
    println!("accepted_source_proof = {}", artifacts.proof_path.display());
}

fn print_multi_table_run(artifacts: &MultiTableRunArtifacts) {
    println!("accepted_object_sha256 = {}", artifacts.verified_sha256);
    println!(
        "source_proof_id = {}",
        artifacts.accepted_source_proof.source_proof_id
    );
    println!("projected_tables = {}", artifacts.tables.len());
    for table in &artifacts.tables {
        println!(
            "projected_table = family={} instrument={} data_type={} bar_spec={:?} rows={} \
             subroot={} catalog_hash={}",
            table.table_family,
            table.nt_instrument_id,
            table.data_type,
            table.bar_spec,
            table.rows,
            table.subroot.display(),
            table.catalog_hash
        );
    }
    if let Some(path) = &artifacts.conversion_tables_path {
        println!("conversion_tables_index = {}", path.display());
    }
    println!("nt_version = {}", artifacts.contract.nt_version);
    println!(
        "strategy_config_hash = {}",
        artifacts.contract.strategy_config_hash
    );
    println!(
        "backtest_run_config_id = {:?}",
        artifacts.nt_result.run_config_id
    );
    println!(
        "nt_iterations = {} (market-data points processed by the engine)",
        artifacts.nt_result.iterations
    );
    println!(
        "nt_backtest_start = {:?}, nt_backtest_end = {:?}",
        artifacts.nt_result.backtest_start, artifacts.nt_result.backtest_end
    );
    println!(
        "nt_total_events = {}, nt_total_orders = {}, nt_total_positions = {}",
        artifacts.nt_result.total_events,
        artifacts.nt_result.total_orders,
        artifacts.nt_result.total_positions
    );
    println!("fidelity_class = {:?}", artifacts.contract.fidelity_class);
    if let Some(report) = &artifacts.contract.config_override_report {
        print_config_override_report(report);
    }
    if let Some(report) = &artifacts.contract.run_guard_report {
        print_run_guard_report(report);
    }
    print_feed_labels(&artifacts.contract.feed_labels);
    println!("result_contract = {}", artifacts.contract_path.display());
    println!("accepted_source_proof = {}", artifacts.proof_path.display());
}

fn print_config_override_report(report: &BacktestConfigOverrideReport) {
    println!("config_override_label = {}", report.label);
    println!(
        "config_override_production_root = {}",
        report.production_root_path
    );
    println!(
        "config_override_production_bundle_checksum = {}",
        report.production_config_bundle_checksum
    );
    println!(
        "config_override_signal = role={} before={}:{} after={}:{}",
        report.signal_role,
        report.signal_before.data_client_id,
        report.signal_before.instrument_id,
        report.signal_after.data_client_id,
        report.signal_after.instrument_id
    );
    for source in &report.realized_volatility_sources_before {
        println!(
            "config_override_rv_before = surface={} source={} client={} instrument={}",
            report.realized_volatility_surface_id,
            source.source_id,
            source.data_client_id,
            source.instrument_id
        );
    }
    for source in &report.realized_volatility_sources_after {
        println!(
            "config_override_rv_after = surface={} source={} client={} instrument={}",
            report.realized_volatility_surface_id,
            source.source_id,
            source.data_client_id,
            source.instrument_id
        );
    }
    for source in &report.realized_volatility_sources_removed {
        println!(
            "config_override_rv_removed = surface={} source={} client={} instrument={}",
            report.realized_volatility_surface_id,
            source.source_id,
            source.data_client_id,
            source.instrument_id
        );
    }
}

fn print_run_guard_report(report: &BacktestRunGuardReport) {
    println!(
        "run_guard = armed={} traded={} signal_quote_received={} rv_ready={} \
         price_to_beat_received={} reference_fresh={}",
        report.armed,
        report.traded,
        report.signal_quote_received,
        report.realized_volatility_ready,
        report.price_to_beat_received,
        report.reference_fresh
    );
    println!(
        "run_guard_counts = snapshots={} order_intents={} admission_decisions={} \
         admitted_orders={} submit_reservations={} submit_fills={} entry_skips={} \
         exit_decisions={} loss_governor_halts={} requote_throttles={}",
        report.strategy_input_snapshot_count,
        report.order_intent_count,
        report.admission_decision_count,
        report.admitted_order_count,
        report.submit_reservation_count,
        report.submit_fill_count,
        report.entry_skip_count,
        report.exit_decision_count,
        report.loss_governor_halt_count,
        report.requote_throttle_count
    );
    if let Some(reason) = &report.did_not_arm_reason {
        println!("run_guard_did_not_arm_reason = {reason}");
    }
}

fn print_feed_labels(labels: &[BacktestFeedLabel]) {
    for label in labels {
        println!(
            "feed_label = id={} source_class={} data_type={} instrument={} label={}",
            label.feed_id, label.source_class, label.data_type, label.instrument_id, label.label
        );
    }
}

fn read_run_spec_with_hash(path: &Path) -> Result<(RunSpec, String)> {
    backtesting_vertical_slice::research_analytics::read_run_spec_with_hash(path)
}

fn read_execution_plan(path: &Path) -> Result<BackfillExecutionPlan> {
    let bytes =
        fs::read(path).with_context(|| format!("read execution plan {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse execution-plan JSON")
}

fn read_object_checked(
    path: &Path,
    expected_bytes: u64,
    work_budget: &OperatorWorkBudgetGuard,
) -> Result<Vec<u8>> {
    read_exact_sized_file_guarded(
        path,
        expected_bytes,
        work_budget,
        OperatorWorkBudgetStage::ObjectVerification,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use backtesting_vertical_slice::backfill_execution_plan::BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION;
    use backtesting_vertical_slice::hashing::sha256_hex;
    use clap::error::ErrorKind;

    const COMMITTED_RUN_SPEC: &str = include_str!(
        "../../../specs/023-nt-research-analytics-platform/reference/backtesting-vertical-slice-run-spec.bnbusdc-2026-03-01.toml"
    );
    const TEST_MAX_SOURCE_ROWS: u64 = 100_000;
    const TEST_MAX_PROJECTED_ROW_GROUPS: u64 = 1;
    const TEST_MAX_WALL_SECONDS: u64 = 300;

    fn write_matching_execution_plan(dir: &Path, spec: &RunSpec, run_spec_hash: &str) -> PathBuf {
        let path = dir.join("execution-plan.json");
        let accepted_tranche_id = format!("{}-accepted-tranche", spec.manifest.run_id);
        let plan = serde_json::json!({
            "schema_version": BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
            "plan_id": format!("{}-execution-plan", spec.manifest.run_id),
            "status": "ready",
            "accepted_tranche_id": accepted_tranche_id,
            "accepted_tranche_manifest_hash": sha256_hex(accepted_tranche_id.as_bytes()),
            "run_spec_hash": run_spec_hash,
            "operator_run_id": spec.manifest.run_id,
            "output_prefix": spec.manifest.output_prefix,
            "source_proof_id": spec.manifest.source_proof_id,
            "source_proof_version": spec.manifest.source_proof_version,
            "source_binding": spec.manifest.venue_binding_key,
            "table_family": &spec.source_proof.table_family,
            "object_count": 1,
            "accepted_bytes": spec.accepted_object.bytes,
            "max_object_bytes": spec.converter.raw_payload.max_object_bytes,
            "max_decoded_bytes": spec.converter.raw_payload.max_decoded_bytes,
            "max_source_rows": TEST_MAX_SOURCE_ROWS,
            "max_projected_row_groups": TEST_MAX_PROJECTED_ROW_GROUPS,
            "max_wall_seconds": TEST_MAX_WALL_SECONDS,
            "objects": [{
                "s3_uri": spec.accepted_object.s3_uri,
                "source_url": spec.accepted_object.source_url,
                "sha256": spec.accepted_object.sha256,
                "bytes": spec.accepted_object.bytes,
                "archive_date": spec.accepted_object.archive_date,
            }],
            "blocking_issues": []
        });
        backtesting_vertical_slice::reference_artifact::write_reference_artifact(
            &path,
            BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
            &plan,
            backtesting_vertical_slice::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
        )
        .unwrap();
        path
    }

    fn run_spec_text_with_catalog_data_type(data_type: String) -> String {
        let mut value: toml::Value =
            toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec TOML parses as value");
        let root = value.as_table_mut().expect("run-spec root is a table");
        root.remove("artifact_store");
        root.remove("catalog_dispatch");
        let manifest = root
            .get_mut("manifest")
            .and_then(toml::Value::as_table_mut)
            .expect("run-spec has manifest table");
        let catalog_input = manifest
            .get_mut("catalog_inputs")
            .and_then(toml::Value::as_array_mut)
            .and_then(|inputs| inputs.first_mut())
            .and_then(toml::Value::as_table_mut)
            .expect("run-spec has at least one manifest catalog_inputs table");
        catalog_input.insert("data_type".to_string(), toml::Value::String(data_type));
        toml::to_string_pretty(&value).expect("mutated run-spec serializes")
    }

    fn local_committed_run_spec_text() -> String {
        let mut value: toml::Value =
            toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec TOML parses as value");
        let root = value.as_table_mut().expect("run-spec root is a table");
        root.remove("artifact_store");
        root.remove("catalog_dispatch");
        toml::to_string_pretty(&value).expect("local run-spec serializes")
    }

    fn run_spec_text_with_source_binding(
        source_bindings_path: &Path,
        source_binding: &str,
        venue: &str,
        product_family: &str,
        source_url: &str,
    ) -> String {
        let mut value: toml::Value =
            toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec TOML parses as value");
        let root = value.as_table_mut().expect("run-spec root is a table");
        root.remove("artifact_store");
        root.remove("catalog_dispatch");
        root.insert(
            "source_bindings_path".to_string(),
            toml::Value::String(source_bindings_path.display().to_string()),
        );
        let accepted_object = root
            .get_mut("accepted_object")
            .and_then(toml::Value::as_table_mut)
            .expect("run-spec has accepted_object table");
        accepted_object.insert(
            "source_url".to_string(),
            toml::Value::String(source_url.to_string()),
        );
        let source_proof = root
            .get_mut("source_proof")
            .and_then(toml::Value::as_table_mut)
            .expect("run-spec has source_proof table");
        for (field, value) in [
            ("source_binding", source_binding),
            ("venue", venue),
            ("product_family", product_family),
            ("product_category", product_family),
        ] {
            source_proof.insert(field.to_string(), toml::Value::String(value.to_string()));
        }
        let manifest = root
            .get_mut("manifest")
            .and_then(toml::Value::as_table_mut)
            .expect("run-spec has manifest table");
        manifest.insert(
            "venue_binding_key".to_string(),
            toml::Value::String(source_binding.to_string()),
        );
        toml::to_string_pretty(&value).expect("mutated run-spec serializes")
    }

    #[test]
    fn cli_has_no_runtime_publication_switch() {
        let base_args = [
            "backtesting-vertical-slice",
            "--run-spec",
            "run.toml",
            "--execution-plan",
            "execution-plan.json",
            "--object",
            "object.csv.gz",
            "--output-dir",
            "out",
        ];

        Cli::try_parse_from(base_args).expect("default cli parses");

        let publish_error = Cli::try_parse_from(
            base_args
                .into_iter()
                .chain(["--publish-output"])
                .collect::<Vec<_>>(),
        )
        .expect_err("retired publication switch must not parse");
        assert!(
            publish_error.to_string().contains("unexpected argument"),
            "{publish_error}"
        );

        let durable_error = Cli::try_parse_from(
            base_args
                .into_iter()
                .chain(["--durable-completion-locator", "completion.json"])
                .collect::<Vec<_>>(),
        )
        .expect_err("direct binary must not expose a durable resume path");
        assert!(
            durable_error.to_string().contains("unexpected argument"),
            "{durable_error}"
        );
    }

    #[test]
    fn direct_binary_has_no_durable_dispatch_path() {
        let source = include_str!("main.rs");
        for forbidden in [
            concat!("DurableRun", "Dispatcher"),
            concat!("DurableRun", "Request"),
            concat!("DurableCompletion", "Locator"),
            "runtime_evidence_guarded(",
        ] {
            assert!(!source.contains(forbidden), "forbidden direct path {forbidden}");
        }
    }

    #[test]
    fn read_object_rejects_size_mismatch_before_loading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let object_path = dir.path().join("object.csv.gz");
        fs::write(&object_path, b"not-the-accepted-object").unwrap();

        let err = read_object_checked(&object_path, 99, &OperatorWorkBudgetGuard::unbounded())
            .unwrap_err();

        assert!(err.to_string().contains("object byte length 23"), "{err}");
        assert!(err.to_string().contains("pinned expected size 99"), "{err}");
    }

    #[test]
    fn cli_rejects_plan_object_above_payload_budget_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        let run_spec_text = local_committed_run_spec_text();
        fs::write(&run_spec_path, &run_spec_text).unwrap();
        let mut spec: RunSpec = toml::from_str(&run_spec_text).expect("run-spec parses");
        spec.converter.raw_payload.max_object_bytes = spec.accepted_object.bytes - 1;
        let run_spec_hash = sha256_hex(run_spec_text.as_bytes());
        let execution_plan = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan,
            object_path: dir.path().join("oversized-object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!("object reader must not run after configured payload max rejection")
            };
        let err = run_cli_with_spec_object_reader(&cli, spec, &run_spec_hash, &mut object_reader)
            .expect_err("execution plan payload budget must reject before object read");

        let error_chain = err
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(error_chain.contains("execution plan"), "{error_chain}");
        assert!(error_chain.contains("byte budget"), "{error_chain}");
        assert!(
            !object_reader_called,
            "execution plan byte budget must reject before local object read"
        );
    }

    #[test]
    fn cli_rejects_unsupported_catalog_data_type_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        let base_spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let unsupported_data_type = format!(
            "{}-unsupported",
            base_spec.manifest.catalog_inputs[0].data_type
        );
        let run_spec_text = run_spec_text_with_catalog_data_type(unsupported_data_type);
        fs::write(&run_spec_path, &run_spec_text).unwrap();
        let spec: RunSpec = toml::from_str(&run_spec_text).expect("mutated run-spec parses");
        let run_spec_hash = sha256_hex(run_spec_text.as_bytes());
        let execution_plan = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!("object reader must not run after manifest rejection")
            };
        let err = run_cli_with_object_reader(&cli, &mut object_reader)
            .expect_err("unsupported catalog data type must reject before object read");

        let error_chain = err
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            error_chain.contains("unsupported catalog data type"),
            "{error_chain}"
        );
        assert!(
            !object_reader_called,
            "run-manifest validation must reject before local object read"
        );
    }

    #[test]
    fn cli_uses_run_spec_source_bindings_path_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let source_bindings_path = dir.path().join("source-bindings.toml");
        fs::write(
            &source_bindings_path,
            r#"schema_version = "backfill-source-bindings.v1"
contract_version = "backfill-table-contract.v1"

[[source_binding]]
key = "runtime-synthetic-native-trades"
venue = "runtime-synthetic"
product_family = "spot"
market_structure_fixture = "perps-spot"
source_uri = "https://runtime-source.example/trades/{symbol}/{dt}.csv.gz"
evidence_state = "owner_archive_backfillable"
table_families = ["trades"]
"#,
        )
        .unwrap();
        let source_url = "https://runtime-source.example/trades/BNBUSDC/2026-03-01.csv.gz";
        let run_spec_text = run_spec_text_with_source_binding(
            &source_bindings_path,
            "runtime-synthetic-native-trades",
            "runtime-synthetic",
            "spot",
            source_url,
        );
        let run_spec_path = dir.path().join("run.toml");
        fs::write(&run_spec_path, &run_spec_text).unwrap();
        let spec: RunSpec = toml::from_str(&run_spec_text).expect("mutated run-spec parses");
        let run_spec_hash = sha256_hex(run_spec_text.as_bytes());
        let execution_plan = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!("object reader reached after runtime registry preflight")
            };
        let err = run_cli_with_object_reader(&cli, &mut object_reader)
            .expect_err("object reader sentinel should stop after registry preflight");

        let error_chain = err
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(object_reader_called, "{error_chain}");
        assert!(
            error_chain.contains("object reader reached after runtime registry preflight"),
            "{error_chain}"
        );
    }

    #[test]
    fn synchronous_local_core_rejects_durable_run_spec_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        fs::write(&run_spec_path, COMMITTED_RUN_SPEC).unwrap();
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let run_spec_hash = sha256_hex(COMMITTED_RUN_SPEC.as_bytes());
        let execution_plan = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan,
            object_path: dir.path().join("missing-object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!("object reader should not run before publish preflight")
            };
        let err = run_cli_with_object_reader(&cli, &mut object_reader)
            .expect_err("durable RunSpec must reject from the local-only test core");

        assert!(err.to_string().contains("source_universe_batch_execution"), "{err}");
        assert!(
            !object_reader_called,
            "publish preflight must run before local object read"
        );
    }

    #[test]
    fn cli_requires_execution_plan_before_reading_object() {
        let args = [
            "backtesting-vertical-slice",
            "--run-spec",
            "run.toml",
            "--object",
            "object.csv.gz",
            "--output-dir",
            "out",
        ];

        let err = Cli::try_parse_from(args).expect_err("execution plan is required");

        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert!(err.to_string().contains("--execution-plan"), "{err}");
    }

    #[test]
    fn cli_execution_plan_mismatch_rejects_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        let run_spec_text = local_committed_run_spec_text();
        fs::write(&run_spec_path, &run_spec_text).unwrap();
        let spec: RunSpec = toml::from_str(&run_spec_text).expect("run-spec parses");
        let execution_plan_path = dir.path().join("execution-plan.json");
        let execution_plan = serde_json::json!({
            "schema_version": BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
            "plan_id": "synthetic-plan",
            "status": "ready",
            "accepted_tranche_id": "synthetic-tranche",
            "accepted_tranche_manifest_hash": "synthetic-tranche-hash",
            "run_spec_hash": "different-run-spec-hash",
            "operator_run_id": spec.manifest.run_id,
            "output_prefix": spec.manifest.output_prefix,
            "source_proof_id": spec.manifest.source_proof_id,
            "source_proof_version": spec.manifest.source_proof_version,
            "source_binding": spec.manifest.venue_binding_key,
            "table_family": &spec.source_proof.table_family,
            "object_count": 1,
            "accepted_bytes": spec.accepted_object.bytes,
            "max_object_bytes": spec.converter.raw_payload.max_object_bytes,
            "max_decoded_bytes": spec.converter.raw_payload.max_decoded_bytes,
            "max_source_rows": TEST_MAX_SOURCE_ROWS,
            "max_projected_row_groups": TEST_MAX_PROJECTED_ROW_GROUPS,
            "max_wall_seconds": TEST_MAX_WALL_SECONDS,
            "objects": [{
                "s3_uri": spec.accepted_object.s3_uri,
                "source_url": spec.accepted_object.source_url,
                "sha256": spec.accepted_object.sha256,
                "bytes": spec.accepted_object.bytes,
                "archive_date": spec.accepted_object.archive_date,
            }],
            "blocking_issues": []
        });
        backtesting_vertical_slice::reference_artifact::write_reference_artifact_with_len(
            &execution_plan_path,
            BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
            &execution_plan,
            backtesting_vertical_slice::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
        )
        .unwrap();
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan: execution_plan_path,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!("object reader must not run after execution-plan mismatch")
            };
        let err = run_cli_with_object_reader(&cli, &mut object_reader)
            .expect_err("execution-plan mismatch must reject before object read");

        assert!(err.to_string().contains("execution plan"), "{err}");
        assert!(!object_reader_called);
    }

    #[test]
    fn cli_execution_plan_table_family_mismatch_rejects_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        let run_spec_text = local_committed_run_spec_text();
        fs::write(&run_spec_path, &run_spec_text).unwrap();
        let spec: RunSpec = toml::from_str(&run_spec_text).expect("run-spec parses");
        let run_spec_hash = sha256_hex(run_spec_text.as_bytes());
        let execution_plan_path = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let mut execution_plan: serde_json::Value =
            serde_json::from_slice(&fs::read(&execution_plan_path).unwrap()).unwrap();
        execution_plan["table_family"] =
            serde_json::Value::String(format!("{}-mismatch", spec.source_proof.table_family));
        backtesting_vertical_slice::reference_artifact::write_reference_artifact_with_len(
            &execution_plan_path,
            BACKFILL_EXECUTION_PLAN_SCHEMA_VERSION,
            &execution_plan,
            backtesting_vertical_slice::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
        )
        .unwrap();
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan: execution_plan_path,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
        };
        let mut object_reader_called = false;
        let mut object_reader =
            |_path: &Path, _expected_bytes: u64, _work_budget: &OperatorWorkBudgetGuard| {
                object_reader_called = true;
                anyhow::bail!(
                    "object reader must not run after execution-plan table-family mismatch"
                )
            };
        let err = run_cli_with_object_reader(&cli, &mut object_reader)
            .expect_err("execution-plan table-family mismatch must reject before object read");

        let error_chain = err
            .chain()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(error_chain.contains("table_family"), "{error_chain}");
        assert!(!object_reader_called);
    }
}
