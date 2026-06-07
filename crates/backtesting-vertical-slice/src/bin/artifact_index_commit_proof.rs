use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::{
    artifact_index_commit_proof::{
        ArtifactIndexCommitProofReport,
        run_artifact_index_commit_proof_from_spec_file_with_resolver,
    },
    artifact_store_secrets::ArtifactStoreSsmResolver,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Prove Artifact Index conditional commit semantics against a configured store")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut resolver = ArtifactStoreSsmResolver::new()?;
    let mut resolve_secret = |region: &str, path: &str| {
        resolver
            .resolve(region, path)
            .map_err(|error| error.to_string())
    };
    let artifact = run_artifact_index_commit_proof_from_spec_file_with_resolver(
        &cli.spec,
        &mut resolve_secret,
    )?;
    let report: ArtifactIndexCommitProofReport =
        serde_json::from_slice(&fs::read(&artifact.report_path)?)?;
    println!(
        "artifact_index_commit_proof_report = {}",
        artifact.report_path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("report_bytes = {}", artifact.report_bytes);
    println!("artifact_root = {}", artifact.artifact_root);
    println!("latest_pointer_uri = {}", artifact.latest_pointer_uri);
    println!(
        "latest_pointer_update_if_match_proven = {}",
        report.latest_pointer_update_if_match_proven
    );
    println!(
        "stale_etag_update_rejected = {}",
        report.stale_etag_update_rejected
    );
    println!(
        "direct_s3_commit_proven = {}",
        report.direct_s3_commit_proven
    );
    println!(
        "producer_iam_scope_proven = {}",
        report.producer_iam_scope_proven
    );
    Ok(())
}
