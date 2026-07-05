use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use backtesting_vertical_slice::artifact_index_iam_policy::{
    ARTIFACT_INDEX_PRODUCER_IAM_PROVISIONING_PLAN_ROLE, ArtifactIndexProducerIamProvisioningPlan,
    ArtifactIndexProducerIamProvisioningPlanSpec, artifact_index_producer_iam_provisioning_plan,
};
use clap::Parser;
use serde::Deserialize;

#[derive(Debug, Parser)]
#[command(about = "Write a per-kind Artifact Index producer IAM provisioning plan from TOML")]
struct Cli {
    #[arg(long)]
    spec: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ProvisioningPlanFile {
    output_path: PathBuf,
    #[serde(flatten)]
    spec: ArtifactIndexProducerIamProvisioningPlanSpec,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec_text = fs::read_to_string(&cli.spec)
        .with_context(|| format!("read provisioning plan spec {}", cli.spec.display()))?;
    let file: ProvisioningPlanFile = toml::from_str(&spec_text)
        .with_context(|| format!("parse provisioning plan spec {}", cli.spec.display()))?;
    let plan = artifact_index_producer_iam_provisioning_plan(file.spec)?;
    let output_path = resolve_output_path(&cli.spec, &file.output_path)?;
    write_plan(&output_path, &plan)?;

    println!(
        "artifact_index_iam_provisioning_plan = {}",
        output_path.display()
    );
    println!("artifact_kind = {}", plan.artifact_kind.as_str());
    println!(
        "access_key_id_parameter = {}",
        plan.ssm_parameter_paths.access_key_id
    );
    println!(
        "secret_access_key_parameter = {}",
        plan.ssm_parameter_paths.secret_access_key
    );
    println!(
        "expected_denied_write_attempts = {}",
        plan.expected_denied_write_attempts
    );
    Ok(())
}

fn resolve_output_path(spec_path: &Path, output_path: &Path) -> Result<PathBuf> {
    if output_path.is_absolute() {
        return Ok(output_path.to_path_buf());
    }
    let spec_dir = spec_path
        .parent()
        .with_context(|| format!("resolve parent for spec {}", spec_path.display()))?;
    Ok(spec_dir.join(output_path))
}

fn write_plan(path: &Path, plan: &ArtifactIndexProducerIamProvisioningPlan) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create provisioning plan directory {}", parent.display()))?;
    }
    backtesting_vertical_slice::reference_artifact::write_reference_artifact(
        path,
        ARTIFACT_INDEX_PRODUCER_IAM_PROVISIONING_PLAN_ROLE,
        plan,
    )
    .map(|_| ())
    .with_context(|| format!("write provisioning plan {}", path.display()))
}
