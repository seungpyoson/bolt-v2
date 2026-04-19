use std::path::PathBuf;

use clap::{Parser, ValueEnum};

#[path = "../delivery_validator.rs"]
mod delivery_validator;

use delivery_validator::{Stage, render_report, validate_dir};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum StageArg {
    Intake,
    #[value(alias = "seam_locked")]
    SeamLocked,
    #[value(alias = "proof_locked")]
    ProofLocked,
    Review,
    #[value(alias = "merge_candidate")]
    MergeCandidate,
}

impl From<StageArg> for Stage {
    fn from(value: StageArg) -> Self {
        match value {
            StageArg::Intake => Stage::Intake,
            StageArg::SeamLocked => Stage::SeamLocked,
            StageArg::ProofLocked => Stage::ProofLocked,
            StageArg::Review => Stage::Review,
            StageArg::MergeCandidate => Stage::MergeCandidate,
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "process_validator")]
struct Cli {
    #[arg(long)]
    delivery_dir: PathBuf,
    #[arg(long, value_enum)]
    stage: StageArg,
}

fn main() {
    let cli = Cli::parse();
    match validate_dir(&cli.delivery_dir, cli.stage.into()) {
        Ok(report) => {
            print!("{}", render_report(&report));
            if report.has_block() {
                std::process::exit(1);
            }
        }
        Err(err) => {
            eprintln!("STATUS: BLOCK");
            eprintln!("KIND: schema");
            eprintln!("WHERE: {}", cli.delivery_dir.display());
            eprintln!("WHY: {err}");
            eprintln!("NEXT: fix the artifact package before re-running validation");
            std::process::exit(1);
        }
    }
}
