//! Client registration boundary for Bolt-v3.
//!
//! Translates a [`BoltV3ClientConfigs`] value into NT-native
//! `add_data_client` / `add_exec_client` calls on a
//! [`nautilus_live::builder::LiveNodeBuilder`] for every configured
//! `[clients.<id>]` block. The bolt-v3 client identifier is reused as the
//! NT registration name so per-client routing stays addressable.
//!
//! This module accumulates registration intent on the builder. Bolt-v3
//! itself never opens a network connection, never runs the event loop,
//! never calls a user-level `subscribe_*` API, never selects a market,
//! never constructs an order, and never submits an order from this
//! boundary or its callers in the slice-7 path.
//!
//! The actual NT-side build behaviour lives inside
//! `LiveNodeBuilder::build` and is **not** purely passive: NT
//! constructs the client objects (Polymarket data, Polymarket
//! execution, Binance data) from the bolt-v3-supplied configs, parses
//! the Polymarket private key into an NT secp256k1 signer (deriving
//! the EVM address), and performs internal NT engine/message-bus
//! subscriptions for client instrument topics. None of that opens an
//! external network connection or starts the live event loop, but it
//! is more than no-op factory storage and the boundary documentation
//! must reflect that.

use std::collections::BTreeMap;

use nautilus_live::builder::LiveNodeBuilder;

use crate::bolt_v3_adapters::BoltV3ClientConfigs;

/// Inspectable record of which NT client kinds the bolt-v3 boundary
/// added to the [`LiveNodeBuilder`] for one configured client. A `false`
/// flag means the corresponding `[clients.<id>.<block>]` was absent in
/// the validated config so no `add_*_client` call was made for that
/// kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoltV3RegisteredClient {
    pub data: bool,
    pub execution: bool,
}

/// Per-client summary of which NT factory kinds were added to the
/// [`LiveNodeBuilder`] during the bolt-v3 client-registration smoke.
/// Keyed by the bolt-v3 client identifier (the TOML `[clients.<id>]`
/// table key, which the boundary also uses as the NT registration
/// name). The summary is the only inspectable surface this module
/// exposes; the builder itself owns the actual factory and config
/// instances.
#[derive(Clone, Debug, Default)]
pub struct BoltV3RegistrationSummary {
    pub clients: BTreeMap<String, BoltV3RegisteredClient>,
}

#[derive(Debug)]
pub enum BoltV3ClientRegistrationError {
    /// `LiveNodeBuilder::add_data_client` rejected the data factory for
    /// a client (e.g. duplicate registration name). The wrapped string
    /// is the underlying NT error message.
    AddDataClient {
        client_id_key: String,
        message: String,
    },
    /// `LiveNodeBuilder::add_exec_client` rejected the execution
    /// factory for a client (e.g. duplicate registration name).
    AddExecClient {
        client_id_key: String,
        message: String,
    },
}

impl std::fmt::Display for BoltV3ClientRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddDataClient {
                client_id_key,
                message,
            } => write!(
                f,
                "clients.{client_id_key}: NT LiveNodeBuilder rejected data client: {message}"
            ),
            Self::AddExecClient {
                client_id_key,
                message,
            } => write!(
                f,
                "clients.{client_id_key}: NT LiveNodeBuilder rejected execution client: {message}"
            ),
        }
    }
}

impl std::error::Error for BoltV3ClientRegistrationError {}

/// Adds an NT data and/or execution client factory to `builder` for
/// every configured `[clients.<id>]` block in `adapters`, using the
/// bolt-v3 client identifier as the NT registration name. Returns the
/// updated builder paired with an inspectable summary of which client
/// kinds were registered per client.
///
/// This function does not call `connect`, `disconnect`, `run`, any
/// `subscribe_*` API, market selection, order construction, or any
/// submit path. Network I/O is gated by `LiveNodeBuilder::build`,
/// owned by NT.
pub fn register_bolt_v3_clients(
    mut builder: LiveNodeBuilder,
    adapters: BoltV3ClientConfigs,
) -> Result<(LiveNodeBuilder, BoltV3RegistrationSummary), BoltV3ClientRegistrationError> {
    let mut clients = BTreeMap::new();
    for (client_id_key, client_id) in adapters.clients {
        let mut data_added = false;
        let mut exec_added = false;
        if let Some(data) = client_id.data {
            builder = builder
                .add_data_client(Some(client_id_key.clone()), data.factory, data.config)
                .map_err(|error| BoltV3ClientRegistrationError::AddDataClient {
                    client_id_key: client_id_key.clone(),
                    message: error.to_string(),
                })?;
            data_added = true;
        }
        if let Some(execution) = client_id.execution {
            builder = builder
                .add_exec_client(
                    Some(client_id_key.clone()),
                    execution.factory,
                    execution.config,
                )
                .map_err(|error| BoltV3ClientRegistrationError::AddExecClient {
                    client_id_key: client_id_key.clone(),
                    message: error.to_string(),
                })?;
            exec_added = true;
        }
        let registered = BoltV3RegisteredClient {
            data: data_added,
            execution: exec_added,
        };
        clients.insert(client_id_key.clone(), registered);
    }
    Ok((builder, BoltV3RegistrationSummary { clients }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{path::PathBuf, sync::Arc};

    use nautilus_common::enums::Environment;
    use nautilus_live::node::LiveNode;
    use nautilus_model::identifiers::TraderId;
    use nautilus_polymarket::{
        config::PolymarketDataClientConfig, factories::PolymarketDataClientFactory,
    };

    use crate::{
        bolt_v3_adapters::{
            BoltV3ClientConfig, BoltV3DataClientAdapterConfig, map_bolt_v3_clients,
        },
        bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config},
        bolt_v3_providers::{
            binance::ResolvedBoltV3BinanceSecrets, polymarket::ResolvedBoltV3PolymarketSecrets,
        },
        bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
    };

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root_text = include_str!("../tests/fixtures/bolt_v3/root.toml");
        let root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
        LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root,
            strategies: Vec::new(),
        }
    }

    fn fixture_polymarket_secrets() -> ResolvedBoltV3PolymarketSecrets {
        ResolvedBoltV3PolymarketSecrets {
            // 32-byte secp256k1 hex; the unit tests in this module never
            // reach NT factory.create, but downstream integration tests
            // use the same shape.
            private_key: "0x4242424242424242424242424242424242424242424242424242424242424242"
                .to_string(),
            api_key: "fixture-poly-api-key".to_string(),
            api_secret: "YWJj".to_string(),
            passphrase: "fixture-poly-passphrase".to_string(),
        }
    }

    fn fixture_binance_secrets() -> ResolvedBoltV3BinanceSecrets {
        ResolvedBoltV3BinanceSecrets {
            api_key: "fixture-binance-api-key".to_string(),
            api_secret: "fixture-binance-api-secret".to_string(),
        }
    }

    fn fixture_resolved_secrets() -> ResolvedBoltV3Secrets {
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_polymarket_secrets()),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        ResolvedBoltV3Secrets { clients }
    }

    fn fixture_adapters() -> BoltV3ClientConfigs {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        map_bolt_v3_clients(&loaded, &resolved).expect("adapters should map")
    }

    fn fresh_builder() -> LiveNodeBuilder {
        LiveNode::builder(TraderId::from("BOLT-001"), Environment::Live)
            .expect("Live builder should construct for unit-test fixture")
    }

    #[test]
    fn fixture_clients_register_one_data_and_one_exec_for_polymarket_and_one_data_for_binance() {
        let adapters = fixture_adapters();

        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), adapters)
            .expect("registration should succeed");

        assert_eq!(summary.clients.len(), 2);
        let polymarket = summary
            .clients
            .get("polymarket_main")
            .expect("polymarket_main must appear in summary");
        assert!(
            polymarket.data,
            "polymarket_main has a [data] block in the fixture"
        );
        assert!(
            polymarket.execution,
            "polymarket_main has an [execution] block in the fixture"
        );
        let binance = summary
            .clients
            .get("binance_reference")
            .expect("binance_reference must appear in summary");
        assert!(
            binance.data,
            "binance_reference has a [data] block in the fixture"
        );
        assert!(
            !binance.execution,
            "binance_reference has no [execution] block in the fixture"
        );
    }

    #[test]
    fn empty_adapters_produce_empty_summary_and_pristine_builder_state() {
        let adapters = BoltV3ClientConfigs {
            clients: BTreeMap::new(),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), adapters)
            .expect("empty adapters should register cleanly");
        assert!(summary.clients.is_empty());
    }

    #[test]
    fn polymarket_client_id_with_only_data_block_does_not_register_an_exec_client() {
        let adapters = BoltV3ClientConfigs {
            clients: BTreeMap::from([(
                "polymarket_data_only".to_string(),
                BoltV3ClientConfig {
                    data: Some(BoltV3DataClientAdapterConfig {
                        factory: Box::new(PolymarketDataClientFactory),
                        config: Box::new(PolymarketDataClientConfig {
                            base_url_http: Some("https://clob.polymarket.com".to_string()),
                            base_url_ws: Some(
                                "wss://ws-subscriptions-clob.polymarket.com/ws/market".to_string(),
                            ),
                            base_url_gamma: Some("https://gamma-api.polymarket.com".to_string()),
                            base_url_data_api: Some("https://data-api.polymarket.com".to_string()),
                            http_timeout_secs: 60,
                            ws_timeout_secs: 30,
                            ws_max_subscriptions: 200,
                            update_instruments_interval_mins: 60,
                            subscribe_new_markets: false,
                            auto_load_missing_instruments: false,
                            auto_load_debounce_ms: 100,
                            transport_backend: Default::default(),
                            filters: Vec::new(),
                            new_market_filter: None,
                        }),
                    }),
                    execution: None,
                },
            )]),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), adapters)
            .expect("data-only registration should succeed");
        let registered = summary
            .clients
            .get("polymarket_data_only")
            .expect("data-only client must appear in summary");
        assert!(registered.data);
        assert!(
            !registered.execution,
            "no [execution] block, so no exec registration"
        );
    }

    #[test]
    fn binance_client_id_with_no_data_block_records_data_false_in_summary() {
        let adapters = BoltV3ClientConfigs {
            clients: BTreeMap::from([(
                "binance_no_data".to_string(),
                BoltV3ClientConfig {
                    data: None,
                    execution: None,
                },
            )]),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), adapters)
            .expect("missing data block should register cleanly");
        let registered = summary
            .clients
            .get("binance_no_data")
            .expect("binance client must appear in summary");
        assert!(!registered.data, "no [data] block, so no data registration");
        assert!(!registered.execution);
    }

    #[test]
    fn duplicate_data_client_name_returns_data_registration_error() {
        let mut existing_adapters = fixture_adapters();
        let data = existing_adapters
            .clients
            .remove("polymarket_main")
            .expect("fixture polymarket_main should map")
            .data
            .expect("fixture polymarket_main should have data adapter");
        let builder = fresh_builder()
            .add_data_client(
                Some("polymarket_main".to_string()),
                data.factory,
                data.config,
            )
            .expect("pre-registering fixture data client should succeed");

        let error = register_bolt_v3_clients(builder, fixture_adapters())
            .expect_err("duplicate data client name should fail registration");

        match error {
            BoltV3ClientRegistrationError::AddDataClient {
                client_id_key,
                message,
            } => {
                assert_eq!(client_id_key, "polymarket_main");
                assert!(
                    message.contains("already registered"),
                    "underlying NT error should explain duplicate registration: {message}"
                );
            }
            other => panic!("expected AddDataClient error, got {other:?}"),
        }
    }

    #[test]
    fn duplicate_exec_client_name_returns_exec_registration_error() {
        let mut existing_adapters = fixture_adapters();
        let execution = existing_adapters
            .clients
            .remove("polymarket_main")
            .expect("fixture polymarket_main should map")
            .execution
            .expect("fixture polymarket_main should have execution adapter");
        let builder = fresh_builder()
            .add_exec_client(
                Some("polymarket_main".to_string()),
                execution.factory,
                execution.config,
            )
            .expect("pre-registering fixture execution client should succeed");

        let error = register_bolt_v3_clients(builder, fixture_adapters())
            .expect_err("duplicate execution client name should fail registration");

        match error {
            BoltV3ClientRegistrationError::AddExecClient {
                client_id_key,
                message,
            } => {
                assert_eq!(client_id_key, "polymarket_main");
                assert!(
                    message.contains("already registered"),
                    "underlying NT error should explain duplicate registration: {message}"
                );
            }
            other => panic!("expected AddExecClient error, got {other:?}"),
        }
    }
}
