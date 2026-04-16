use clap::Parser;
use log::LevelFilter;
use std::{collections::HashSet, path::PathBuf, rc::Rc};

use bolt_v2::{
    clients::{chainlink, polymarket},
    config::{Config, ReferenceVenueKind, ensure_runtime_has_active_path},
    normalized_sink,
    platform::runtime::{
        build_reference_data_client, reference_client_name_for_kind,
        registry_runtime_strategy_factory, wire_platform_runtime,
    },
    secrets, startup_validation,
    strategies::{production_strategy_registry, registry::StrategyBuildContext},
};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::LiveNode;
use nautilus_model::identifiers::TraderId;

type AppResult = Result<(), Box<dyn std::error::Error>>;
type AppFuture = std::pin::Pin<Box<dyn std::future::Future<Output = AppResult>>>;

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
            let cfg = Config::load(&config)?;
            ensure_runtime_has_active_path(&cfg)?;

            let trader_id = TraderId::from(cfg.node.trader_id.as_str());
            let environment = parse_environment(&cfg.node.environment)?;
            let log_config = LoggerConfig {
                stdout_level: parse_log_level(&cfg.logging.stdout_level)?,
                fileout_level: parse_log_level(&cfg.logging.file_level)?,
                ..Default::default()
            };
            let polymarket_ruleset_setup = polymarket::PolymarketRulesetSetup::from_rulesets(
                &cfg.rulesets,
                cfg.node.timeout_connection_secs,
            )?;
            startup_validation::validate_polymarket_startup_with_prefix_event_slugs(
                &cfg,
                polymarket_ruleset_setup.resolved_prefix_event_slugs(),
            )?;
            let mut polymarket_selector_refresh_raw = None;
            let mut builder = LiveNode::builder(trader_id, environment)?
                .with_name(cfg.node.name.clone())
                .with_logging(log_config)
                .with_load_state(cfg.node.load_state)
                .with_save_state(cfg.node.save_state)
                .with_timeout_connection(cfg.node.timeout_connection_secs)
                .with_timeout_reconciliation(cfg.node.timeout_reconciliation_secs)
                .with_timeout_portfolio(cfg.node.timeout_portfolio_secs)
                .with_timeout_disconnection_secs(cfg.node.timeout_disconnection_secs)
                .with_delay_post_stop_secs(cfg.node.delay_post_stop_secs)
                .with_delay_shutdown_secs(cfg.node.delay_shutdown_secs);

            for client in &cfg.data_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        polymarket_selector_refresh_raw = Some(client.config.clone());
                        let (factory, config) =
                            polymarket_ruleset_setup.build_data_client(&client.config)?;
                        builder =
                            builder.add_data_client(Some(client.name.clone()), factory, config)?;
                    }
                    other => return Err(format!("Unsupported data client type: {other}").into()),
                }
            }

            if !cfg.rulesets.is_empty() {
                let mut registered_reference_kinds = HashSet::new();
                for venue in &cfg.reference.venues {
                    if !registered_reference_kinds.insert(reference_kind_key(&venue.kind)) {
                        continue;
                    }

                    match venue.kind {
                        ReferenceVenueKind::Polymarket => {}
                        ReferenceVenueKind::Chainlink => {
                            let (factory, config) =
                                chainlink::build_chainlink_reference_data_client(&cfg.reference)?;
                            builder = builder.add_data_client(
                                Some(reference_client_name_for_kind(&cfg, &venue.kind)?),
                                factory,
                                config,
                            )?;
                        }
                        _ => {
                            let (factory, config) = build_reference_data_client(venue)?;
                            builder = builder.add_data_client(
                                Some(reference_client_name_for_kind(&cfg, &venue.kind)?),
                                factory,
                                config,
                            )?;
                        }
                    }
                }
            }

            let mut strategy_build_context = None;
            for client in &cfg.exec_clients {
                match client.kind.as_str() {
                    "polymarket" => {
                        let resolved = secrets::resolve_polymarket(&client.secrets)?;
                        if strategy_build_context.is_none() {
                            strategy_build_context = Some(StrategyBuildContext {
                                fee_provider: polymarket::build_fee_provider(
                                    &client.config,
                                    &resolved,
                                    cfg.node.timeout_connection_secs,
                                )?,
                            });
                        }
                        let (factory, config) =
                            polymarket::build_exec_client(&client.config, trader_id, resolved)?;
                        builder =
                            builder.add_exec_client(Some(client.name.clone()), factory, config)?;
                    }
                    other => return Err(format!("Unsupported exec client type: {other}").into()),
                }
            }
            let strategy_registry = production_strategy_registry()?;
            let strategy_build_context = if cfg.strategies.is_empty() && cfg.rulesets.is_empty() {
                None
            } else {
                Some(strategy_build_context.ok_or_else(|| {
                    std::io::Error::other(
                        "missing Polymarket exec client for strategy fee-provider context",
                    )
                })?)
            };

            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            let local = tokio::task::LocalSet::new();
            let app: AppFuture = Box::pin(async move {
                let mut node = builder.build()?;
                let node_handle = node.handle();
                let polymarket_selector_refresh_guard = polymarket_selector_refresh_raw
                    .as_ref()
                    .map(|raw| {
                        polymarket_ruleset_setup.spawn_selector_refresh_task_if_configured(
                            raw,
                            cfg.node.timeout_connection_secs,
                        )
                    })
                    .transpose()?
                    .flatten();
                let normalized_sink_guards = if cfg.streaming.catalog_path.trim().is_empty() {
                    None
                } else {
                    Some(normalized_sink::wire_normalized_sinks(
                        &node,
                        node_handle,
                        &cfg.streaming.catalog_path,
                        cfg.streaming.flush_interval_ms,
                        cfg.streaming.contract_path.as_deref(),
                    )?)
                };
                let mut normalized_sink_guards = normalized_sink_guards;
                let mut sink_failure_receiver = normalized_sink_guards
                    .as_mut()
                    .and_then(|guards| guards.take_failure_receiver());

                if cfg.rulesets.is_empty() {
                    let strategy_build_context = strategy_build_context.as_ref().expect(
                        "strategy build context should exist when strategies are configured",
                    );
                    let trader = Rc::clone(node.kernel().trader());
                    for strategy in &cfg.strategies {
                        strategy_registry
                            .register_strategy(
                                &strategy.kind,
                                &strategy.config,
                                strategy_build_context,
                                &trader,
                            )
                            .map_err(|error| {
                                Box::new(std::io::Error::other(format!(
                                    "failed registering strategy kind {}: {error}",
                                    strategy.kind
                                ))) as Box<dyn std::error::Error>
                            })?;
                    }
                }
                let platform_runtime_guards = if cfg.rulesets.is_empty() {
                    None
                } else {
                    let runtime_strategy_factory = registry_runtime_strategy_factory(
                        strategy_registry.clone(),
                        strategy_build_context
                            .as_ref()
                            .expect("runtime strategy context should exist in ruleset mode")
                            .clone(),
                    );
                    Some(wire_platform_runtime(
                        &mut node,
                        &cfg,
                        runtime_strategy_factory,
                        polymarket_ruleset_setup.selector_state(),
                    )?)
                };

                let run_result = {
                    let run_future = node.run();
                    tokio::pin!(run_future);

                    if let Some(receiver) = sink_failure_receiver.as_mut() {
                        tokio::select! {
                            result = &mut run_future => result,
                            _ = receiver => {
                                log::error!("Normalized sink failure detected, awaiting LiveNode shutdown");
                                run_future.await
                            }
                        }
                    } else {
                        run_future.await
                    }
                };
                if let Some(guard) = polymarket_selector_refresh_guard {
                    guard.shutdown().await;
                }
                let platform_shutdown_result = if let Some(guards) = platform_runtime_guards {
                    guards.shutdown().await.map_err(|e| {
                        Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error>
                    })
                } else {
                    Ok(())
                };
                let shutdown_result = if let Some(guards) = normalized_sink_guards {
                    guards.shutdown().await.map_err(|e| {
                        Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error>
                    })
                } else {
                    Ok(())
                };

                match (run_result, platform_shutdown_result, shutdown_result) {
                    (Ok(()), Ok(()), Ok(())) => Ok(()),
                    (Err(run_error), Ok(()), Ok(())) => Err(run_error.into()),
                    (Ok(()), Err(platform_error), Ok(())) => Err(platform_error),
                    (Ok(()), Ok(()), Err(shutdown_error)) => Err(shutdown_error),
                    (Err(run_error), Err(platform_error), Ok(())) => {
                        log::error!("Live node run error during platform shutdown: {run_error}");
                        Err(Box::new(std::io::Error::other(format!(
                            "platform shutdown error: {platform_error}; node run error: {run_error}"
                        ))) as Box<dyn std::error::Error>)
                    }
                    (Err(run_error), Ok(()), Err(shutdown_error)) => {
                        log::error!("Live node run error during sink shutdown: {run_error}");
                        Err(Box::new(std::io::Error::other(format!(
                            "normalized sink shutdown error: {shutdown_error}; node run error: {run_error}"
                        ))) as Box<dyn std::error::Error>)
                    }
                    (Ok(()), Err(platform_error), Err(shutdown_error)) => {
                        log::error!(
                            "Normalized sink secondary error during platform shutdown: {shutdown_error}"
                        );
                        Err(platform_error)
                    }
                    (Err(run_error), Err(platform_error), Err(shutdown_error)) => {
                        log::error!(
                            "Normalized sink secondary error during platform shutdown: {shutdown_error}"
                        );
                        log::error!("Live node run error during platform shutdown: {run_error}");
                        Err(Box::new(std::io::Error::other(format!(
                            "platform shutdown error: {platform_error}; node run error: {run_error}"
                        ))) as Box<dyn std::error::Error>)
                    }
                }
            });

            Ok(runtime.block_on(local.run_until(app))?)
        }
    }
}

fn run_secrets_command(command: SecretsCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        SecretsCommand::Check { config } => {
            let cfg = Config::load(&config)?;
            ensure_runtime_has_active_path(&cfg)?;
            let mut has_errors = false;

            if let Some(chainlink) = cfg.reference.chainlink.as_ref() {
                let check = secrets::check_chainlink_secret_config(chainlink);
                if check.is_complete() {
                    println!(
                        "reference.chainlink: secret config complete ({})",
                        check.present.join(", ")
                    );
                } else {
                    has_errors = true;
                    eprintln!(
                        "reference.chainlink: missing secret config fields ({})",
                        check.missing.join(", ")
                    );
                }
            }

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
                Err("One or more runtime secret configurations are incomplete".into())
            } else {
                Ok(())
            }
        }
        SecretsCommand::Resolve { config } => {
            let cfg = Config::load(&config)?;
            ensure_runtime_has_active_path(&cfg)?;

            if let Some(chainlink) = cfg.reference.chainlink.as_ref() {
                secrets::resolve_chainlink(
                    &chainlink.region,
                    &chainlink.api_key,
                    &chainlink.api_secret,
                )?;
                println!("reference.chainlink: secrets resolved successfully");
            }

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

fn reference_kind_key(kind: &ReferenceVenueKind) -> &'static str {
    match kind {
        ReferenceVenueKind::Binance => "binance",
        ReferenceVenueKind::Bybit => "bybit",
        ReferenceVenueKind::Deribit => "deribit",
        ReferenceVenueKind::Hyperliquid => "hyperliquid",
        ReferenceVenueKind::Kraken => "kraken",
        ReferenceVenueKind::Okx => "okx",
        ReferenceVenueKind::Polymarket => "polymarket",
        ReferenceVenueKind::Chainlink => "chainlink",
    }
}
