use clap::Parser;
use std::path::PathBuf;

use bolt_v2::{
    bolt_v3_config::load_bolt_v3_config,
    bolt_v3_live_node::{build_bolt_v3_live_node, run_bolt_v3_live_node},
    bolt_v3_providers::binding_for_provider_key,
    bolt_v3_secrets::{check_no_forbidden_credential_env_vars, resolve_bolt_v3_secrets},
    secrets::SsmResolverSession,
};

#[derive(Parser)]
#[command(name = "bolt-v2")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    Run {
        #[arg(short, long)]
        config: PathBuf,
    },
    Secrets {
        #[command(subcommand)]
        command: SecretsCommand,
    },
}

#[derive(clap::Subcommand)]
enum SecretsCommand {
    Check {
        #[arg(short, long)]
        config: PathBuf,
    },
    Resolve {
        #[arg(short, long)]
        config: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Secrets { command } => run_secrets_command(command),
        Command::Run { config } => {
            bolt_v2::log_sweep::sweep_stale_logs();
            let loaded = load_bolt_v3_config(&config)?;
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            let app = async move {
                let mut node = build_bolt_v3_live_node(&loaded)?;
                run_bolt_v3_live_node(&mut node, &loaded).await?;
                Ok(())
            };
            runtime.block_on(local.run_until(app))
        }
    }
}

fn run_secrets_command(command: SecretsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SecretsCommand::Check { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            for (venue_key, venue) in &loaded.root.venues {
                if venue.secrets.is_some() {
                    let binding =
                        binding_for_provider_key(venue.kind.as_str()).ok_or_else(|| {
                            format!(
                                "venues.{venue_key}.kind `{}` is not supported by this build",
                                venue.kind.as_str()
                            )
                        })?;
                    println!(
                        "venues.{venue_key}: required secret fields present ({})",
                        binding.secret_field_names.join(", ")
                    );
                }
            }
            Ok(())
        }
        SecretsCommand::Resolve { config } => {
            let loaded = load_bolt_v3_config(&config)?;
            check_no_forbidden_credential_env_vars(&loaded.root)?;
            let ssm_resolver_session = SsmResolverSession::new()?;
            let resolved = resolve_bolt_v3_secrets(&ssm_resolver_session, &loaded)?;
            for venue_key in resolved.venues.keys() {
                println!("venues.{venue_key}: secrets resolved successfully");
            }
            Ok(())
        }
    }
}
