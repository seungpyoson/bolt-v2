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

use backtesting_vertical_slice::hashing::sha256_hex;
use backtesting_vertical_slice::{
    artifact_store_secrets::{ArtifactStoreSecretResolver, ArtifactStoreSsmResolver},
    backfill_execution_plan::{
        BackfillExecutionPlan, BackfillExecutionPlanStatus, BackfillExecutionRunBinding,
    },
    nt_catalog_capability::NtCatalogSsmCredentialResolver,
    operator::{
        MultiTableRunArtifacts, OperatorRunArtifacts, PublishOptions, PublishedArtifact,
        PublishedCatalogProof, RunArtifacts, RunSpec,
        run_from_run_spec_and_publish_with_resolved_storage_options,
        run_from_run_spec_with_artifact_store, run_operator_from_run_spec,
        validate_run_spec_manifest_for_object_hash,
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
    /// Publish produced artifacts to manifest.output_prefix after the local run succeeds.
    #[arg(long)]
    publish_output: bool,
    /// After publishing, run BacktestNode against the published catalog and publish the proof.
    #[arg(long, requires = "publish_output")]
    prove_published_catalog: bool,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut object_reader =
        |path: &Path, expected_bytes: u64| read_object_checked(path, expected_bytes);
    let (spec, run_spec_hash) = read_run_spec_with_hash(&cli.run_spec)?;
    if cli.publish_output && cli.prove_published_catalog {
        return run_cli_durable_catalog_with_spec_object_reader(
            &cli,
            spec,
            &run_spec_hash,
            &mut object_reader,
        )
        .await;
    }
    if cli.publish_output && spec.manifest.artifact_store.ssm_parameters.is_some() {
        let mut resolver = ArtifactStoreSsmResolver::new()?;
        run_cli_with_spec_object_reader_and_resolver(
            &cli,
            spec,
            &run_spec_hash,
            &mut object_reader,
            &mut resolver,
        )
    } else {
        let mut resolver = |_region: &str, _path: &str| {
            Err::<String, String>("artifact-store SSM resolver was not configured".to_string())
        };
        run_cli_with_spec_object_reader_and_resolver(
            &cli,
            spec,
            &run_spec_hash,
            &mut object_reader,
            &mut resolver,
        )
    }
}

async fn run_cli_durable_catalog_with_spec_object_reader<F>(
    cli: &Cli,
    spec: RunSpec,
    run_spec_hash: &str,
    object_reader: &mut F,
) -> Result<()>
where
    F: FnMut(&Path, u64) -> Result<Vec<u8>>,
{
    let execution_plan = read_execution_plan(&cli.execution_plan)?;
    validate_execution_plan_for_run_spec(&execution_plan, run_spec_hash, &spec)
        .with_context(|| format!("execution plan {}", cli.execution_plan.display()))?;
    validate_run_spec_manifest_for_object_hash(
        &spec,
        &cli.output_dir,
        &spec.accepted_object.sha256,
    )
    .with_context(|| format!("run-manifest {}", cli.run_spec.display()))?;
    ensure_object_read_within_raw_payload_limit(&spec)?;
    let object_bytes = object_reader(&cli.object_path, spec.accepted_object.bytes)?;
    let artifact_store = spec.required_artifact_store()?;
    let nt_catalog_capability_proof = spec.required_nt_catalog_capability_proof()?;
    let artifact_root = artifact_store.resolve()?;
    let credential_resolver =
        NtCatalogSsmCredentialResolver::from_region(artifact_root.s3_region()).await?;
    let credentials = credential_resolver
        .resolve(&nt_catalog_capability_proof.ssm_parameter_refs)
        .await?;
    let store = artifact_root.build_s3_object_store_with_credentials(&credentials)?;
    let artifacts = run_from_run_spec_with_artifact_store(
        &spec,
        &object_bytes,
        &cli.output_dir,
        &store,
        |_, _, create_only_probe| {
            nt_catalog_capability_proof.runtime_evidence(
                artifact_store,
                &credentials,
                create_only_probe,
            )
        },
    )
    .await?;
    print_trade_run(&artifacts, None, None);
    Ok(())
}

#[cfg(test)]
fn run_cli_with_object_reader_and_resolver<F, R>(
    cli: &Cli,
    object_reader: &mut F,
    resolver: &mut R,
) -> Result<()>
where
    F: FnMut(&Path, u64) -> Result<Vec<u8>>,
    R: ArtifactStoreSecretResolver,
{
    let (spec, run_spec_hash) = read_run_spec_with_hash(&cli.run_spec)?;
    run_cli_with_spec_object_reader_and_resolver(cli, spec, &run_spec_hash, object_reader, resolver)
}

fn run_cli_with_spec_object_reader_and_resolver<F, R>(
    cli: &Cli,
    spec: RunSpec,
    run_spec_hash: &str,
    object_reader: &mut F,
    resolver: &mut R,
) -> Result<()>
where
    F: FnMut(&Path, u64) -> Result<Vec<u8>>,
    R: ArtifactStoreSecretResolver,
{
    let execution_plan = read_execution_plan(&cli.execution_plan)?;
    validate_execution_plan_for_run_spec(&execution_plan, run_spec_hash, &spec)
        .with_context(|| format!("execution plan {}", cli.execution_plan.display()))?;
    let publish_options = PublishOptions {
        prove_published_catalog: cli.prove_published_catalog,
    };
    let resolved_publish_storage_options = if cli.publish_output {
        let mut resolve_secret = |region: &str, path: &str| resolver.resolve_secret(region, path);
        spec.manifest
            .artifact_store_storage_options_resolved(&mut resolve_secret)
            .map_err(|error| anyhow::anyhow!("artifact-store options rejected: {error}"))?
    } else {
        None
    };
    validate_run_spec_manifest_for_object_hash(
        &spec,
        &cli.output_dir,
        &spec.accepted_object.sha256,
    )
    .with_context(|| format!("run-manifest {}", cli.run_spec.display()))?;
    ensure_object_read_within_raw_payload_limit(&spec)?;
    let object_bytes = object_reader(&cli.object_path, spec.accepted_object.bytes)?;

    if cli.publish_output {
        let published = run_from_run_spec_and_publish_with_resolved_storage_options(
            &spec,
            &object_bytes,
            &cli.output_dir,
            publish_options,
            resolved_publish_storage_options.as_ref(),
        )?;
        print_trade_run(
            &published.run,
            Some(published.published_artifacts),
            published.published_catalog_proof,
        );
    } else {
        match run_operator_from_run_spec(&spec, &object_bytes, &cli.output_dir)? {
            OperatorRunArtifacts::Trade(artifacts) => print_trade_run(&artifacts, None, None),
            OperatorRunArtifacts::MultiTable(artifacts) => print_multi_table_run(&artifacts),
        }
    }
    Ok(())
}

fn print_trade_run(
    artifacts: &RunArtifacts,
    published_artifacts: Option<Vec<PublishedArtifact>>,
    published_catalog_proof: Option<PublishedCatalogProof>,
) {
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
    if let Some(proof) = published_catalog_proof {
        println!("published_catalog_proof = {}", proof.catalog_uri);
        println!(
            "published_catalog_direct_s3 = {}",
            proof.direct_s3_catalog_access_proven
        );
        println!(
            "published_catalog_iterations = {}/{}",
            proof.nt_iterations, proof.expected_iterations
        );
    }
    if let Some(published_artifacts) = published_artifacts {
        println!("published_artifacts = {}", published_artifacts.len());
        for artifact in published_artifacts {
            println!(
                "published_artifact = {} bytes={} sha256={}",
                artifact.published_uri, artifact.bytes, artifact.sha256
            );
        }
    }
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
         admitted_orders={} submit_reservations={} submit_fills={}",
        report.strategy_input_snapshot_count,
        report.order_intent_count,
        report.admission_decision_count,
        report.admitted_order_count,
        report.submit_reservation_count,
        report.submit_fill_count
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
    let bytes = fs::read(path).with_context(|| format!("read run-spec {}", path.display()))?;
    let hash = sha256_hex(&bytes);
    let text = std::str::from_utf8(&bytes).context("run-spec TOML is not UTF-8")?;
    let mut spec: RunSpec = toml::from_str(text).context("parse run-spec TOML")?;
    if spec.source_bindings_path.is_relative() {
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let sibling_relative = base_dir.join(&spec.source_bindings_path);
        if sibling_relative.exists() {
            spec.source_bindings_path = sibling_relative;
        }
    }
    Ok((spec, hash))
}

fn read_execution_plan(path: &Path) -> Result<BackfillExecutionPlan> {
    let bytes =
        fs::read(path).with_context(|| format!("read execution plan {}", path.display()))?;
    serde_json::from_slice(&bytes).context("parse execution-plan JSON")
}

fn validate_execution_plan_for_run_spec(
    plan: &BackfillExecutionPlan,
    run_spec_hash: &str,
    spec: &RunSpec,
) -> Result<()> {
    ensure!(
        plan.status == BackfillExecutionPlanStatus::Ready,
        "execution plan status must be ready"
    );
    ensure!(
        plan.blocking_issues.is_empty(),
        "execution plan has blocking issues"
    );
    ensure!(
        plan.run_spec_hash == run_spec_hash,
        "execution plan run_spec_hash {} does not match submitted run-spec {run_spec_hash}",
        plan.run_spec_hash
    );
    let binding = BackfillExecutionRunBinding::from_run_spec(spec);
    ensure!(
        plan.operator_run_id == binding.run_id,
        "execution plan operator_run_id mismatch"
    );
    ensure!(
        plan.output_prefix == binding.output_prefix,
        "execution plan output_prefix mismatch"
    );
    ensure!(
        plan.source_proof_id == binding.source_proof_id
            && plan.source_proof_version == binding.source_proof_version,
        "execution plan source proof mismatch"
    );
    ensure!(
        plan.source_binding == binding.source_binding,
        "execution plan source binding mismatch"
    );
    ensure!(
        plan.table_family == binding.table_family,
        "execution plan table_family {} does not match submitted run-spec {}",
        plan.table_family,
        binding.table_family
    );
    ensure!(
        plan.object_count == 1 && plan.objects.len() == 1,
        "execution plan must bind exactly one accepted object"
    );
    let object = &plan.objects[0];
    ensure!(
        plan.accepted_bytes == object.bytes,
        "execution plan accepted_bytes mismatch"
    );
    ensure!(
        object.s3_uri == binding.raw_sample_uri && object.s3_uri == binding.accepted_object_s3_uri,
        "execution plan object URI mismatch"
    );
    ensure!(
        object.sha256 == binding.raw_sample_hash && object.sha256 == binding.accepted_object_sha256,
        "execution plan object hash mismatch"
    );
    ensure!(
        object.source_url == binding.accepted_object_source_url,
        "execution plan object source URL mismatch"
    );
    ensure!(
        object.bytes == binding.accepted_object_bytes,
        "execution plan object byte count mismatch"
    );
    ensure!(
        object.archive_date == binding.accepted_object_archive_date,
        "execution plan object archive date mismatch"
    );
    ensure!(
        plan.max_object_bytes == binding.max_object_bytes && object.bytes <= plan.max_object_bytes,
        "execution plan object byte budget mismatch"
    );
    ensure!(
        plan.max_decoded_bytes == binding.max_decoded_bytes,
        "execution plan decoded byte budget mismatch"
    );
    ensure!(
        plan.max_source_rows > 0,
        "execution plan max_source_rows must be positive"
    );
    ensure!(
        plan.max_projected_row_groups > 0,
        "execution plan max_projected_row_groups must be positive"
    );
    ensure!(
        plan.max_wall_seconds > 0,
        "execution plan max_wall_seconds must be positive"
    );
    Ok(())
}
fn ensure_object_read_within_raw_payload_limit(spec: &RunSpec) -> Result<()> {
    ensure!(
        spec.accepted_object.bytes <= spec.converter.raw_payload.max_object_bytes,
        "accepted_object.bytes {} exceeds converter.raw_payload.max_object_bytes {}",
        spec.accepted_object.bytes,
        spec.converter.raw_payload.max_object_bytes
    );
    Ok(())
}

fn read_object_checked(path: &Path, expected_bytes: u64) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).with_context(|| format!("stat object {}", path.display()))?;
    let actual_bytes = metadata.len();
    ensure!(
        actual_bytes == expected_bytes,
        "object byte length {actual_bytes} does not match run-spec {expected_bytes}"
    );
    fs::read(path).with_context(|| format!("read object {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            "schema_version": "backfill-execution-plan.v1",
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
        fs::write(&path, serde_json::to_vec_pretty(&plan).unwrap()).unwrap();
        path
    }

    fn run_spec_text_with_catalog_data_type(data_type: String) -> String {
        let mut value: toml::Value =
            toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec TOML parses as value");
        let manifest = value
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
    fn cli_publish_output_flag_is_explicit_opt_in() {
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

        let default_cli = Cli::try_parse_from(base_args).expect("default cli parses");
        assert!(
            !default_cli.publish_output,
            "publishing must default off to avoid implicit external writes"
        );

        let publish_cli = Cli::try_parse_from(
            base_args
                .into_iter()
                .chain(["--publish-output"])
                .collect::<Vec<_>>(),
        )
        .expect("publish cli parses");
        assert!(publish_cli.publish_output);
    }

    #[test]
    fn cli_published_catalog_proof_requires_publish_output() {
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

        let err = Cli::try_parse_from(
            base_args
                .into_iter()
                .chain(["--prove-published-catalog"])
                .collect::<Vec<_>>(),
        )
        .expect_err("catalog proof must require publish-output");
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);

        let cli = Cli::try_parse_from(
            base_args
                .into_iter()
                .chain(["--publish-output", "--prove-published-catalog"])
                .collect::<Vec<_>>(),
        )
        .expect("publish plus catalog proof parses");
        assert!(cli.publish_output);
        assert!(cli.prove_published_catalog);
    }

    #[test]
    fn read_object_rejects_size_mismatch_before_loading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let object_path = dir.path().join("object.csv.gz");
        fs::write(&object_path, b"not-the-accepted-object").unwrap();

        let err = read_object_checked(&object_path, 99).unwrap_err();

        assert!(err.to_string().contains("object byte length 23"), "{err}");
        assert!(err.to_string().contains("run-spec 99"), "{err}");
    }

    #[test]
    fn cli_rejects_plan_object_above_payload_budget_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        fs::write(&run_spec_path, COMMITTED_RUN_SPEC).unwrap();
        let mut spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        spec.converter.raw_payload.max_object_bytes = spec.accepted_object.bytes - 1;
        let run_spec_hash = sha256_hex(COMMITTED_RUN_SPEC.as_bytes());
        let execution_plan = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan,
            object_path: dir.path().join("oversized-object.csv.gz"),
            output_dir: dir.path().join("out"),
            publish_output: false,
            prove_published_catalog: false,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader must not run after configured payload max rejection")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_spec_object_reader_and_resolver(
            &cli,
            spec,
            &run_spec_hash,
            &mut object_reader,
            &mut resolver,
        )
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
            publish_output: false,
            prove_published_catalog: false,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader must not run after manifest rejection")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_object_reader_and_resolver(&cli, &mut object_reader, &mut resolver)
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
            publish_output: false,
            prove_published_catalog: false,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader reached after runtime registry preflight")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_object_reader_and_resolver(&cli, &mut object_reader, &mut resolver)
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
    fn cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object() {
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
            publish_output: true,
            prove_published_catalog: true,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader should not run before publish preflight")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_object_reader_and_resolver(&cli, &mut object_reader, &mut resolver)
            .expect_err("publish preflight must reject before object read");

        assert!(
            err.to_string().contains("artifact_store.ssm_parameters"),
            "{err}"
        );
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
        fs::write(&run_spec_path, COMMITTED_RUN_SPEC).unwrap();
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let execution_plan_path = dir.path().join("execution-plan.json");
        let execution_plan = serde_json::json!({
            "schema_version": "backfill-execution-plan.v1",
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
        fs::write(
            &execution_plan_path,
            serde_json::to_vec_pretty(&execution_plan).unwrap(),
        )
        .unwrap();
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan: execution_plan_path,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
            publish_output: false,
            prove_published_catalog: false,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader must not run after execution-plan mismatch")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_object_reader_and_resolver(&cli, &mut object_reader, &mut resolver)
            .expect_err("execution-plan mismatch must reject before object read");

        assert!(err.to_string().contains("execution plan"), "{err}");
        assert!(!object_reader_called);
    }

    #[test]
    fn cli_execution_plan_table_family_mismatch_rejects_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        fs::write(&run_spec_path, COMMITTED_RUN_SPEC).unwrap();
        let spec: RunSpec = toml::from_str(COMMITTED_RUN_SPEC).expect("run-spec parses");
        let run_spec_hash = sha256_hex(COMMITTED_RUN_SPEC.as_bytes());
        let execution_plan_path = write_matching_execution_plan(dir.path(), &spec, &run_spec_hash);
        let mut execution_plan: serde_json::Value =
            serde_json::from_slice(&fs::read(&execution_plan_path).unwrap()).unwrap();
        execution_plan["table_family"] =
            serde_json::Value::String(format!("{}-mismatch", spec.source_proof.table_family));
        fs::write(
            &execution_plan_path,
            serde_json::to_vec_pretty(&execution_plan).unwrap(),
        )
        .unwrap();
        let cli = Cli {
            run_spec: run_spec_path,
            execution_plan: execution_plan_path,
            object_path: dir.path().join("object.csv.gz"),
            output_dir: dir.path().join("out"),
            publish_output: false,
            prove_published_catalog: false,
        };
        let mut object_reader_called = false;
        let mut object_reader = |_path: &Path, _expected_bytes: u64| {
            object_reader_called = true;
            anyhow::bail!("object reader must not run after execution-plan table-family mismatch")
        };
        let mut resolver = |_region: &str, _path: &str| Ok::<String, String>("unused".to_string());

        let err = run_cli_with_object_reader_and_resolver(&cli, &mut object_reader, &mut resolver)
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
