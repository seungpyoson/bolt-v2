use clap::Parser;
use log::LevelFilter;
use std::path::PathBuf;

use bolt_v2::{
<<<<<<< HEAD
    clients::polymarket,
    config::Config,
    secrets,
    strategies::exec_tester,
=======
    clients::polymarket, config::Config, normalized_sink, secrets, strategies::exec_tester,
>>>>>>> 39dd0f6 (feat: wire NT normalized sink)
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::TraderId;

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
            let cfg = Config::load(&config)?;

            let node = cfg.node;
            let logging = cfg.logging;
            let data_clients = cfg.data_clients;
            let exec_clients = cfg.exec_clients;
            let strategies = cfg.strategies;
            let streaming = cfg.streaming;

            let trader_id = TraderId::from(node.trader_id.as_str());
            let environment = parse_environment(&node.environment)?;
            let log_config = LoggerConfig {
                stdout_level: parse_log_level(&logging.stdout_level)?,
                fileout_level: parse_log_level(&logging.file_level)?,
                ..Default::default()
            };

            let mut builder = LiveNode::builder(trader_id, environment)?
                .with_name(node.name)
                .with_logging(log_config)
                .with_load_state(node.load_state)
                .with_save_state(node.save_state)
                .with_timeout_connection(node.timeout_connection_secs)
                .with_timeout_reconciliation(node.timeout_reconciliation_secs)
                .with_timeout_portfolio(node.timeout_portfolio_secs)
                .with_timeout_disconnection_secs(node.timeout_disconnection_secs)
                .with_delay_post_stop_secs(node.delay_post_stop_secs)
                .with_delay_shutdown_secs(node.delay_shutdown_secs);

            for client in &data_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        let (factory, config) = polymarket::build_data_client(&client.config)?;
                        builder =
                            builder.add_data_client(Some(client.name.clone()), factory, config)?;
                    }
                    other => return Err(format!("Unsupported data client type: {other}").into()),
                }
            }

            for client in &exec_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        let resolved = secrets::resolve_polymarket(&client.secrets)?;
                        let (factory, config) =
                            polymarket::build_exec_client(&client.config, trader_id, resolved)?;
                        builder =
                            builder.add_exec_client(Some(client.name.clone()), factory, config)?;
                    }
                    other => return Err(format!("Unsupported exec client type: {other}").into()),
                }
            }

            let mut node = builder.build()?;
            let _normalized_sink_guards = if streaming.catalog_path.trim().is_empty() {
                None
            } else {
                Some(normalized_sink::wire_normalized_sinks(
                    &node,
                    &streaming.catalog_path,
                    streaming.flush_interval_ms,
                )?)
            };

            for strategy in &strategies {
                match strategy.kind.as_str() {
                    "exec_tester" => {
                        node.add_strategy(exec_tester::build_exec_tester(&strategy.config)?)?;
                    }
                    other => return Err(format!("Unsupported strategy type: {other}").into()),
                }
            }

            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(node.run())?;

            Ok(())
        }
    }
}

fn run_secrets_command(command: SecretsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SecretsCommand::Check { config } => {
            let cfg = Config::load(&config)?;
            let mut has_errors = false;

            for client in &cfg.exec_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        let check = secrets::check_polymarket_secret_config(&client.secrets);
                        if check.is_complete() {
                            println!(
                                "{}: secret config complete ({})",
                                client.name,
                                check.present.join(", ")
                            );
                        } else {
                            has_errors = true;
                            eprintln!(
                                "{}: missing secret config fields ({})",
                                client.name,
                                check.missing.join(", ")
                            );
                        }
                    }
                    other => return Err(format!("Unsupported exec client type: {other}").into()),
                }
            }

            if has_errors {
                Err("One or more exec clients have incomplete secret configuration".into())
            } else {
                Ok(())
            }
        }
        SecretsCommand::Resolve { config } => {
            let cfg = Config::load(&config)?;

            for client in &cfg.exec_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        secrets::resolve_polymarket(&client.secrets)?;
                        println!("{}: secrets resolved successfully", client.name);
                    }
                    other => return Err(format!("Unsupported exec client type: {other}").into()),
                }
            }

            Ok(())
        }
    }
}

fn parse_environment(s: &str) -> Result<Environment, Box<dyn std::error::Error>> {
    match s {
        "Live" => Ok(Environment::Live),
        "Sandbox" => Ok(Environment::Sandbox),
        other => Err(format!("Unknown environment: {other}").into()),
    }
}

fn parse_log_level(s: &str) -> Result<LevelFilter, Box<dyn std::error::Error>> {
    match s {
        "Trace" => Ok(LevelFilter::Trace),
        "Debug" => Ok(LevelFilter::Debug),
        "Info" => Ok(LevelFilter::Info),
        "Warn" => Ok(LevelFilter::Warn),
        "Error" => Ok(LevelFilter::Error),
        "Off" => Ok(LevelFilter::Off),
        other => Err(format!("Unknown log level: {other}").into()),
    }
}
