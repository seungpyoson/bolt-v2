use std::path::PathBuf;

use bolt_v2::{MaterializationOutcome, materialize_live_config};
use clap::Parser;

#[derive(Parser)]
#[command(name = "render_live_config")]
struct Cli {
    #[arg(long)]
    input: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let outcome = materialize_live_config(&cli.input, &cli.output)?;

    match outcome {
        MaterializationOutcome::Created => println!(
            "Generated config created: {} from {}",
            cli.output.display(),
            cli.input.display()
        ),
        MaterializationOutcome::Updated => println!(
            "Generated config updated: {} from {}",
            cli.output.display(),
            cli.input.display()
        ),
        MaterializationOutcome::PermissionsRepaired => println!(
            "Generated config permissions repaired: {} from {}",
            cli.output.display(),
            cli.input.display()
        ),
        MaterializationOutcome::Unchanged => {
            println!("Generated config unchanged: {}", cli.output.display())
        }
    }

    Ok(())
}
