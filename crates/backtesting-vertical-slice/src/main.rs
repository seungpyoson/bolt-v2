//! Binary: run the NautilusTrader backtesting vertical slice over an accepted
//! dataset object.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven run-spec TOML; the only
//! command-line inputs are filesystem paths. The orchestration itself lives in
//! [`backtesting_vertical_slice::operator`] so it is unit-testable; this binary
//! is a thin CLI shim that reads the inputs, runs the operator path, and prints
//! the produced artifacts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;

use backtesting_vertical_slice::{
    artifact_store_secrets::{ArtifactStoreSecretResolver, ArtifactStoreSsmResolver},
    operator::{
        PublishOptions, PublishedArtifact, PublishedCatalogProof, RunSpec, run_from_run_spec,
        run_from_run_spec_and_publish_with_resolved_storage_options,
    },
};

#[derive(Debug, Parser)]
#[command(about = "Run the NautilusTrader backtesting vertical slice over an accepted dataset.")]
struct Cli {
    /// Path to the run-spec TOML (dataset facts: object, source proof, manifest).
    #[arg(long)]
    run_spec: PathBuf,
    /// Local path to the accepted `.csv.gz` object whose SHA-256 the run-spec pins.
    #[arg(long)]
    object_gz: PathBuf,
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

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut object_reader =
        |path: &Path, expected_bytes: u64| read_object_gz_checked(path, expected_bytes);
    let spec_text = fs::read_to_string(&cli.run_spec)
        .with_context(|| format!("read run-spec {}", cli.run_spec.display()))?;
    let spec: RunSpec = toml::from_str(&spec_text).context("parse run-spec TOML")?;
    if cli.publish_output && spec.manifest.artifact_store.ssm_parameters.is_some() {
        let mut resolver = ArtifactStoreSsmResolver::new()?;
        run_cli_with_spec_object_reader_and_resolver(&cli, spec, &mut object_reader, &mut resolver)
    } else {
        let mut resolver = |_region: &str, _path: &str| {
            Err::<String, String>("artifact-store SSM resolver was not configured".to_string())
        };
        run_cli_with_spec_object_reader_and_resolver(&cli, spec, &mut object_reader, &mut resolver)
    }
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
    let spec_text = fs::read_to_string(&cli.run_spec)
        .with_context(|| format!("read run-spec {}", cli.run_spec.display()))?;
    let spec: RunSpec = toml::from_str(&spec_text).context("parse run-spec TOML")?;
    run_cli_with_spec_object_reader_and_resolver(cli, spec, object_reader, resolver)
}

fn run_cli_with_spec_object_reader_and_resolver<F, R>(
    cli: &Cli,
    spec: RunSpec,
    object_reader: &mut F,
    resolver: &mut R,
) -> Result<()>
where
    F: FnMut(&Path, u64) -> Result<Vec<u8>>,
    R: ArtifactStoreSecretResolver,
{
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
    let gz_bytes = object_reader(&cli.object_gz, spec.accepted_object.bytes)?;

    let (artifacts, published_artifacts, published_catalog_proof): (
        _,
        Option<Vec<PublishedArtifact>>,
        Option<PublishedCatalogProof>,
    ) = if cli.publish_output {
        let published = run_from_run_spec_and_publish_with_resolved_storage_options(
            &spec,
            &gz_bytes,
            &cli.output_dir,
            publish_options,
            resolved_publish_storage_options.as_ref(),
        )?;
        (
            published.run,
            Some(published.published_artifacts),
            published.published_catalog_proof,
        )
    } else {
        (
            run_from_run_spec(&spec, &gz_bytes, &cli.output_dir)?,
            None,
            None,
        )
    };
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
    println!("nt_catalog_root = {}", artifacts.catalog_root.display());
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
    Ok(())
}

fn read_object_gz_checked(path: &Path, expected_bytes: u64) -> Result<Vec<u8>> {
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

    #[test]
    fn cli_publish_output_flag_is_explicit_opt_in() {
        let base_args = [
            "backtesting-vertical-slice",
            "--run-spec",
            "run.toml",
            "--object-gz",
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
            "--object-gz",
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
    fn read_object_gz_rejects_size_mismatch_before_loading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let object_path = dir.path().join("object.csv.gz");
        fs::write(&object_path, b"not-the-accepted-object").unwrap();

        let err = read_object_gz_checked(&object_path, 99).unwrap_err();

        assert!(err.to_string().contains("object byte length 23"), "{err}");
        assert!(err.to_string().contains("run-spec 99"), "{err}");
    }

    #[test]
    fn cli_publish_preflight_rejects_missing_s3_ssm_before_reading_object() {
        let dir = tempfile::TempDir::new().unwrap();
        let run_spec_path = dir.path().join("run.toml");
        fs::write(&run_spec_path, COMMITTED_RUN_SPEC).unwrap();
        let cli = Cli {
            run_spec: run_spec_path,
            object_gz: dir.path().join("missing-object.csv.gz"),
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
}
