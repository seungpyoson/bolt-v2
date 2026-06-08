//! Generate a source catalog-mapping readiness report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::Result;
use backtesting_vertical_slice::source_catalog_mapping_readiness::{
    SourceCatalogMappingReadinessReport,
    write_source_catalog_mapping_readiness_report_from_spec_file,
};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(about = "Write a source catalog-mapping readiness report from a TOML spec.")]
struct Cli {
    /// Path to the source catalog-mapping readiness spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let artifact = write_source_catalog_mapping_readiness_report_from_spec_file(&cli.spec)?;
    let report: SourceCatalogMappingReadinessReport =
        serde_json::from_slice(&fs::read(&artifact.path)?)?;
    println!(
        "source_catalog_mapping_readiness_report = {}",
        artifact.path.display()
    );
    println!("content_hash = {}", artifact.content_hash);
    println!("bytes = {}", artifact.bytes);
    println!("status = {:?}", report.status);
    println!("blockers = {}", report.blockers.len());
    Ok(())
}
