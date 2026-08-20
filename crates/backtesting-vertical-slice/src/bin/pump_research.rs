use std::path::PathBuf;

use backtesting_vertical_slice::research_experiment::{
    ExperimentError, load_and_validate_experiment, register_version,
};
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
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    status: &'static str,
    reason_code: &'a str,
    message: String,
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
    let output: Result<serde_json::Value, ExperimentError> = match cli.command {
        Command::Validate { spec } => load_and_validate_experiment(&spec).map(|experiment| {
            serde_json::to_value(experiment.summary())
                .expect("validation summary serialization is infallible")
        }),
        Command::RegisterVersion {
            spec,
            expected_parent_version_id,
        } => register_version(&spec, expected_parent_version_id.as_deref())
            .await
            .map(|summary| {
                serde_json::to_value(summary)
                    .expect("registration summary serialization is infallible")
            }),
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
            emit_error(error.reason_code(), error.to_string());
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
