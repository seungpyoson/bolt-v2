use std::path::PathBuf;

use anyhow::Result;
use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_protection_response,
};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "nt_pointer_probe")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    ValidateControlPlane {
        #[arg(long)]
        repo_root: PathBuf,
    },
    CompareBranchProtection {
        #[arg(long)]
        expected: PathBuf,
        #[arg(long)]
        actual_json: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::ValidateControlPlane { repo_root } => {
            let loaded = LoadedControlPlane::load_from_repo_root(&repo_root)?;
            println!(
                "validated control plane for {} with {} seams, {} safe-list entries, and {} replay fixtures",
                loaded.control.repo,
                loaded.registry.seams.len(),
                loaded.safe_list.entries.len(),
                loaded.replay_set.entries.len()
            );
        }
        Command::CompareBranchProtection {
            expected,
            actual_json,
        } => {
            let expected = ExpectedBranchProtection::load_and_validate(&expected)?;
            let actual_json = std::fs::read_to_string(&actual_json)?;
            compare_branch_protection_response(&expected, &actual_json)?;
            println!(
                "branch protection matches expected state for {}",
                expected.branch
            );
        }
    }

    Ok(())
}
