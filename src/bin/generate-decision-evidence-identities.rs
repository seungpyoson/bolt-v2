use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

#[path = "../bolt_v3_decision_evidence/identity_generator.rs"]
mod identity_generator;

use identity_generator::{parse_registry, render_registry, validate_append_only_compatibility};

const REGISTRY_PATH: &str = "config/decision-evidence-identities.toml";
const FROZEN_REGISTRY_PATH: &str = "config/decision-evidence-identities-frozen.toml";
const GENERATED_PATH: &str = "src/bolt_v3_decision_evidence/generated_identities.rs";

fn main() -> Result<()> {
    let text =
        fs::read_to_string(REGISTRY_PATH).with_context(|| format!("reading {REGISTRY_PATH}"))?;
    let registry = parse_registry(&text)?;
    let frozen_text = fs::read_to_string(FROZEN_REGISTRY_PATH)
        .with_context(|| format!("reading {FROZEN_REGISTRY_PATH}"))?;
    let frozen_registry = parse_registry(&frozen_text)?;
    validate_append_only_compatibility(&frozen_registry, &registry)?;
    let rendered = render_registry(&registry)?;
    fs::write(Path::new(GENERATED_PATH), rendered)
        .with_context(|| format!("writing {GENERATED_PATH}"))?;
    Ok(())
}
