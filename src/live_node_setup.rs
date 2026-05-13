use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;

use anyhow::anyhow;
use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};
use nautilus_common::{enums::Environment, logging::logger::LoggerConfig};
use nautilus_data::DataClientAdapter;
use nautilus_execution::engine::ExecutionEngine;
use nautilus_live::{config::LiveNodeConfig, node::LiveNode};
use nautilus_model::identifiers::TraderId;

use crate::{
    bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter, clients::polymarket, config::Config,
    strategies::registry::StrategyBuildContext,
};

pub type DataClientRegistration = (
    Option<String>,
    Box<dyn DataClientFactory>,
    Box<dyn ClientConfig>,
);
pub type ExecClientRegistration = (
    Option<String>,
    Box<dyn ExecutionClientFactory>,
    Box<dyn ClientConfig>,
);

fn resolve_client_name(name: &Option<String>, factory_name: &str) -> String {
    name.clone().unwrap_or_else(|| factory_name.to_string())
}

pub fn make_strategy_build_context(
    fee_provider: Arc<dyn polymarket::FeeProvider>,
    reference_publish_topic: String,
    decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter>,
) -> StrategyBuildContext {
    StrategyBuildContext::try_new(
        fee_provider,
        reference_publish_topic,
        Some(decision_evidence),
    )
    .expect("strategy build context requires decision evidence")
}

pub fn make_live_node_config(
    cfg: &Config,
    trader_id: TraderId,
    environment: Environment,
    log_config: LoggerConfig,
) -> LiveNodeConfig {
    LiveNodeConfig {
        environment,
        trader_id,
        load_state: cfg.node.load_state,
        save_state: cfg.node.save_state,
        logging: log_config,
        timeout_connection: std::time::Duration::from_secs(cfg.node.timeout_connection_secs),
        timeout_reconciliation: std::time::Duration::from_secs(
            cfg.node.timeout_reconciliation_secs,
        ),
        timeout_portfolio: std::time::Duration::from_secs(cfg.node.timeout_portfolio_secs),
        timeout_disconnection: std::time::Duration::from_secs(cfg.node.timeout_disconnection_secs),
        delay_post_stop: std::time::Duration::from_secs(cfg.node.delay_post_stop_secs),
        timeout_shutdown: std::time::Duration::from_secs(cfg.node.delay_shutdown_secs),
        exec_engine: nautilus_live::config::LiveExecEngineConfig {
            position_check_interval_secs: cfg.exec_engine.position_check_interval_secs,
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn register_data_client(
    node: &mut LiveNode,
    name: Option<String>,
    factory: Box<dyn DataClientFactory>,
    config: Box<dyn ClientConfig>,
) -> std::result::Result<(), Box<dyn Error>> {
    let name = resolve_client_name(&name, factory.name());
    let kernel = node.kernel();
    let client = factory.create(
        &name,
        config.as_ref(),
        kernel.cache.clone(),
        kernel.clock.clone(),
    )?;
    let client_id = client.client_id();
    let venue = client.venue();
    let adapter = DataClientAdapter::new(client_id, venue, true, true, client);

    node.kernel_mut()
        .data_engine
        .borrow_mut()
        .register_client(adapter, venue);

    log::info!("Registered DataClient-{client_id}");
    Ok(())
}

pub fn register_exec_client(
    node: &mut LiveNode,
    name: Option<String>,
    factory: Box<dyn ExecutionClientFactory>,
    config: Box<dyn ClientConfig>,
) -> std::result::Result<(), Box<dyn Error>> {
    let name = resolve_client_name(&name, factory.name());
    let cache = node.kernel().cache.clone();
    let client = factory.create(&name, config.as_ref(), cache)?;
    let client_id = client.client_id();
    let venue = client.venue();

    node.kernel_mut()
        .exec_engine
        .borrow_mut()
        .register_client(client)?;
    ExecutionEngine::subscribe_venue_instruments(&node.kernel().exec_engine, venue);

    log::info!("Registered ExecutionClient-{client_id}");
    Ok(())
}

pub fn build_live_node(
    name: String,
    config: LiveNodeConfig,
    data_clients: Vec<DataClientRegistration>,
    exec_clients: Vec<ExecClientRegistration>,
) -> std::result::Result<LiveNode, Box<dyn Error>> {
    match config.environment {
        Environment::Live => {
            let mut node = LiveNode::build(name, Some(config))?;
            let mut seen_data_client_names = HashSet::new();
            for (client_name, factory, client_config) in data_clients {
                let resolved_name = resolve_client_name(&client_name, factory.name());
                if !seen_data_client_names.insert(resolved_name.clone()) {
                    return Err(
                        anyhow!("Data client '{resolved_name}' is already registered").into(),
                    );
                }
                register_data_client(&mut node, client_name, factory, client_config)?;
            }
            let mut seen_exec_client_names = HashSet::new();
            for (client_name, factory, client_config) in exec_clients {
                let resolved_name = resolve_client_name(&client_name, factory.name());
                if !seen_exec_client_names.insert(resolved_name.clone()) {
                    return Err(anyhow!(
                        "Execution client '{resolved_name}' is already registered"
                    )
                    .into());
                }
                register_exec_client(&mut node, client_name, factory, client_config)?;
            }
            Ok(node)
        }
        Environment::Sandbox => {
            if config.exec_engine.position_check_interval_secs.is_some() {
                return Err(anyhow!(
                    "exec_engine.position_check_interval_secs is unsupported in Sandbox startup mode"
                )
                .into());
            }

            let mut builder = LiveNode::builder(config.trader_id, Environment::Sandbox)?
                .with_name(name)
                .with_logging(config.logging.clone())
                .with_load_state(config.load_state)
                .with_save_state(config.save_state)
                .with_timeout_connection(config.timeout_connection.as_secs())
                .with_timeout_reconciliation(config.timeout_reconciliation.as_secs())
                .with_reconciliation(config.exec_engine.reconciliation)
                .with_timeout_portfolio(config.timeout_portfolio.as_secs())
                .with_timeout_disconnection_secs(config.timeout_disconnection.as_secs())
                .with_delay_post_stop_secs(config.delay_post_stop.as_secs())
                .with_delay_shutdown_secs(config.timeout_shutdown.as_secs());

            if let Some(lookback_mins) = config.exec_engine.reconciliation_lookback_mins {
                builder = builder.with_reconciliation_lookback_mins(lookback_mins);
            }

            for (client_name, factory, client_config) in data_clients {
                builder = builder.add_data_client(client_name, factory, client_config)?;
            }
            for (client_name, factory, client_config) in exec_clients {
                builder = builder.add_exec_client(client_name, factory, client_config)?;
            }

            Ok(builder.build()?)
        }
        Environment::Backtest => {
            Err(anyhow!("LiveNode startup path does not support Backtest").into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use futures_util::future::{BoxFuture, FutureExt};

    #[derive(Debug, Clone)]
    struct FixedFeeProvider;

    impl polymarket::FeeProvider for FixedFeeProvider {
        fn fee_bps(&self, _token_id: &str) -> Option<rust_decimal::Decimal> {
            Some(rust_decimal::Decimal::new(7, 0))
        }

        fn warm(&self, _token_id: &str) -> BoxFuture<'_, Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    #[derive(Debug)]
    struct FixedDecisionEvidenceWriter;

    impl BoltV3DecisionEvidenceWriter for FixedDecisionEvidenceWriter {
        fn record_order_intent(
            &self,
            _intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn strategy_build_context_includes_reference_publish_topic() {
        let context = make_strategy_build_context(
            Arc::new(FixedFeeProvider),
            "platform.reference.test".to_string(),
            Arc::new(FixedDecisionEvidenceWriter),
        );

        assert_eq!(context.reference_publish_topic(), "platform.reference.test");
        assert_eq!(
            context.fee_provider().fee_bps("TOKEN"),
            Some(rust_decimal::Decimal::new(7, 0))
        );
    }

    #[test]
    fn live_node_config_defaults_position_check_interval_to_none() {
        let cfg: Config = toml::from_str(
            r#"
            [node]
            name = "bolt-v2"
            trader_id = "TRADER-001"
            environment = "Live"
            load_state = false
            save_state = false
            timeout_connection_secs = 60
            timeout_reconciliation_secs = 30
            timeout_portfolio_secs = 10
            timeout_disconnection_secs = 10
            delay_post_stop_secs = 5
            delay_shutdown_secs = 5

            [logging]
            stdout_level = "Info"
            file_level = "Debug"

            [[data_clients]]
            name = "POLYMARKET"
            type = "polymarket"
            [data_clients.config]
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            ws_max_subscriptions = 200
            event_slugs = ["btc-updown-5m"]

            [[exec_clients]]
            name = "POLYMARKET"
            type = "polymarket"
            [exec_clients.config]
            account_id = "POLYMARKET-001"
            signature_type = 2
            funder = "0xabc"
            [exec_clients.secrets]
            region = "us-east-1"
            pk = "/pk"
            api_key = "/key"
            api_secret = "/secret"
            passphrase = "/pass"
            "#,
        )
        .expect("config should parse");

        let node_config = make_live_node_config(
            &cfg,
            TraderId::from("TRADER-001"),
            Environment::Live,
            LoggerConfig::default(),
        );

        assert_eq!(node_config.exec_engine.position_check_interval_secs, None);
    }

    #[test]
    fn live_node_config_passes_through_position_check_interval() {
        let cfg: Config = toml::from_str(
            r#"
            [node]
            name = "bolt-v2"
            trader_id = "TRADER-001"
            environment = "Live"
            load_state = false
            save_state = false
            timeout_connection_secs = 60
            timeout_reconciliation_secs = 30
            timeout_portfolio_secs = 10
            timeout_disconnection_secs = 10
            delay_post_stop_secs = 5
            delay_shutdown_secs = 5

            [logging]
            stdout_level = "Info"
            file_level = "Debug"

            [exec_engine]
            position_check_interval_secs = 11

            [[data_clients]]
            name = "POLYMARKET"
            type = "polymarket"
            [data_clients.config]
            subscribe_new_markets = false
            update_instruments_interval_mins = 60
            ws_max_subscriptions = 200
            event_slugs = ["btc-updown-5m"]

            [[exec_clients]]
            name = "POLYMARKET"
            type = "polymarket"
            [exec_clients.config]
            account_id = "POLYMARKET-001"
            signature_type = 2
            funder = "0xabc"
            [exec_clients.secrets]
            region = "us-east-1"
            pk = "/pk"
            api_key = "/key"
            api_secret = "/secret"
            passphrase = "/pass"
            "#,
        )
        .expect("config should parse");

        let node_config = make_live_node_config(
            &cfg,
            TraderId::from("TRADER-001"),
            Environment::Live,
            LoggerConfig::default(),
        );

        assert_eq!(
            node_config.exec_engine.position_check_interval_secs,
            Some(11.0)
        );
    }
}
