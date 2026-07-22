use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use bolt_v2::bolt_v3_decision_evidence::contract_generator::{
    parse_contract_registry, render_contract_rust,
};

const REGISTRY_PATH: &str = "config/decision-evidence-contract.toml";
const GENERATED_PATH: &str = "src/bolt_v3_decision_evidence/generated_contract.rs";

fn main() -> Result<()> {
    let registry_source = fs::read_to_string(REGISTRY_PATH)
        .with_context(|| format!("failed to read `{REGISTRY_PATH}`"))?;
    let registry = parse_contract_registry(&registry_source)?;
    let rendered = render_contract_rust(&registry)?;
    fs::write(Path::new(GENERATED_PATH), rendered)
        .with_context(|| format!("failed to write `{GENERATED_PATH}`"))?;
    Ok(())
}
