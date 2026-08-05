use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use bolt_v2::bolt_v3_current_evidence::contract_generator::{
    parse_contract_registry, render_contract,
};

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let registry_path = root.join("config/decision-evidence-contract.toml");
    let output_path = root.join("src/bolt_v3_current_evidence/generated_contract.rs");
    let registry = fs::read_to_string(&registry_path)
        .with_context(|| format!("failed to read `{}`", registry_path.display()))?;
    let contract = parse_contract_registry(&registry)?;
    fs::write(&output_path, render_contract(&contract))
        .with_context(|| format!("failed to write `{}`", output_path.display()))?;
    Ok(())
}
