mod config;

use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use log::LevelFilter;
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_live::node::LiveNode;
use nautilus_model::{
    identifiers::{AccountId, ClientId, InstrumentId, StrategyId, TraderId},
    types::Quantity,
};
use nautilus_polymarket::{
    common::enums::SignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::EventSlugFilter,
};
use nautilus_testkit::testers::{ExecTester, ExecTesterConfig};

use crate::config::Config;

#[derive(Parser)]
#[command(name = "bolt-v2")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Run the trading node
    Run {
        /// Path to TOML config file
        #[arg(short, long)]
        config: PathBuf,
    },
    /// Resolve secrets from the config and output as env vars
    Secrets {
        /// Path to TOML config file
        #[arg(short, long)]
        config: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Command::Secrets { config } => {
            let cfg = Config::load(&config)?;
            cfg.wallet.print_env()?;
            Ok(())
        }
        Command::Run { config } => {
            let cfg = Config::load(&config)?;
            // Env vars set here, before tokio spawns worker threads.
            cfg.wallet.inject()?;

            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?
                .block_on(run(cfg))
        }
    }
}

async fn run(cfg: Config) -> Result<(), Box<dyn std::error::Error>> {
    let trader_id = TraderId::from(cfg.node.trader_id.as_str());
    let account_id = AccountId::from(cfg.node.account_id.as_str());
    let strategy_id = StrategyId::from(cfg.strategy.strategy_id.as_str());
    let client_id = ClientId::new(cfg.node.client_id.clone());
    let instrument_id = InstrumentId::from(cfg.venue.instrument_id.as_str());

    let signature_type = match cfg.wallet.signature_type_id {
        0 => SignatureType::Eoa,
        1 => SignatureType::PolyProxy,
        2 => SignatureType::PolyGnosisSafe,
        other => return Err(format!("Unknown signature_type_id: {other}. Expected 0 (EOA), 1 (PolyProxy), or 2 (PolyGnosisSafe)").into()),
    };

    let environment = match cfg.node.environment.as_str() {
        "Live" => Environment::Live,
        "Sandbox" => Environment::Sandbox,
        other => return Err(format!("Unknown environment: {other}. Expected Live or Sandbox").into()),
    };

    let data_filter = EventSlugFilter::from_slugs(vec![cfg.venue.event_slug]);

    let data_config = PolymarketDataClientConfig {
        filters: vec![Arc::new(data_filter)],
        ..Default::default()
    };

    let exec_config = PolymarketExecClientConfig {
        trader_id,
        account_id,
        signature_type,
        ..Default::default()
    };

    let log_config = LoggerConfig {
        stdout_level: parse_log_level(&cfg.logging.stdout_level)?,
        fileout_level: parse_log_level(&cfg.logging.file_level)?,
        ..Default::default()
    };

    let mut node = LiveNode::builder(trader_id, environment)?
        .with_name(cfg.node.name)
        .with_logging(log_config)
        .with_load_state(cfg.node.load_state)
        .with_save_state(cfg.node.save_state)
        .with_timeout_connection(cfg.timeouts.connection_secs)
        .with_timeout_reconciliation(cfg.timeouts.reconciliation_secs)
        .with_timeout_portfolio(cfg.timeouts.portfolio_secs)
        .with_timeout_disconnection_secs(cfg.timeouts.disconnection_secs)
        .with_delay_post_stop_secs(cfg.timeouts.post_stop_delay_secs)
        .with_delay_shutdown_secs(cfg.timeouts.shutdown_delay_secs)
        .with_reconciliation(cfg.venue.reconciliation_enabled)
        .with_reconciliation_lookback_mins(cfg.venue.reconciliation_lookback_mins)
        .add_data_client(None, Box::new(PolymarketDataClientFactory), Box::new(data_config))?
        .add_exec_client(None, Box::new(PolymarketExecutionClientFactory), Box::new(exec_config))?
        .build()?;

    let tester_config = ExecTesterConfig::builder()
        .base(nautilus_trading::strategy::StrategyConfig {
            strategy_id: Some(strategy_id),
            external_order_claims: Some(vec![instrument_id]),
            ..Default::default()
        })
        .instrument_id(instrument_id)
        .client_id(client_id)
        .order_qty(Quantity::from(cfg.strategy.order_qty.as_str()))
        .log_data(cfg.strategy.log_data)
        .use_post_only(cfg.strategy.use_post_only)
        .tob_offset_ticks(cfg.strategy.tob_offset_ticks)
        .enable_limit_sells(cfg.strategy.enable_limit_sells)
        .enable_stop_buys(cfg.strategy.enable_stop_buys)
        .enable_stop_sells(cfg.strategy.enable_stop_sells)
        .build();

    node.add_strategy(ExecTester::new(tester_config))?;
    node.run().await?;

    Ok(())
}

fn parse_log_level(s: &str) -> Result<LevelFilter, Box<dyn std::error::Error>> {
    match s {
        "Trace" => Ok(LevelFilter::Trace),
        "Debug" => Ok(LevelFilter::Debug),
        "Info" => Ok(LevelFilter::Info),
        "Warn" => Ok(LevelFilter::Warn),
        "Error" => Ok(LevelFilter::Error),
        "Off" => Ok(LevelFilter::Off),
        other => Err(format!("Unknown log level: {other}. Expected Trace, Debug, Info, Warn, Error, or Off").into()),
    }
}
