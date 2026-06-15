use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::{
    artifact_store_secrets::ArtifactStoreSsmResolver,
    source_proof_evidence_staging::{
        SourceProofEvidenceStagingManifest,
        stage_source_proof_evidence_from_spec_file_with_resolver,
    },
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Verify and stage source-proof evidence files")]
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
    let artifact =
        stage_source_proof_evidence_from_spec_file_with_resolver(&cli.spec, &mut resolve_secret)?;
    let manifest: SourceProofEvidenceStagingManifest =
        serde_json::from_slice(&fs::read(&artifact.manifest_path)?)?;
    println!(
        "source_proof_evidence_staging_manifest = {}",
        artifact.manifest_path.display()
    );
    println!("manifest_hash = {}", artifact.manifest_hash);
    println!("manifest_bytes = {}", artifact.manifest_bytes);
    println!("record_count = {}", manifest.record_count);
    println!("total_bytes = {}", manifest.total_bytes);
    Ok(())
}
