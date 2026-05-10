//! Adapter config mapping for Bolt-v3.
//!
//! Converts a validated [`LoadedBoltV3Config`] plus already-resolved SSM
//! secrets ([`ResolvedBoltV3Secrets`]) into provider-owned NT client
//! factory/config assemblies.
//!
//! The mapper is intentionally a no-trade boundary: it produces config
//! struct values only and never registers clients, opens connections,
//! starts an event loop, selects markets, constructs orders, or enables
//! any submit path. Secrets travel only through the resolved-secrets
//! struct passed in by the caller; AWS Systems Manager is never touched
//! here.

use std::{collections::BTreeMap, fmt, sync::Arc};

use nautilus_common::factories::{ClientConfig, DataClientFactory, ExecutionClientFactory};

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_market_families::updown::MarketIdentityPlan,
    bolt_v3_providers::{self, ProviderAdapterMapContext},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};

/// Boxed closure used by the provider-binding layer to obtain the
/// current unix-seconds value at the moment a provider filter wants
/// fresh slugs. The closure is invoked from inside the provider's
/// `load_all` cycle on every refresh, so it must be `Send + Sync` and
/// own all state it captures. Tests inject a fixed-time closure;
/// future live wiring will inject one backed by an NT runtime clock.
pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Provider-owned NT data-client factory and config for one configured
/// Bolt-v3 client data block.
pub struct BoltV3DataClientAdapterConfig {
    pub factory: Box<dyn DataClientFactory>,
    pub config: Box<dyn ClientConfig>,
}

/// Provider-owned NT execution-client factory and config for one configured
/// Bolt-v3 client execution block.
pub struct BoltV3ExecutionClientAdapterConfig {
    pub factory: Box<dyn ExecutionClientFactory>,
    pub config: Box<dyn ClientConfig>,
}

/// Mapped provider-owned adapter assemblies for one configured Bolt-v3
/// client. Sub-configs are present iff the corresponding
/// `[clients.<id>.<block>]` section is present in the validated config.
pub struct BoltV3ClientConfig {
    pub data: Option<BoltV3DataClientAdapterConfig>,
    pub execution: Option<BoltV3ExecutionClientAdapterConfig>,
}

impl BoltV3DataClientAdapterConfig {
    pub fn config_as<T: 'static>(&self) -> Option<&T> {
        self.config.as_any().downcast_ref()
    }
}

impl BoltV3ExecutionClientAdapterConfig {
    pub fn config_as<T: 'static>(&self) -> Option<&T> {
        self.config.as_any().downcast_ref()
    }
}

/// Mapped NT-native client configs keyed by the bolt-v3 client
/// identifier (the TOML `[clients.<id>]` table key).
pub struct BoltV3ClientConfigs {
    pub clients: BTreeMap<String, BoltV3ClientConfig>,
}

impl fmt::Debug for BoltV3DataClientAdapterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3DataClientAdapterConfig")
            .field("factory", &self.factory.name())
            .field("config_type", &self.factory.config_type())
            .finish()
    }
}

impl fmt::Debug for BoltV3ExecutionClientAdapterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3ExecutionClientAdapterConfig")
            .field("factory", &self.factory.name())
            .field("config_type", &self.factory.config_type())
            .finish()
    }
}

impl fmt::Debug for BoltV3ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3ClientConfig")
            .field("data", &self.data)
            .field("execution", &self.execution)
            .finish()
    }
}

impl fmt::Debug for BoltV3ClientConfigs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3ClientConfigs")
            .field("clients", &self.clients)
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3ClientMappingError {
    /// The validated venue and the resolved secret handle disagree.
    /// Indicates an internal-consistency bug between the resolver output
    /// and the mapper inputs.
    SecretVenueMismatch {
        client_id_key: String,
        expected_venue: &'static str,
    },
    /// A client requires resolved secrets but none were found in the
    /// passed-in `ResolvedBoltV3Secrets`. Validation guarantees a
    /// `[secrets]` block exists, so reaching this branch indicates the
    /// resolved-secrets value was constructed inconsistently with the
    /// loaded config.
    MissingResolvedSecrets {
        client_id_key: String,
        expected_venue: &'static str,
    },
    /// A `[data]` or `[execution]` block existed but failed to
    /// deserialize into the corresponding NT-native shape. The validator
    /// runs the same `try_into` calls before the mapper, so reaching
    /// this branch means the inputs were mutated between validation and
    /// mapping.
    SchemaParse {
        client_id_key: String,
        block: &'static str,
        message: String,
    },
    /// A bolt-v3 numeric config value did not fit the NT-native field
    /// type on this target (e.g. `u64 -> usize` overflow on a 32-bit
    /// build). No silent truncation: the mapper refuses to default.
    NumericRange {
        client_id_key: String,
        field: &'static str,
        message: String,
    },
    /// The caller passed a config value that validated bolt-v3 startup
    /// must reject before mapping to NT. Keeping this guard at the
    /// mapper boundary prevents programmatic callers from bypassing
    /// root validation and reaching a hidden NT runtime behavior.
    ValidationInvariant {
        client_id_key: String,
        field: &'static str,
        message: String,
    },
}

impl std::fmt::Display for BoltV3ClientMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3ClientMappingError::SecretVenueMismatch {
                client_id_key,
                expected_venue,
            } => write!(
                f,
                "clients.{client_id_key}: resolved secret handle does not match validated venue \
                 (expected {venue})",
                venue = expected_venue,
            ),
            BoltV3ClientMappingError::MissingResolvedSecrets {
                client_id_key,
                expected_venue,
            } => write!(
                f,
                "clients.{client_id_key} (venue={venue}) requires resolved SSM secrets but none were \
                 supplied to the client mapper",
                venue = expected_venue,
            ),
            BoltV3ClientMappingError::SchemaParse {
                client_id_key,
                block,
                message,
            } => write!(
                f,
                "clients.{client_id_key}.{block}: failed to deserialize into NT-native config: {message}",
            ),
            BoltV3ClientMappingError::NumericRange {
                client_id_key,
                field,
                message,
            } => write!(
                f,
                "clients.{client_id_key}.{field}: bolt-v3 value does not fit the NT-native field type: {message}",
            ),
            BoltV3ClientMappingError::ValidationInvariant {
                client_id_key,
                field,
                message,
            } => write!(
                f,
                "clients.{client_id_key}.{field}: bolt-v3 validation invariant failed at adapter mapping: {message}",
            ),
        }
    }
}

impl std::error::Error for BoltV3ClientMappingError {}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// NT-native client config values, one per configured client. The mapper
/// never re-resolves SSM and never registers clients; callers receive
/// owned config structs and may pass them to NT factories at a later
/// stage.
///
/// This entry point intentionally installs no provider filter and
/// passes an empty plan into the with-identity variant. Callers that
/// need the rotating-market filter surface MUST use
/// [`map_bolt_v3_clients_with_market_identity`] directly with a
/// derived [`MarketIdentityPlan`] and a real clock — copying the
/// `Arc::new(|| 0_i64)` sentinel below into a non-empty-plan call site
/// would produce slugs anchored to unix-second 0 every cycle.
pub fn map_bolt_v3_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3ClientConfigs, BoltV3ClientMappingError> {
    let empty_plan = MarketIdentityPlan {
        updown_targets: Vec::new(),
    };
    // The clock here is never invoked: with no updown targets, no
    // provider filter closure is built, so the closure body is never
    // entered. We wire in a deterministic constant so callers cannot
    // observe any wall-clock dependency on the no-identity entry point.
    // Treat this constant as a sentinel for the no-filter path; do not
    // reuse it from any call site that supplies a non-empty plan.
    let zero_clock: BoltV3UpdownNowFn = Arc::new(|| 0_i64);
    map_bolt_v3_clients_with_market_identity(loaded, resolved, &empty_plan, zero_clock)
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// provider-owned NT client factory/config assemblies, and additionally
/// let each provider binding install whatever provider-specific filter
/// surface corresponds to the supplied provider-neutral
/// [`MarketIdentityPlan`].
pub fn map_bolt_v3_clients_with_market_identity(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<BoltV3ClientConfigs, BoltV3ClientMappingError> {
    map_bolt_v3_clients_with_market_identity_and_venue_lookup(
        loaded,
        resolved,
        plan,
        clock,
        bolt_v3_providers::binding_for_venue,
    )
}

fn map_bolt_v3_clients_with_market_identity_and_venue_lookup(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
    binding_for_venue: impl Fn(&str) -> Option<&'static bolt_v3_providers::ProviderBinding>,
) -> Result<BoltV3ClientConfigs, BoltV3ClientMappingError> {
    validate_market_identity_target_clients(loaded, plan)?;
    let mut clients = BTreeMap::new();
    for (client_id_key, client_id) in &loaded.root.clients {
        let Some(binding) = binding_for_venue(client_id.venue.as_str()) else {
            return Err(BoltV3ClientMappingError::ValidationInvariant {
                client_id_key: client_id_key.clone(),
                field: "venue",
                message: format!(
                    "venue `{}` is not supported by this build",
                    client_id.venue.as_str()
                ),
            });
        };
        validate_provider_market_family_support(client_id_key, binding, plan)?;
        let mapped = (binding.map_adapters)(ProviderAdapterMapContext {
            root: &loaded.root,
            client_id_key: client_id_key,
            client_id,
            resolved,
            plan,
            clock: clock.clone(),
        })?;
        clients.insert(client_id_key.clone(), mapped);
    }
    Ok(BoltV3ClientConfigs { clients })
}

fn validate_provider_market_family_support(
    client_id_key: &str,
    binding: &bolt_v3_providers::ProviderBinding,
    plan: &MarketIdentityPlan,
) -> Result<(), BoltV3ClientMappingError> {
    // Only clients referenced by a market-identity target need family
    // support. A provider with an empty `supported_market_families`
    // remains valid for data-only/reference clients that no strategy
    // target routes through.
    for target in plan
        .client_id_target_refs()
        .filter(|target| target.client_id_key == client_id_key)
    {
        if !binding
            .supported_market_families
            .contains(&target.family_key)
        {
            return Err(BoltV3ClientMappingError::ValidationInvariant {
                client_id_key: target.client_id_key.to_string(),
                field: "strategy.execution_client_id",
                message: format!(
                    "configured target `{}` uses market family `{}` on client `{}`, but venue `{}` does not support that market family",
                    target.configured_target_id,
                    target.family_key,
                    target.client_id_key,
                    binding.key,
                ),
            });
        }
    }
    Ok(())
}

fn validate_market_identity_target_clients(
    loaded: &LoadedBoltV3Config,
    plan: &MarketIdentityPlan,
) -> Result<(), BoltV3ClientMappingError> {
    for target in plan.client_id_target_refs() {
        if !loaded.root.clients.contains_key(target.client_id_key) {
            return Err(BoltV3ClientMappingError::ValidationInvariant {
                client_id_key: target.client_id_key.to_string(),
                field: "strategy.execution_client_id",
                message: format!(
                    "configured target `{}` references unknown client `{}`",
                    target.configured_target_id, target.client_id_key,
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{path::PathBuf, sync::Arc};

    use nautilus_binance::{
        common::enums::{
            BinanceEnvironment as NtBinanceEnvironment, BinanceProductType as NtBinanceProductType,
        },
        config::BinanceDataClientConfig,
    };
    use nautilus_model::identifiers::{AccountId, TraderId};
    use nautilus_polymarket::{
        common::enums::SignatureType as NtPolymarketSignatureType,
        config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    };

    use crate::bolt_v3_config::BoltV3RootConfig;
    use crate::bolt_v3_market_families::updown::{self, UpdownTargetPlan};
    use crate::bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderBinding, ProviderResolvedSecrets,
        ProviderSecretResolveContext, ResolvedClientSecrets, SsmSecretResolver,
        binance::{self, ResolvedBoltV3BinanceSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
    };
    use crate::bolt_v3_secrets::{
        BoltV3SecretError, ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets,
    };

    const FAKE_UPDOWN_PROVIDER_KEY: &str = "FAKE_UPDOWN_PROVIDER";

    #[derive(Debug)]
    struct FakeProviderSecrets;

    impl ProviderResolvedSecrets for FakeProviderSecrets {
        fn venue_key(&self) -> &'static str {
            FAKE_UPDOWN_PROVIDER_KEY
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn validate_fake_provider_client_id(
        _key: &str,
        _client_id: &crate::bolt_v3_config::ClientBlock,
    ) -> Vec<String> {
        Vec::new()
    }

    fn resolve_fake_provider_secrets(
        _context: ProviderSecretResolveContext<'_>,
        _resolver: &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
        Ok(Arc::new(FakeProviderSecrets))
    }

    fn map_fake_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3ClientConfig, BoltV3ClientMappingError> {
        assert_eq!(context.client_id.venue.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.client_id_key, "polymarket_main");
        assert_eq!(context.plan.updown_targets.len(), 1);
        assert_eq!(
            context.plan.updown_targets[0].client_id_key,
            context.client_id_key
        );
        Ok(BoltV3ClientConfig {
            data: None,
            execution: None,
        })
    }

    fn map_fake_no_target_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3ClientConfig, BoltV3ClientMappingError> {
        assert_eq!(context.client_id.venue.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.client_id_key, "polymarket_main");
        assert!(context.plan.updown_targets.is_empty());
        Ok(BoltV3ClientConfig {
            data: None,
            execution: None,
        })
    }

    static FAKE_UPDOWN_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_client_id: validate_fake_provider_client_id,
        supported_market_families: &[updown::KEY],
        required_secret_blocks: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_provider_adapters,
    };

    static FAKE_UNSUPPORTED_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_client_id: validate_fake_provider_client_id,
        supported_market_families: &[],
        required_secret_blocks: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_provider_adapters,
    };

    static FAKE_UNSUPPORTED_NO_TARGET_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_client_id: validate_fake_provider_client_id,
        supported_market_families: &[],
        required_secret_blocks: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_no_target_provider_adapters,
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
            private_key: "fixture-poly-private-key".to_string(),
            api_key: "fixture-poly-api-key".to_string(),
            api_secret: "fixture-poly-api-secret".to_string(),
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

    #[test]
    fn injected_provider_binding_can_accept_updown_target_without_core_provider_edit() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_id_key, _block| client_id_key == "polymarket_main");
        let plan = MarketIdentityPlan {
            updown_targets: vec![UpdownTargetPlan {
                strategy_instance_id: "fake-strategy".to_string(),
                configured_target_id: "fake-updown".to_string(),
                client_id_key: "polymarket_main".to_string(),
                underlying_asset: "BTC".to_string(),
                cadence_seconds: 300,
                cadence_slug_token: "5m".to_string(),
            }],
        };
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let configs = map_bolt_v3_clients_with_market_identity_and_venue_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            |key| {
                if key == FAKE_UPDOWN_PROVIDER_KEY {
                    Some(&FAKE_UPDOWN_PROVIDER_BINDING)
                } else {
                    None
                }
            },
        )
        .expect("core mapping should route through the injected fake provider binding");

        let fake = configs
            .clients
            .get("polymarket_main")
            .expect("fake provider client should map");
        assert!(fake.data.is_none());
        assert!(fake.execution.is_none());
    }

    #[test]
    fn injected_provider_binding_without_family_support_rejects_before_provider_mapping() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_id_key, _block| client_id_key == "polymarket_main");
        let plan = MarketIdentityPlan {
            updown_targets: vec![UpdownTargetPlan {
                strategy_instance_id: "fake-strategy".to_string(),
                configured_target_id: "fake-updown".to_string(),
                client_id_key: "polymarket_main".to_string(),
                underlying_asset: "BTC".to_string(),
                cadence_seconds: 300,
                cadence_slug_token: "5m".to_string(),
            }],
        };
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let error = map_bolt_v3_clients_with_market_identity_and_venue_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            |key| {
                if key == FAKE_UPDOWN_PROVIDER_KEY {
                    Some(&FAKE_UNSUPPORTED_PROVIDER_BINDING)
                } else {
                    None
                }
            },
        )
        .expect_err("core mapping must reject unsupported market families before provider mapping");

        match error {
            BoltV3ClientMappingError::ValidationInvariant {
                client_id_key,
                field,
                message,
            } => {
                assert_eq!(client_id_key, "polymarket_main");
                assert_eq!(field, "strategy.execution_client_id");
                assert!(message.contains("does not support that market family"));
            }
            other => panic!("expected ValidationInvariant, got {other}"),
        }
    }

    #[test]
    fn provider_without_family_support_can_map_when_no_target_references_client_id() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_id_key, _block| client_id_key == "polymarket_main");
        let plan = MarketIdentityPlan {
            updown_targets: Vec::new(),
        };
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let configs = map_bolt_v3_clients_with_market_identity_and_venue_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            |key| {
                if key == FAKE_UPDOWN_PROVIDER_KEY {
                    Some(&FAKE_UNSUPPORTED_NO_TARGET_PROVIDER_BINDING)
                } else {
                    None
                }
            },
        )
        .expect("family support check applies only to clients referenced by plan targets");

        assert!(configs.clients.contains_key("polymarket_main"));
    }

    #[test]
    fn maps_polymarket_client_id_data_and_execution_blocks_from_fixture() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map cleanly");

        let polymarket = configs
            .clients
            .get("polymarket_main")
            .expect("polymarket_main must be present");

        let data = polymarket
            .data
            .as_ref()
            .expect("polymarket [data] block must map")
            .config_as::<PolymarketDataClientConfig>()
            .expect("polymarket data config should downcast to NT config");
        assert_eq!(
            data.base_url_http.as_deref(),
            Some("https://clob.polymarket.com")
        );
        assert_eq!(
            data.base_url_ws.as_deref(),
            Some("wss://ws-subscriptions-clob.polymarket.com/ws/market")
        );
        assert_eq!(
            data.base_url_gamma.as_deref(),
            Some("https://gamma-api.polymarket.com")
        );
        assert_eq!(
            data.base_url_data_api.as_deref(),
            Some("https://data-api.polymarket.com")
        );
        assert_eq!(data.http_timeout_secs, 60);
        assert_eq!(data.ws_timeout_secs, 30);
        assert_eq!(data.ws_max_subscriptions, 200);
        assert_eq!(data.update_instruments_interval_mins, 60);
        assert!(!data.subscribe_new_markets);
        assert!(data.filters.is_empty());
        assert!(data.new_market_filter.is_none());

        let exec = polymarket
            .execution
            .as_ref()
            .expect("polymarket [execution] block must map")
            .config_as::<PolymarketExecClientConfig>()
            .expect("polymarket execution config should downcast to NT config");
        assert_eq!(exec.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(exec.account_id, AccountId::from("POLYMARKET-001"));
        assert_eq!(
            exec.private_key.as_deref(),
            Some("fixture-poly-private-key")
        );
        assert_eq!(exec.api_key.as_deref(), Some("fixture-poly-api-key"));
        assert_eq!(exec.api_secret.as_deref(), Some("fixture-poly-api-secret"));
        assert_eq!(exec.passphrase.as_deref(), Some("fixture-poly-passphrase"));
        assert_eq!(
            exec.funder.as_deref(),
            Some("0x1111111111111111111111111111111111111111")
        );
        assert_eq!(exec.signature_type, NtPolymarketSignatureType::PolyProxy);
        assert_eq!(
            exec.base_url_http.as_deref(),
            Some("https://clob.polymarket.com")
        );
        assert_eq!(
            exec.base_url_ws.as_deref(),
            Some("wss://ws-subscriptions-clob.polymarket.com/ws/user")
        );
        assert_eq!(
            exec.base_url_data_api.as_deref(),
            Some("https://data-api.polymarket.com")
        );
        assert_eq!(exec.http_timeout_secs, 60);
        assert_eq!(exec.max_retries, 3);
        assert_eq!(exec.retry_delay_initial_ms, 250);
        assert_eq!(exec.retry_delay_max_ms, 2000);
        assert_eq!(exec.ack_timeout_secs, 5);
    }

    #[test]
    fn maps_binance_client_id_data_block_from_fixture() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map cleanly");

        let binance = configs
            .clients
            .get("binance_reference")
            .expect("binance_reference must be present");
        let data = binance
            .data
            .as_ref()
            .expect("binance [data] block must map")
            .config_as::<BinanceDataClientConfig>()
            .expect("binance data config should downcast to NT config");

        assert_eq!(data.product_types, vec![NtBinanceProductType::Spot]);
        assert_eq!(data.environment, NtBinanceEnvironment::Mainnet);
        // base_url_http and base_url_ws are now required bolt-v3
        // fields; the mapper must pass the configured values through to
        // NT as `Some(...)` rather than letting NT fall back to its
        // compiled-in defaults.
        assert_eq!(
            data.base_url_http.as_deref(),
            Some("https://api.binance.com")
        );
        assert_eq!(
            data.base_url_ws.as_deref(),
            Some("wss://stream.binance.com:9443/ws")
        );
        assert_eq!(data.api_key.as_deref(), Some("fixture-binance-api-key"));
        assert_eq!(
            data.api_secret.as_deref(),
            Some("fixture-binance-api-secret")
        );
        assert_eq!(data.instrument_status_poll_secs, 3600);
    }

    #[test]
    fn missing_resolved_secrets_for_polymarket_execution_is_a_mapping_error() {
        let loaded = fixture_loaded_config();
        // Provide the binance_reference secret entry so map iteration
        // reaches `polymarket_main` (which is alphabetically later in
        // the BTreeMap) and trips on the missing polymarket secrets.
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = map_bolt_v3_clients(&loaded, &resolved)
            .expect_err("missing resolved secrets must surface as a mapper error");
        match error {
            BoltV3ClientMappingError::MissingResolvedSecrets {
                client_id_key,
                expected_venue,
            } => {
                assert_eq!(client_id_key, "polymarket_main");
                assert_eq!(expected_venue, polymarket::KEY);
            }
            other => panic!("expected MissingResolvedSecrets, got {other}"),
        }
    }

    #[test]
    fn missing_resolved_secrets_for_binance_data_is_a_mapping_error() {
        let loaded = fixture_loaded_config();
        // Provide only polymarket_main so iteration succeeds for it and
        // fails when it reaches `binance_reference` with no entry. This
        // pairs with the polymarket case so neither alphabetical
        // position can hide an unmapped resolved-secrets gap.
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_polymarket_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = map_bolt_v3_clients(&loaded, &resolved)
            .expect_err("missing binance resolved secrets must surface as a mapper error");
        match error {
            BoltV3ClientMappingError::MissingResolvedSecrets {
                client_id_key,
                expected_venue,
            } => {
                assert_eq!(client_id_key, "binance_reference");
                assert_eq!(expected_venue, binance::KEY);
            }
            other => panic!("expected MissingResolvedSecrets, got {other}"),
        }
    }

    #[test]
    fn mismatched_resolved_secret_handle_is_a_mapping_error() {
        let loaded = fixture_loaded_config();
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = map_bolt_v3_clients(&loaded, &resolved)
            .expect_err("mismatched resolved secret handle must surface as a mapper error");
        match error {
            BoltV3ClientMappingError::SecretVenueMismatch {
                client_id_key,
                expected_venue,
            } => {
                assert_eq!(client_id_key, "polymarket_main");
                assert_eq!(expected_venue, polymarket::KEY);
            }
            other => panic!("expected SecretVenueMismatch, got {other}"),
        }
    }

    #[test]
    fn binance_adapter_debug_redacts_resolved_api_credentials() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map");
        let debug = format!("{configs:?}");

        assert!(debug.contains("BinanceDataClientConfig"));
        assert!(debug.contains("BinanceDataClientConfig"));
        for raw_secret in [
            fixture_binance_secrets().api_key.as_str(),
            fixture_binance_secrets().api_secret.as_str(),
        ] {
            assert!(
                !debug.contains(raw_secret),
                "binance adapter Debug must not leak resolved secret values"
            );
        }
    }

    #[test]
    fn polymarket_adapter_debug_does_not_leak_resolved_credentials() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        let configs = map_bolt_v3_clients(&loaded, &resolved).expect("fixture should map");
        let debug = format!("{configs:?}");

        for raw_secret in [
            fixture_polymarket_secrets().private_key.as_str(),
            fixture_polymarket_secrets().api_key.as_str(),
            fixture_polymarket_secrets().api_secret.as_str(),
            fixture_polymarket_secrets().passphrase.as_str(),
        ] {
            assert!(
                !debug.contains(raw_secret),
                "polymarket adapter Debug must not leak resolved secret values"
            );
        }
    }

    // The no-trade-boundary source-inspection check lives in the
    // `tests/bolt_v3_adapter_mapping.rs` integration test so the
    // forbidden-strings list is not part of this module's own source
    // (which would otherwise self-trip the assertion).
}
