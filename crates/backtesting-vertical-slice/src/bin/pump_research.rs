use std::path::PathBuf;

use backtesting_vertical_slice::research_experiment::{
    ExperimentError, load_and_validate_experiment, register_version,
};
use backtesting_vertical_slice::source_proof::stage_research_source_registration_from_toml_file;
use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(name = "pump-research")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Validate {
        #[arg(long)]
        spec: PathBuf,
    },
    RegisterVersion {
        #[arg(long)]
        spec: PathBuf,
        #[arg(long)]
        expected_parent_version_id: Option<String>,
    },
    RegisterSource {
        #[arg(long)]
        source_entry: PathBuf,
    },
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    status: &'static str,
    reason_code: &'a str,
    message: String,
}

struct CommandError {
    reason_code: &'static str,
    message: String,
}

impl From<ExperimentError> for CommandError {
    fn from(error: ExperimentError) -> Self {
        Self {
            reason_code: error.reason_code(),
            message: error.to_string(),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
            ) =>
        {
            let _ = error.print();
            return std::process::ExitCode::SUCCESS;
        }
        Err(error) => {
            emit_error("invalid_command", error.to_string());
            return std::process::ExitCode::from(2);
        }
    };
    let output: Result<serde_json::Value, CommandError> = match cli.command {
        Command::Validate { spec } => load_and_validate_experiment(&spec)
            .map(|experiment| {
                serde_json::to_value(experiment.summary())
                    .expect("validation summary serialization is infallible")
            })
            .map_err(CommandError::from),
        Command::RegisterVersion {
            spec,
            expected_parent_version_id,
        } => register_version(&spec, expected_parent_version_id.as_deref())
            .await
            .map(|summary| {
                serde_json::to_value(summary)
                    .expect("registration summary serialization is infallible")
            })
            .map_err(CommandError::from),
        Command::RegisterSource { source_entry } => {
            stage_research_source_registration_from_toml_file(&source_entry)
                .map(|summary| {
                    serde_json::to_value(summary)
                        .expect("source staging summary serialization is infallible")
                })
                .map_err(|error| CommandError {
                    reason_code: "source_admission_failed",
                    message: error.to_string(),
                })
        }
    };
    match output {
        Ok(output) => {
            println!(
                "{}",
                serde_json::to_string(&output).expect("JSON value serialization is infallible")
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            emit_error(error.reason_code, error.message);
            std::process::ExitCode::FAILURE
        }
    }
}

fn emit_error(reason_code: &str, message: String) {
    let envelope = ErrorEnvelope {
        status: "error",
        reason_code,
        message,
    };
    eprintln!(
        "{}",
        serde_json::to_string(&envelope).expect("error envelope serialization is infallible")
    );
}
