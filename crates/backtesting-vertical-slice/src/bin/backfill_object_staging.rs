use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::{
    artifact_store_secrets::ArtifactStoreSsmResolver,
    backfill_object_staging::{
        BackfillObjectStagingManifest, stage_backfill_object_from_spec_file_with_resolver,
    },
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Verify and stage exactly one raw backfill object")]
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
        stage_backfill_object_from_spec_file_with_resolver(&cli.spec, &mut resolve_secret)?;
    let manifest: BackfillObjectStagingManifest =
        serde_json::from_slice(&fs::read(&artifact.manifest_path)?)?;
    println!(
        "backfill_object_staging_manifest = {}",
        artifact.manifest_path.display()
    );
    println!("manifest_hash = {}", artifact.manifest_hash);
    println!("manifest_bytes = {}", artifact.manifest_bytes);
    println!("object_uri = {}", artifact.object_uri);
    println!("object_sha256 = {}", artifact.object_sha256);
    println!("object_bytes = {}", artifact.object_bytes);
    println!("payload_records = {}", manifest.payload_records.len());
    Ok(())
}
