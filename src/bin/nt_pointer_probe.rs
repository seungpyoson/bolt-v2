use std::path::PathBuf;

use bolt_v2::nt_pointer_probe::control::{
    LoadedControlPlane, compare_branch_governance_responses, compare_branch_protection_response,
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
        repo_root: PathBuf,
        #[arg(long)]
        actual_json: PathBuf,
    },
    CompareBranchGovernance {
        #[arg(long)]
        repo_root: PathBuf,
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

fn exit_on_error<T, E>(result: Result<T, E>) -> T
where
    E: std::fmt::Display,
{
    match result {
        Ok(value) => value,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Command::ValidateControlPlane { repo_root } => {
            let loaded = exit_on_error(LoadedControlPlane::load_from_repo_root(&repo_root));
            println!(
                "validated control plane for {} with {} seams, {} safe-list entries, and {} replay fixtures",
                loaded.control.repo,
                loaded.registry.seams.len(),
                loaded.safe_list.entries.len(),
                loaded.replay_set.entries.len()
            );
        }
        Command::PrintNtCrateDiffPattern { repo_root } => {
            let loaded = exit_on_error(LoadedControlPlane::load_from_repo_root(&repo_root));
            println!("{}", loaded.nt_crate_diff_pattern());
        }
        Command::CompareBranchProtection {
            repo_root,
            actual_json,
        } => {
            let loaded = exit_on_error(LoadedControlPlane::load_from_repo_root(&repo_root));
            let actual_json = exit_on_error(std::fs::read_to_string(&actual_json));
            exit_on_error(compare_branch_protection_response(
                &loaded.expected_branch_protection,
                &actual_json,
            ));
            println!(
                "branch protection matches expected state for {}",
                loaded.expected_branch_protection.branch
            );
        }
        Command::CompareBranchGovernance {
            repo_root,
            actual_json,
            actual_rules_json,
            actual_ruleset_details_json,
        } => {
            let loaded = exit_on_error(LoadedControlPlane::load_from_repo_root(&repo_root));
            let actual_json = exit_on_error(std::fs::read_to_string(&actual_json));
            let actual_rules_json = exit_on_error(std::fs::read_to_string(&actual_rules_json));
            let actual_ruleset_details_json =
                exit_on_error(std::fs::read_to_string(&actual_ruleset_details_json));
            exit_on_error(compare_branch_governance_responses(
                &loaded.expected_branch_protection,
                &actual_json,
                &actual_rules_json,
                &actual_ruleset_details_json,
            ));
            println!(
                "branch governance matches expected state for {}",
                loaded.expected_branch_protection.branch
            );
        }
        Command::CheckNtMutation {
            repo_root,
            base_ref,
            head_ref,
        } => {
            let loaded = exit_on_error(LoadedControlPlane::load_from_repo_root(&repo_root));
            exit_on_error(loaded.ensure_no_nt_mutation_from_git_refs(&base_ref, &head_ref));
            println!(
                "no unmanaged NT mutations detected between {} and {}",
                base_ref, head_ref
            );
        }
    }
}
