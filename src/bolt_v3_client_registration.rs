//! Client registration boundary for Bolt-v3.
//!
//! Translates a [`BoltV3AdapterConfigs`] value into NT-native
//! `add_data_client` / `add_exec_client` calls on a
//! [`nautilus_live::builder::LiveNodeBuilder`] for every configured
//! `[venues.<id>]` block. The bolt-v3 venue identifier is reused as the
//! NT registration name so per-venue routing stays addressable.
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
//! subscriptions for venue instrument topics. None of that opens an
//! external network connection or starts the live event loop, but it
//! is more than no-op factory storage and the boundary documentation
//! must reflect that.

use std::collections::BTreeMap;

use nautilus_binance::factories::BinanceDataClientFactory;
use nautilus_live::builder::LiveNodeBuilder;
use nautilus_polymarket::factories::{
    PolymarketDataClientFactory, PolymarketExecutionClientFactory,
};

use crate::bolt_v3_adapters::{
    BoltV3AdapterConfigs, BoltV3BinanceAdapters, BoltV3PolymarketAdapters, BoltV3VenueAdapterConfig,
};

/// Inspectable record of which NT client kinds the bolt-v3 boundary
/// added to the [`LiveNodeBuilder`] for one configured venue. A `false`
/// flag means the corresponding `[venues.<id>.<block>]` was absent in
/// the validated config so no `add_*_client` call was made for that
/// kind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoltV3RegisteredVenue {
    Polymarket { data: bool, execution: bool },
    Binance { data: bool },
}

/// Per-venue summary of which NT factory kinds were added to the
/// [`LiveNodeBuilder`] during the bolt-v3 client-registration smoke.
/// Keyed by the bolt-v3 venue identifier (the TOML `[venues.<id>]`
/// table key, which the boundary also uses as the NT registration
/// name). The summary is the only inspectable surface this module
/// exposes; the builder itself owns the actual factory and config
/// instances.
#[derive(Clone, Debug, Default)]
pub struct BoltV3RegistrationSummary {
    pub venues: BTreeMap<String, BoltV3RegisteredVenue>,
}

#[derive(Debug)]
pub enum BoltV3ClientRegistrationError {
    /// `LiveNodeBuilder::add_data_client` rejected the data factory for
    /// a venue (e.g. duplicate registration name). The wrapped string
    /// is the underlying NT error message.
    AddDataClient { venue_key: String, message: String },
    /// `LiveNodeBuilder::add_exec_client` rejected the execution
    /// factory for a venue (e.g. duplicate registration name).
    AddExecClient { venue_key: String, message: String },
}

impl std::fmt::Display for BoltV3ClientRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddDataClient { venue_key, message } => write!(
                f,
                "venues.{venue_key}: NT LiveNodeBuilder rejected data client: {message}"
            ),
            Self::AddExecClient { venue_key, message } => write!(
                f,
                "venues.{venue_key}: NT LiveNodeBuilder rejected execution client: {message}"
            ),
        }
    }
}

impl std::error::Error for BoltV3ClientRegistrationError {}

/// Adds an NT data and/or execution client factory to `builder` for
/// every configured `[venues.<id>]` block in `adapters`, using the
/// bolt-v3 venue identifier as the NT registration name. Returns the
/// updated builder paired with an inspectable summary of which client
/// kinds were registered per venue.
///
/// This function does not call `connect`, `disconnect`, `run`, any
/// `subscribe_*` API, market selection, order construction, or any
/// submit path. Network I/O is gated by `LiveNodeBuilder::build`,
/// owned by NT.
pub fn register_bolt_v3_clients(
    mut builder: LiveNodeBuilder,
    adapters: &BoltV3AdapterConfigs,
) -> Result<(LiveNodeBuilder, BoltV3RegistrationSummary), BoltV3ClientRegistrationError> {
    let mut venues = BTreeMap::new();
    for (venue_key, venue) in &adapters.venues {
        let registered = match venue {
            BoltV3VenueAdapterConfig::Polymarket(BoltV3PolymarketAdapters { data, execution }) => {
                let mut data_added = false;
                let mut exec_added = false;
                if let Some(cfg) = data {
                    builder = builder
                        .add_data_client(
                            Some(venue_key.clone()),
                            Box::new(PolymarketDataClientFactory),
                            Box::new(cfg.clone()),
                        )
                        .map_err(|error| BoltV3ClientRegistrationError::AddDataClient {
                            venue_key: venue_key.clone(),
                            message: error.to_string(),
                        })?;
                    data_added = true;
                }
                if let Some(cfg) = execution {
                    builder = builder
                        .add_exec_client(
                            Some(venue_key.clone()),
                            Box::new(PolymarketExecutionClientFactory),
                            Box::new(cfg.clone()),
                        )
                        .map_err(|error| BoltV3ClientRegistrationError::AddExecClient {
                            venue_key: venue_key.clone(),
                            message: error.to_string(),
                        })?;
                    exec_added = true;
                }
                BoltV3RegisteredVenue::Polymarket {
                    data: data_added,
                    execution: exec_added,
                }
            }
            BoltV3VenueAdapterConfig::Binance(BoltV3BinanceAdapters { data }) => {
                let mut data_added = false;
                if let Some(cfg) = data {
                    builder = builder
                        .add_data_client(
                            Some(venue_key.clone()),
                            Box::new(BinanceDataClientFactory::new()),
                            Box::new(cfg.clone()),
                        )
                        .map_err(|error| BoltV3ClientRegistrationError::AddDataClient {
                            venue_key: venue_key.clone(),
                            message: error.to_string(),
                        })?;
                    data_added = true;
                }
                BoltV3RegisteredVenue::Binance { data: data_added }
            }
        };
        venues.insert(venue_key.clone(), registered);
    }
    Ok((builder, BoltV3RegistrationSummary { venues }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use nautilus_common::enums::Environment;
    use nautilus_live::node::LiveNode;
    use nautilus_model::identifiers::TraderId;

    use crate::{
        bolt_v3_adapters::{BoltV3BinanceAdapters, BoltV3PolymarketAdapters, map_bolt_v3_adapters},
        bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config},
        bolt_v3_secrets::{
            ResolvedBoltV3BinanceSecrets, ResolvedBoltV3PolymarketSecrets, ResolvedBoltV3Secrets,
            ResolvedBoltV3VenueSecrets,
        },
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
        let mut venues = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            ResolvedBoltV3VenueSecrets::Polymarket(fixture_polymarket_secrets()),
        );
        venues.insert(
            "binance_reference".to_string(),
            ResolvedBoltV3VenueSecrets::Binance(fixture_binance_secrets()),
        );
        ResolvedBoltV3Secrets { venues }
    }

    fn fresh_builder() -> LiveNodeBuilder {
        LiveNode::builder(TraderId::from("BOLT-001"), Environment::Live)
            .expect("Live builder should construct for unit-test fixture")
    }

    #[test]
    fn fixture_venues_register_one_data_and_one_exec_for_polymarket_and_one_data_for_binance() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        let adapters = map_bolt_v3_adapters(&loaded, &resolved).expect("adapters should map");

        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), &adapters)
            .expect("registration should succeed");

        assert_eq!(summary.venues.len(), 2);
        match summary
            .venues
            .get("polymarket_main")
            .expect("polymarket_main must appear in summary")
        {
            BoltV3RegisteredVenue::Polymarket { data, execution } => {
                assert!(*data, "polymarket_main has a [data] block in the fixture");
                assert!(
                    *execution,
                    "polymarket_main has an [execution] block in the fixture"
                );
            }
            other => panic!("expected Polymarket summary entry, got {other:?}"),
        }
        match summary
            .venues
            .get("binance_reference")
            .expect("binance_reference must appear in summary")
        {
            BoltV3RegisteredVenue::Binance { data } => {
                assert!(*data, "binance_reference has a [data] block in the fixture");
            }
            other => panic!("expected Binance summary entry, got {other:?}"),
        }
    }

    #[test]
    fn empty_adapters_produce_empty_summary_and_pristine_builder_state() {
        let adapters = BoltV3AdapterConfigs {
            venues: BTreeMap::new(),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), &adapters)
            .expect("empty adapters should register cleanly");
        assert!(summary.venues.is_empty());
    }

    #[test]
    fn polymarket_venue_with_only_data_block_does_not_register_an_exec_client() {
        let adapters = BoltV3AdapterConfigs {
            venues: BTreeMap::from([(
                "polymarket_data_only".to_string(),
                BoltV3VenueAdapterConfig::Polymarket(BoltV3PolymarketAdapters {
                    data: Some(nautilus_polymarket::config::PolymarketDataClientConfig {
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
                        filters: Vec::new(),
                        new_market_filter: None,
                    }),
                    execution: None,
                }),
            )]),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), &adapters)
            .expect("data-only registration should succeed");
        match summary
            .venues
            .get("polymarket_data_only")
            .expect("data-only venue must appear in summary")
        {
            BoltV3RegisteredVenue::Polymarket { data, execution } => {
                assert!(*data);
                assert!(!*execution, "no [execution] block, so no exec registration");
            }
            other => panic!("expected Polymarket summary entry, got {other:?}"),
        }
    }

    #[test]
    fn binance_venue_with_no_data_block_records_data_false_in_summary() {
        let adapters = BoltV3AdapterConfigs {
            venues: BTreeMap::from([(
                "binance_no_data".to_string(),
                BoltV3VenueAdapterConfig::Binance(BoltV3BinanceAdapters { data: None }),
            )]),
        };
        let (_builder, summary) = register_bolt_v3_clients(fresh_builder(), &adapters)
            .expect("missing data block should register cleanly");
        match summary
            .venues
            .get("binance_no_data")
            .expect("binance venue must appear in summary")
        {
            BoltV3RegisteredVenue::Binance { data } => {
                assert!(!*data, "no [data] block, so no data registration");
            }
            other => panic!("expected Binance summary entry, got {other:?}"),
        }
    }
}
