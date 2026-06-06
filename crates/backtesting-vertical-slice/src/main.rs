//! Binary: run the NautilusTrader backtesting vertical slice over an accepted
//! dataset object.
//!
//! Everything that identifies the dataset (accepted object, source proof, run
//! manifest, instrument spec) comes from a config-driven run-spec TOML; the only
//! command-line inputs are filesystem paths. The orchestration itself lives in
//! [`backtesting_vertical_slice::operator`] so it is unit-testable; this binary
//! is a thin CLI shim that reads the inputs, runs the operator path, and prints
//! the produced artifacts.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use backtesting_vertical_slice::{
    artifact_store_secrets::{ArtifactStoreSecretResolver, ArtifactStoreSsmResolver},
    operator::{
        PublishOptions, PublishedArtifact, PublishedCatalogProof, RunSpec, run_from_run_spec,
        run_from_run_spec_and_publish_with_options, run_from_run_spec_and_publish_with_resolver,
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
    let spec_text = fs::read_to_string(&cli.run_spec)
        .with_context(|| format!("read run-spec {}", cli.run_spec.display()))?;
    let spec: RunSpec = toml::from_str(&spec_text).context("parse run-spec TOML")?;
    let gz_bytes = fs::read(&cli.object_gz)
        .with_context(|| format!("read object {}", cli.object_gz.display()))?;

    let (artifacts, published_artifacts, published_catalog_proof): (
        _,
        Option<Vec<PublishedArtifact>>,
        Option<PublishedCatalogProof>,
    ) = if cli.publish_output {
        let publish_options = PublishOptions {
            prove_published_catalog: cli.prove_published_catalog,
        };
        let published = if spec.manifest.artifact_store.ssm_parameters.is_some() {
            let mut resolver = ArtifactStoreSsmResolver::new()?;
            run_from_run_spec_and_publish_with_resolver(
                &spec,
                &gz_bytes,
                &cli.output_dir,
                publish_options,
                &mut |region, path| resolver.resolve_secret(region, path),
            )?
        } else {
            run_from_run_spec_and_publish_with_options(
                &spec,
                &gz_bytes,
                &cli.output_dir,
                publish_options,
            )?
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind;

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
}
