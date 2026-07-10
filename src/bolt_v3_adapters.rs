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

use nautilus_common::{
    clock::Clock,
    factories::{ClientConfig, DataClientFactory, ExecutionClientFactory},
    live::clock::LiveClock,
    runner::try_get_time_event_sender,
};
use nautilus_core::datetime::NANOSECONDS_IN_SECOND;

use crate::{
    bolt_v3_config::LoadedBoltV3Config,
    bolt_v3_market_families::{
        MarketIdentityPlan, MarketIdentityPlanError, market_identity_plan_from_config,
    },
    bolt_v3_providers::{self, ProviderAdapterMapContext, ProviderRuntimeApprovals},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};

/// Boxed closure used by the provider-binding layer to obtain the
/// current unix-seconds value at the moment a provider filter wants
/// fresh slugs. The closure is invoked from inside the provider's
/// `load_all` cycle on every refresh, so it must be `Send + Sync` and
/// own all state it captures. Tests inject a fixed-time closure;
/// future live wiring will inject one backed by an NT runtime clock.
pub type BoltV3MarketClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

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
pub struct BoltV3ClientAdapterConfig {
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

/// Mapped NT-native adapter configs keyed by the bolt-v3 client
/// identifier (the TOML `[clients.<id>]` table key).
pub struct BoltV3AdapterConfigs {
    pub clients: BTreeMap<String, BoltV3ClientAdapterConfig>,
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

impl fmt::Debug for BoltV3ClientAdapterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3ClientAdapterConfig")
            .field("data", &self.data)
            .field("execution", &self.execution)
            .finish()
    }
}

impl fmt::Debug for BoltV3AdapterConfigs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3AdapterConfigs")
            .field("clients", &self.clients)
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3AdapterMappingError {
    MarketIdentity(MarketIdentityPlanError),
    /// The configured client provider and the resolved secret provider disagree.
    /// Indicates an internal-consistency bug between the resolver output
    /// and the mapper inputs.
    SecretProviderMismatch {
        client_key: String,
        expected_provider_key: &'static str,
    },
    /// A client requires resolved secrets but none were found in the
    /// passed-in `ResolvedBoltV3Secrets`. Validation guarantees a
    /// `[secrets]` block exists, so reaching this branch indicates the
    /// resolved-secrets value was constructed inconsistently with the
    /// loaded config.
    MissingResolvedSecrets {
        client_key: String,
        expected_provider_key: &'static str,
    },
    /// A `[data]` or `[execution]` block existed but failed to
    /// deserialize into the corresponding NT-native shape. The validator
    /// runs the same `try_into` calls before the mapper, so reaching
    /// this branch means the inputs were mutated between validation and
    /// mapping.
    SchemaParse {
        client_key: String,
        block: &'static str,
        message: String,
    },
    /// A bolt-v3 numeric config value did not fit the NT-native field
    /// type on this target (e.g. `u64 -> usize` overflow on a 32-bit
    /// build). No silent truncation: the mapper refuses to default.
    NumericRange {
        client_key: String,
        field: &'static str,
        message: String,
    },
    /// The caller passed a config value that validated bolt-v3 startup
    /// must reject before mapping to NT. Keeping this guard at the
    /// mapper boundary prevents programmatic callers from bypassing
    /// root validation and reaching a hidden NT runtime behavior.
    ValidationInvariant {
        client_key: String,
        field: &'static str,
        message: String,
    },
}

impl std::fmt::Display for BoltV3AdapterMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3AdapterMappingError::MarketIdentity(error) => {
                write!(f, "bolt-v3 market identity plan failed: {error}")
            }
            BoltV3AdapterMappingError::SecretProviderMismatch {
                client_key,
                expected_provider_key,
            } => write!(
                f,
                "clients.{client_key}: resolved secret provider does not match configured client provider \
                 (expected {provider})",
                provider = expected_provider_key,
            ),
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                client_key,
                expected_provider_key,
            } => write!(
                f,
                "clients.{client_key} (provider={provider}) requires resolved SSM secrets but none were \
                 supplied to the adapter mapper",
                provider = expected_provider_key,
            ),
            BoltV3AdapterMappingError::SchemaParse {
                client_key,
                block,
                message,
            } => write!(
                f,
                "clients.{client_key}.{block}: failed to deserialize into NT-native config: {message}",
            ),
            BoltV3AdapterMappingError::NumericRange {
                client_key,
                field,
                message,
            } => write!(
                f,
                "clients.{client_key}.{field}: bolt-v3 value does not fit the NT-native field type: {message}",
            ),
            BoltV3AdapterMappingError::ValidationInvariant {
                client_key,
                field,
                message,
            } => write!(
                f,
                "clients.{client_key}: bolt-v3 validation invariant failed for {field}: {message}",
            ),
        }
    }
}

impl std::error::Error for BoltV3AdapterMappingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3AdapterMappingError::MarketIdentity(error) => Some(error),
            BoltV3AdapterMappingError::SecretProviderMismatch { .. }
            | BoltV3AdapterMappingError::MissingResolvedSecrets { .. }
            | BoltV3AdapterMappingError::SchemaParse { .. }
            | BoltV3AdapterMappingError::NumericRange { .. }
            | BoltV3AdapterMappingError::ValidationInvariant { .. } => None,
        }
    }
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// NT-native adapter config values, one per configured client. The mapper
/// never re-resolves SSM and never registers clients; callers receive
/// owned config structs and may pass them to NT factories at a later
/// stage.
///
/// This entry point derives [`MarketIdentityPlan`] from the loaded
/// strategy TOML and gives provider bindings an NT
/// [`LiveClock`]-backed timestamp source for filter projection.
pub fn map_bolt_v3_adapters(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    map_bolt_v3_adapters_with_runtime_approvals(loaded, resolved, ProviderRuntimeApprovals::none())
}

pub fn map_bolt_v3_adapters_with_runtime_approvals(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    runtime_approvals: ProviderRuntimeApprovals<'_>,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    let plan = market_identity_plan_from_config(loaded)
        .map_err(BoltV3AdapterMappingError::MarketIdentity)?;
    map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        loaded,
        resolved,
        &plan,
        nt_market_clock(),
        runtime_approvals,
    )
}

fn nt_market_clock() -> BoltV3MarketClockFn {
    let clock = LiveClock::new(try_get_time_event_sender());
    Arc::new(move || {
        let now_unix_seconds = clock.timestamp_ns().as_u64() / NANOSECONDS_IN_SECOND;
        now_unix_seconds.min(i64::MAX as u64) as i64
    })
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// provider-owned NT client factory/config assemblies, and additionally
/// let each provider binding install whatever provider-specific filter
/// surface corresponds to the supplied provider-neutral
/// [`MarketIdentityPlan`].
pub fn map_bolt_v3_adapters_with_market_identity(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3MarketClockFn,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
        loaded,
        resolved,
        plan,
        clock,
        ProviderRuntimeApprovals::none(),
    )
}

pub fn map_bolt_v3_adapters_with_market_identity_and_runtime_approvals(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3MarketClockFn,
    runtime_approvals: ProviderRuntimeApprovals<'_>,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    map_bolt_v3_adapters_with_market_identity_and_provider_lookup(
        loaded,
        resolved,
        plan,
        clock,
        runtime_approvals,
        bolt_v3_providers::binding_for_provider_key,
    )
}

fn map_bolt_v3_adapters_with_market_identity_and_provider_lookup(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3MarketClockFn,
    runtime_approvals: ProviderRuntimeApprovals<'_>,
    binding_for_provider_key: impl Fn(&str) -> Option<&'static bolt_v3_providers::ProviderBinding>,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    validate_market_identity_target_clients(loaded, plan)?;
    let mut clients = BTreeMap::new();
    for (client_key, client) in &loaded.root.clients {
        let Some(binding) = binding_for_provider_key(client.venue.as_str()) else {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.clone(),
                field: "venue",
                message: format!(
                    "venue `{}` is not supported by this build",
                    client.venue.as_str()
                ),
            });
        };
        validate_provider_market_family_support(client_key, binding, plan)?;
        let mapped = (binding.map_adapters)(ProviderAdapterMapContext {
            root: &loaded.root,
            client_key,
            client,
            resolved,
            plan,
            clock: clock.clone(),
            runtime_approvals,
        })?;
        clients.insert(client_key.clone(), mapped);
    }
    Ok(BoltV3AdapterConfigs { clients })
}

fn validate_provider_market_family_support(
    client_key: &str,
    binding: &bolt_v3_providers::ProviderBinding,
    plan: &MarketIdentityPlan,
) -> Result<(), BoltV3AdapterMappingError> {
    // Only clients referenced by a market-identity target need family
    // support. A provider with an empty `supported_market_families`
    // remains valid for data-only/reference clients that no strategy
    // target routes through.
    for target in plan
        .execution_client_target_refs()
        .filter(|target| target.execution_client_id == client_key)
    {
        if !binding
            .supported_market_families
            .contains(&target.family_key)
        {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: target.execution_client_id.to_string(),
                field: "strategy.execution_client_id",
                message: format!(
                    "configured target `{}` uses market family `{}` on client `{}`, but provider `{}` does not support that market family",
                    target.configured_target_id,
                    target.family_key,
                    target.execution_client_id,
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
) -> Result<(), BoltV3AdapterMappingError> {
    for target in plan.execution_client_target_refs() {
        if !loaded.root.clients.contains_key(target.execution_client_id) {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: target.execution_client_id.to_string(),
                field: "strategy.execution_client_id",
                message: format!(
                    "configured target `{}` references unknown client `{}`",
                    target.configured_target_id, target.execution_client_id,
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
        config::{
            BinanceDataClientConfig, BinanceSpotMarketDataMode as NtBinanceSpotMarketDataMode,
        },
    };
    use nautilus_model::identifiers::{AccountId, TraderId};
    use nautilus_polymarket::{
        common::enums::SignatureType as NtPolymarketSignatureType,
        config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    };

    use crate::bolt_v3_config::load_bolt_v3_config;
    use crate::bolt_v3_market_families::{
        static_binary_event::{self, StaticBinaryEventTargetPlan},
        updown::{self, UpdownTargetPlan},
    };
    use crate::bolt_v3_providers::{
        DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS, NtReconnectBudgetCapability,
        ProviderAdapterMapContext, ProviderBinding, ProviderResolvedSecrets,
        ProviderSecretResolveContext, ResolvedClientSecrets, SsmSecretResolver,
        binance::{self, ResolvedBoltV3BinanceSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
        polyresearch::ResolvedBoltV3PolyResearchSecrets,
    };
    use crate::bolt_v3_secrets::{
        BoltV3SecretError, ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets,
    };

    const FAKE_UPDOWN_PROVIDER_KEY: &str = "FAKE_UPDOWN_PROVIDER";

    fn nt_polymarket_signature_type(
        value: polymarket::PolymarketSignatureType,
    ) -> NtPolymarketSignatureType {
        match value {
            polymarket::PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
            polymarket::PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
            polymarket::PolymarketSignatureType::PolyGnosisSafe => {
                NtPolymarketSignatureType::PolyGnosisSafe
            }
        }
    }

    #[derive(Debug)]
    struct FakeProviderSecrets;

    impl ProviderResolvedSecrets for FakeProviderSecrets {
        fn provider_key(&self) -> &'static str {
            FAKE_UPDOWN_PROVIDER_KEY
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn redaction_values(&self) -> Vec<&str> {
            Vec::new()
        }
    }

    fn validate_fake_provider_client(
        _key: &str,
        _client: &crate::bolt_v3_config::ClientBlock,
    ) -> Vec<String> {
        Vec::new()
    }

    fn resolve_fake_provider_secrets(
        _context: ProviderSecretResolveContext<'_>,
        _resolver: &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
        Ok(Arc::new(FakeProviderSecrets))
    }

    fn configured_fake_provider_secret_paths(
        _context: ProviderSecretResolveContext<'_>,
    ) -> Result<Vec<crate::bolt_v3_providers::ProviderSsmPathReference>, BoltV3SecretError> {
        Ok(Vec::new())
    }

    fn map_fake_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
        assert_eq!(context.client.venue.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.client_key, "polymarket_main");
        let targets = updown::target_plans(context.plan).collect::<Vec<_>>();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].execution_client_id, context.client_key);
        Ok(BoltV3ClientAdapterConfig {
            data: None,
            execution: None,
        })
    }

    fn map_fake_no_target_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
        assert_eq!(context.client.venue.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.client_key, "polymarket_main");
        assert_eq!(context.plan.targets().count(), 0);
        Ok(BoltV3ClientAdapterConfig {
            data: None,
            execution: None,
        })
    }

    static FAKE_UPDOWN_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: validate_fake_provider_client,
        supported_market_families: &[updown::KEY],
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        configured_secret_paths: configured_fake_provider_secret_paths,
        map_adapters: map_fake_provider_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        build_fee_provider: None,
        build_venue_truth_runtime_source: None,
    };

    static FAKE_UNSUPPORTED_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: validate_fake_provider_client,
        supported_market_families: &[],
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        configured_secret_paths: configured_fake_provider_secret_paths,
        map_adapters: map_fake_provider_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        build_fee_provider: None,
        build_venue_truth_runtime_source: None,
    };

    static FAKE_UNSUPPORTED_NO_TARGET_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        nt_reconnect_budget: NtReconnectBudgetCapability::NotApplicable,
        validate_client: validate_fake_provider_client,
        supported_market_families: &[],
        market_exit_order_constraints: DEFAULT_MARKET_EXIT_ORDER_CONSTRAINTS,
        metadata_refresh_interval_mins: None,
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        configured_secret_paths: configured_fake_provider_secret_paths,
        map_adapters: map_fake_no_target_provider_adapters,
        load_live_submit_approval: None,
        preflight_live_submit_arming: None,
        write_live_submit_approval_artifact: None,
        write_product_submit_proof_artifact: None,
        build_fee_provider: None,
        build_venue_truth_runtime_source: None,
    };

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        load_bolt_v3_config(&PathBuf::from("tests/fixtures/bolt_v3/root.toml"))
            .expect("fixture config bundle should load")
    }

    fn binance_reference_client() -> crate::bolt_v3_config::ClientBlock {
        toml::from_str(include_str!(
            "../tests/fixtures/bolt_v3/binance_reference_client.toml"
        ))
        .expect("binance provider fixture client should parse")
    }

    fn fixture_loaded_config_with_binance_reference() -> LoadedBoltV3Config {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .clients
            .insert("binance_reference".to_string(), binance_reference_client());
        loaded
    }

    fn fixture_polymarket_secrets() -> ResolvedBoltV3PolymarketSecrets {
        ResolvedBoltV3PolymarketSecrets {
            private_key: zeroize::Zeroizing::new("fixture-poly-private-key".to_string()),
            api_key: zeroize::Zeroizing::new("fixture-poly-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-poly-api-secret".to_string()),
            passphrase: zeroize::Zeroizing::new("fixture-poly-passphrase".to_string()),
        }
    }

    fn fixture_binance_secrets() -> ResolvedBoltV3BinanceSecrets {
        ResolvedBoltV3BinanceSecrets {
            api_key: zeroize::Zeroizing::new("fixture-binance-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-binance-api-secret".to_string()),
        }
    }

    fn fixture_chainlink_strike_secrets()
    -> crate::bolt_v3_providers::chainlink::ResolvedBoltV3ChainlinkSecrets {
        crate::bolt_v3_providers::chainlink::ResolvedBoltV3ChainlinkSecrets {
            api_key: zeroize::Zeroizing::new("fixture-chainlink-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new("fixture-chainlink-api-secret".to_string()),
        }
    }

    fn fixture_chainlink_reference_secrets()
    -> crate::bolt_v3_providers::chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets {
        crate::bolt_v3_providers::chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets {
            api_key: zeroize::Zeroizing::new("fixture-chainlink-reference-api-key".to_string()),
            api_secret: zeroize::Zeroizing::new(
                "fixture-chainlink-reference-api-secret".to_string(),
            ),
        }
    }

    fn fixture_polyresearch_secrets() -> ResolvedBoltV3PolyResearchSecrets {
        ResolvedBoltV3PolyResearchSecrets {
            api_key: zeroize::Zeroizing::new("fixture-polyresearch-api-key".to_string()),
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
        clients.insert(
            "chainlink_strike".to_string(),
            Arc::new(fixture_chainlink_strike_secrets()),
        );
        clients.insert(
            "chainlink_reference".to_string(),
            Arc::new(fixture_chainlink_reference_secrets()),
        );
        clients.insert(
            "polyresearch_reference".to_string(),
            Arc::new(fixture_polyresearch_secrets()),
        );
        ResolvedBoltV3Secrets { clients }
    }

    #[test]
    fn injected_provider_binding_can_accept_updown_target_without_core_provider_edit() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_key, _client| client_key == "polymarket_main");
        let mut plan = MarketIdentityPlan::empty();
        plan.push_target(UpdownTargetPlan {
            strategy_instance_id: "fake-strategy".to_string(),
            configured_target_id: "fake-updown".to_string(),
            execution_client_id: "polymarket_main".to_string(),
            underlying_asset: "ASSET".to_string(),
            cadence_secs: 300,
            cadence_slug_token: "window".to_string(),
        });
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let configs = map_bolt_v3_adapters_with_market_identity_and_provider_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            ProviderRuntimeApprovals::none(),
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
    fn polymarket_binding_accepts_static_binary_event_target() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        let mut plan = MarketIdentityPlan::empty();
        plan.push_target(StaticBinaryEventTargetPlan {
            strategy_instance_id: "sample-static-event".to_string(),
            configured_target_id: "sample-static-event-target".to_string(),
            execution_client_id: "polymarket_main".to_string(),
            event_key: "sample_event_2026".to_string(),
            market_slug: "will-sample-event-resolve-yes".to_string(),
            condition_id: Some("condition-sample-event".to_string()),
            yes_outcome: "Yes".to_string(),
            no_outcome: "No".to_string(),
        });

        let configs = map_bolt_v3_adapters_with_market_identity(
            &loaded,
            &resolved,
            &plan,
            Arc::new(|| 1_746_000_000),
        )
        .expect("polymarket provider binding should support static_binary_event targets");

        let data = configs
            .clients
            .get("polymarket_main")
            .expect("polymarket_main must map")
            .data
            .as_ref()
            .expect("polymarket data config must map")
            .config_as::<PolymarketDataClientConfig>()
            .expect("polymarket data config should downcast to NT config");
        let slugs = data
            .filters
            .iter()
            .flat_map(|filter| filter.market_slugs().unwrap_or_default())
            .collect::<Vec<_>>();

        assert_eq!(slugs, vec!["will-sample-event-resolve-yes".to_string()]);
        assert!(
            polymarket::SUPPORTED_MARKET_FAMILIES.contains(&static_binary_event::KEY),
            "provider allowlist must keep the adapter mapping path reachable"
        );
    }

    #[test]
    fn injected_provider_binding_without_family_support_rejects_before_provider_mapping() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_key, _client| client_key == "polymarket_main");
        let mut plan = MarketIdentityPlan::empty();
        plan.push_target(UpdownTargetPlan {
            strategy_instance_id: "fake-strategy".to_string(),
            configured_target_id: "fake-updown".to_string(),
            execution_client_id: "polymarket_main".to_string(),
            underlying_asset: "ASSET".to_string(),
            cadence_secs: 300,
            cadence_slug_token: "window".to_string(),
        });
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let error = map_bolt_v3_adapters_with_market_identity_and_provider_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            ProviderRuntimeApprovals::none(),
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
            BoltV3AdapterMappingError::ValidationInvariant {
                client_key,
                field,
                message,
            } => {
                assert_eq!(client_key, "polymarket_main");
                assert_eq!(field, "strategy.execution_client_id");
                assert!(message.contains("does not support that market family"));
                let rendered = format!(
                    "{}",
                    BoltV3AdapterMappingError::ValidationInvariant {
                        client_key,
                        field,
                        message,
                    }
                );
                assert!(rendered.starts_with("clients.polymarket_main:"));
                assert!(rendered.contains("strategy.execution_client_id"));
                assert!(!rendered.contains("provider venue"));
                assert!(!rendered.contains("strategy.target."));
            }
            other => panic!("expected ValidationInvariant, got {other}"),
        }
    }

    #[test]
    fn provider_without_family_support_can_map_when_no_target_references_client() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("venue = \"POLYMARKET\"", "venue = \"FAKE_UPDOWN_PROVIDER\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .clients
            .retain(|client_key, _client| client_key == "polymarket_main");
        let plan = MarketIdentityPlan::empty();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let configs = map_bolt_v3_adapters_with_market_identity_and_provider_lookup(
            &loaded,
            &resolved,
            &plan,
            clock,
            ProviderRuntimeApprovals::none(),
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
    fn maps_polymarket_client_data_and_execution_blocks_from_fixture() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

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
        assert_eq!(data.update_instruments_interval_mins, Some(1));
        assert!(!data.subscribe_new_markets);
        assert_eq!(
            data.base_url_rtds.as_deref(),
            Some("wss://ws-live-data.polymarket.com")
        );
        assert_eq!(data.new_market_fetch_max_concurrency, 8);
        assert!(!data.resolve_poll_enabled);
        assert_eq!(data.resolve_poll_interval_secs, 30);
        assert_eq!(data.resolve_poll_grace_secs, 10);
        assert_eq!(data.resolve_poll_max_wait_secs, 1800);
        assert_eq!(data.filters.len(), 1);
        assert_eq!(
            data.filters[0]
                .market_slugs()
                .expect("production mapper must install an active updown slug filter")
                .len(),
            2
        );
        assert!(data.new_market_filter.is_none());

        let exec = polymarket
            .execution
            .as_ref()
            .expect("polymarket [execution] block must map")
            .config_as::<PolymarketExecClientConfig>()
            .expect("polymarket execution config should downcast to NT config");
        let expected_execution: polymarket::PolymarketExecutionConfig = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture Polymarket client should exist")
            .execution
            .clone()
            .expect("fixture Polymarket execution block should exist")
            .try_into()
            .expect("fixture Polymarket execution block should parse");
        assert_eq!(exec.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(exec.account_id, AccountId::from("POLYMARKET-001"));
        assert_eq!(
            exec.private_key.as_deref(),
            Some("fixture-poly-private-key")
        );
        assert_eq!(exec.api_key.as_deref(), Some("fixture-poly-api-key"));
        assert_eq!(exec.api_secret.as_deref(), Some("fixture-poly-api-secret"));
        assert_eq!(exec.passphrase.as_deref(), Some("fixture-poly-passphrase"));
        assert!(
            exec.funder.as_deref() == expected_execution.funder.as_deref(),
            "mapped Polymarket funder must match the fixture funder"
        );
        assert_eq!(
            exec.signature_type,
            nt_polymarket_signature_type(expected_execution.signature_type)
        );
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
    fn maps_binance_client_data_block_from_fixture() {
        let loaded = fixture_loaded_config_with_binance_reference();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

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

        assert_eq!(data.product_type, NtBinanceProductType::Spot);
        assert_eq!(data.environment, NtBinanceEnvironment::Live);
        assert_eq!(data.spot_market_data_mode, NtBinanceSpotMarketDataMode::Sbe);
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
            Some("wss://stream-sbe.binance.com/ws")
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
        // Provide secrets for every other secret-requiring client so the mapper
        // reaches `polymarket_main` and fails specifically there. The fixture
        // also ships reference Chainlink clients (alphabetically before
        // polymarket_main in the BTreeMap iteration), so their secrets must be
        // present or the error would otherwise surface for chainlink first.
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "chainlink_reference".to_string(),
            Arc::new(fixture_chainlink_reference_secrets()),
        );
        clients.insert(
            "chainlink_strike".to_string(),
            Arc::new(fixture_chainlink_strike_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("missing resolved secrets must surface as a mapper error");
        let rendered = error.to_string();
        assert!(rendered.contains("(provider=POLYMARKET)"));
        assert!(!rendered.contains("(kind="));
        assert!(!rendered.contains("(venue="));
        match error {
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                client_key,
                expected_provider_key,
            } => {
                assert_eq!(client_key, "polymarket_main");
                assert_eq!(expected_provider_key, polymarket::KEY);
            }
            other => panic!("expected MissingResolvedSecrets, got {other}"),
        }
    }

    #[test]
    fn missing_resolved_secrets_for_binance_data_is_a_mapping_error() {
        let loaded = fixture_loaded_config_with_binance_reference();
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

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("missing binance resolved secrets must surface as a mapper error");
        let rendered = error.to_string();
        assert!(rendered.contains("(provider=BINANCE)"));
        assert!(!rendered.contains("(kind="));
        assert!(!rendered.contains("(venue="));
        match error {
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                client_key,
                expected_provider_key,
            } => {
                assert_eq!(client_key, "binance_reference");
                assert_eq!(expected_provider_key, binance::KEY);
            }
            other => panic!("expected MissingResolvedSecrets, got {other}"),
        }
    }

    #[test]
    fn mismatched_resolved_secret_provider_is_a_mapping_error() {
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
        // The fixture ships reference Chainlink clients before polymarket_main;
        // supply their matching secrets so the mapper reaches polymarket_main
        // and surfaces the provider MISMATCH there rather than a chainlink miss.
        clients.insert(
            "chainlink_reference".to_string(),
            Arc::new(fixture_chainlink_reference_secrets()),
        );
        clients.insert(
            "chainlink_strike".to_string(),
            Arc::new(fixture_chainlink_strike_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("mismatched resolved secret provider must surface as a mapper error");
        let rendered = error.to_string();
        assert!(rendered.contains("configured client provider"));
        match error {
            BoltV3AdapterMappingError::SecretProviderMismatch {
                client_key,
                expected_provider_key,
            } => {
                assert_eq!(client_key, "polymarket_main");
                assert_eq!(expected_provider_key, polymarket::KEY);
            }
            other => panic!("expected SecretProviderMismatch, got {other}"),
        }
    }

    #[test]
    fn binance_adapter_debug_redacts_resolved_api_credentials() {
        let loaded = fixture_loaded_config_with_binance_reference();
        let resolved = fixture_resolved_secrets();
        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map");
        let debug = format!("{configs:?}");

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
        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map");
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
