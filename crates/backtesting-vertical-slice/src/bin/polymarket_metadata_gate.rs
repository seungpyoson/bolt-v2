//! Generate a Polymarket NT metadata-gate report from a config-owned TOML spec.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use backtesting_vertical_slice::{
    path_resolution::resolve_output_dir,
    polymarket_metadata_gate::{
        POLYMARKET_METADATA_GATE_REPORT_SCHEMA_VERSION, PolymarketMetadataGateSpec,
        evaluate_polymarket_metadata_gate_with_base,
    },
};
use clap::Parser;
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(about = "Write a Polymarket NT metadata-gate report from a TOML spec.")]
struct Cli {
    /// Path to the Polymarket metadata gate spec TOML.
    #[arg(long)]
    spec: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSpec {
    source_binding: String,
    selected_token_id: String,
    selected_condition_id: String,
    gamma_markets_path: PathBuf,
    output_path: PathBuf,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_text = fs::read_to_string(&cli.spec)
        .with_context(|| format!("read metadata gate spec {}", cli.spec.display()))?;
    let file_spec: FileSpec = toml::from_str(&spec_text)
        .with_context(|| format!("parse metadata gate spec TOML {}", cli.spec.display()))?;
    let base_dir = cli.spec.parent().unwrap_or_else(|| std::path::Path::new("."));
    let output_path = resolve_output_dir(base_dir, &file_spec.output_path);
    let report = evaluate_polymarket_metadata_gate_with_base(
        &PolymarketMetadataGateSpec {
            source_binding: file_spec.source_binding,
            selected_token_id: file_spec.selected_token_id,
            selected_condition_id: file_spec.selected_condition_id,
            gamma_markets_path: file_spec.gamma_markets_path,
        },
        base_dir,
    )?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create metadata gate report dir {}", parent.display()))?;
    }
    backtesting_vertical_slice::reference_artifact::write_reference_artifact(
        &output_path,
        POLYMARKET_METADATA_GATE_REPORT_SCHEMA_VERSION,
        &report,
        backtesting_vertical_slice::reference_artifact::ReferenceArtifactRewrite::OverwriteAlways,
    )
    .with_context(|| {
        format!("write metadata gate report {}", output_path.display())
    })?;

    let status = serde_json::to_value(report.status)?
        .as_str()
        .unwrap_or_default()
        .to_string();
    println!("polymarket_metadata_gate_report = {}", output_path.display());
    println!("status = {status}");
    println!("gamma_market_count = {}", report.gamma_market_count);
    println!(
        "matching_gamma_market_count = {}",
        report.matching_gamma_market_count
    );
    println!(
        "selected_token_nt_def_count = {}",
        report.selected_token_nt_def_count
    );
    println!("gamma_markets_sha256 = {}", report.gamma_markets_sha256);
    Ok(())
}
