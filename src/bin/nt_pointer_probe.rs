use std::path::PathBuf;

use anyhow::Result;
use bolt_v2::nt_pointer_probe::control::{
    ExpectedBranchProtection, LoadedControlPlane, compare_branch_governance_responses,
    compare_branch_protection_response,
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
    PrintNtCrateDiffPattern {
        #[arg(long)]
        repo_root: PathBuf,
    },
    CompareBranchProtection {
        #[arg(long)]
        expected: PathBuf,
        #[arg(long)]
        actual_json: PathBuf,
    },
    CompareBranchGovernance {
        #[arg(long)]
        expected: PathBuf,
        #[arg(long)]
        actual_json: PathBuf,
        #[arg(long)]
        actual_rules_json: PathBuf,
        #[arg(long)]
        actual_ruleset_details_json: PathBuf,
    },
    CheckNtMutation {
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        base_ref: String,
        #[arg(long)]
        head_ref: String,
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
        Command::PrintNtCrateDiffPattern { repo_root } => {
            let loaded = LoadedControlPlane::load_from_repo_root(&repo_root)?;
            println!("{}", loaded.nt_crate_diff_pattern());
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
        Command::CompareBranchGovernance {
            expected,
            actual_json,
            actual_rules_json,
            actual_ruleset_details_json,
        } => {
            let expected = ExpectedBranchProtection::load_and_validate(&expected)?;
            let actual_json = std::fs::read_to_string(&actual_json)?;
            let actual_rules_json = std::fs::read_to_string(&actual_rules_json)?;
            let actual_ruleset_details_json =
                std::fs::read_to_string(&actual_ruleset_details_json)?;
            compare_branch_governance_responses(
                &expected,
                &actual_json,
                &actual_rules_json,
                &actual_ruleset_details_json,
            )?;
            println!(
                "branch governance matches expected state for {}",
                expected.branch
            );
        }
        Command::CheckNtMutation {
            repo_root,
            base_ref,
            head_ref,
        } => {
            let loaded = LoadedControlPlane::load_from_repo_root(&repo_root)?;
            loaded.ensure_no_nt_mutation_from_git_refs(&base_ref, &head_ref)?;
            println!(
                "no unmanaged NT mutations detected between {} and {}",
                base_ref, head_ref
            );
        }
    }

    Ok(())
}
