use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use bolt_v2::bolt_v3_evidence_novelty::generator::{parse_registry, render_registry};

const REGISTRY_PATH: &str = "config/evidence-novelty.toml";
const GENERATED_PATH: &str = "src/bolt_v3_evidence_novelty/generated.rs";

fn main() -> Result<()> {
    let registry_text =
        fs::read_to_string(REGISTRY_PATH).with_context(|| format!("reading {REGISTRY_PATH}"))?;
    let registry = parse_registry(&registry_text)?;
    let rendered = render_registry(&registry)?;
    fs::write(Path::new(GENERATED_PATH), rendered)
        .with_context(|| format!("writing {GENERATED_PATH}"))?;
    Ok(())
}
