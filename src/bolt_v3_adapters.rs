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
    bolt_v3_instrument_filters::{InstrumentFilterConfig, InstrumentFilterError},
    bolt_v3_market_families::instrument_filters_from_config,
    bolt_v3_providers::{self, ProviderAdapterMapContext},
    bolt_v3_secrets::ResolvedBoltV3Secrets,
};

/// Boxed closure used by the provider-binding layer to obtain the
/// current unix-seconds value at the moment a provider filter wants
/// fresh slugs. The closure is invoked from inside the provider's
/// `load_all` cycle on every refresh, so it must be `Send + Sync` and
/// own all state it captures. Production mapping injects one backed by
/// an NT runtime clock; tests inject a fixed-time closure.
pub type BoltV3InstrumentFilterClockFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Provider-owned NT data-client factory and config for one configured
/// Bolt-v3 venue data block.
pub struct BoltV3DataClientAdapterConfig {
    pub factory: Box<dyn DataClientFactory>,
    pub config: Box<dyn ClientConfig>,
}

/// Provider-owned NT execution-client factory and config for one configured
/// Bolt-v3 venue execution block.
pub struct BoltV3ExecutionClientAdapterConfig {
    pub factory: Box<dyn ExecutionClientFactory>,
    pub config: Box<dyn ClientConfig>,
}

/// Mapped provider-owned adapter assemblies for one configured Bolt-v3
/// venue. Sub-configs are present iff the corresponding
/// `[venues.<id>.<block>]` section is present in the validated config.
pub struct BoltV3VenueAdapterConfig {
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

/// Mapped NT-native adapter configs keyed by the bolt-v3 venue
/// identifier (the TOML `[venues.<id>]` table key).
pub struct BoltV3AdapterConfigs {
    pub venues: BTreeMap<String, BoltV3VenueAdapterConfig>,
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

impl fmt::Debug for BoltV3VenueAdapterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3VenueAdapterConfig")
            .field("data", &self.data)
            .field("execution", &self.execution)
            .finish()
    }
}

impl fmt::Debug for BoltV3AdapterConfigs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoltV3AdapterConfigs")
            .field("venues", &self.venues)
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3AdapterMappingError {
    InstrumentFilter(InstrumentFilterError),
    /// The validated venue kind and the resolved secret kind disagree.
    /// Indicates an internal-consistency bug between the resolver output
    /// and the mapper inputs.
    SecretKindMismatch {
        venue_key: String,
        expected_provider_key: &'static str,
    },
    /// A venue requires resolved secrets but none were found in the
    /// passed-in `ResolvedBoltV3Secrets`. Validation guarantees a
    /// `[secrets]` block exists, so reaching this branch indicates the
    /// resolved-secrets value was constructed inconsistently with the
    /// loaded config.
    MissingResolvedSecrets {
        venue_key: String,
        expected_provider_key: &'static str,
    },
    /// A `[data]` or `[execution]` block existed but failed to
    /// deserialize into the corresponding NT-native shape. The validator
    /// runs the same `try_into` calls before the mapper, so reaching
    /// this branch means the inputs were mutated between validation and
    /// mapping.
    SchemaParse {
        venue_key: String,
        block: &'static str,
        message: String,
    },
    /// A bolt-v3 numeric config value did not fit the NT-native field
    /// type on this target (e.g. `u64 -> usize` overflow on a 32-bit
    /// build). No silent truncation: the mapper refuses to default.
    NumericRange {
        venue_key: String,
        field: &'static str,
        message: String,
    },
    /// The caller passed a config value that validated bolt-v3 startup
    /// must reject before mapping to NT. Keeping this guard at the
    /// mapper boundary prevents programmatic callers from bypassing
    /// root validation and reaching a hidden NT runtime behavior.
    ValidationInvariant {
        venue_key: String,
        field: &'static str,
        message: String,
    },
}

impl std::fmt::Display for BoltV3AdapterMappingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3AdapterMappingError::InstrumentFilter(error) => {
                write!(f, "bolt-v3 instrument filter config failed: {error}")
            }
            BoltV3AdapterMappingError::SecretKindMismatch {
                venue_key,
                expected_provider_key,
            } => write!(
                f,
                "venues.{venue_key}: resolved secret kind does not match validated venue kind \
                 (expected {kind})",
                kind = expected_provider_key,
            ),
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                venue_key,
                expected_provider_key,
            } => write!(
                f,
                "venues.{venue_key} (kind={kind}) requires resolved SSM secrets but none were \
                 supplied to the adapter mapper",
                kind = expected_provider_key,
            ),
            BoltV3AdapterMappingError::SchemaParse {
                venue_key,
                block,
                message,
            } => write!(
                f,
                "venues.{venue_key}.{block}: failed to deserialize into NT-native config: {message}",
            ),
            BoltV3AdapterMappingError::NumericRange {
                venue_key,
                field,
                message,
            } => write!(
                f,
                "venues.{venue_key}.{field}: bolt-v3 value does not fit the NT-native field type: {message}",
            ),
            BoltV3AdapterMappingError::ValidationInvariant {
                venue_key,
                field,
                message,
            } => write!(
                f,
                "venues.{venue_key}.{field}: bolt-v3 validation invariant failed at adapter mapping: {message}",
            ),
        }
    }
}

impl std::error::Error for BoltV3AdapterMappingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3AdapterMappingError::InstrumentFilter(error) => Some(error),
            BoltV3AdapterMappingError::SecretKindMismatch { .. }
            | BoltV3AdapterMappingError::MissingResolvedSecrets { .. }
            | BoltV3AdapterMappingError::SchemaParse { .. }
            | BoltV3AdapterMappingError::NumericRange { .. }
            | BoltV3AdapterMappingError::ValidationInvariant { .. } => None,
        }
    }
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// NT-native adapter config values, one per configured venue. The mapper
/// never re-resolves SSM and never registers clients; callers receive
/// owned config structs and may pass them to NT factories at a later
/// stage.
///
/// This entry point derives [`InstrumentFilterConfig`] from the loaded
/// strategy TOML and gives provider bindings an NT
/// [`LiveClock`]-backed timestamp source for filter projection.
pub fn map_bolt_v3_adapters(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    let instrument_filters = instrument_filters_from_config(loaded)
        .map_err(BoltV3AdapterMappingError::InstrumentFilter)?;
    map_bolt_v3_adapters_with_instrument_filters(
        loaded,
        resolved,
        &instrument_filters,
        nt_live_clock(),
    )
}

fn nt_live_clock() -> BoltV3InstrumentFilterClockFn {
    let clock = LiveClock::new(try_get_time_event_sender());
    Arc::new(move || {
        let now_unix_seconds = clock.timestamp_ns().as_u64() / NANOSECONDS_IN_SECOND;
        now_unix_seconds.min(i64::MAX as u64) as i64
    })
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// provider-owned NT client factory/config assemblies, then lets each
/// provider binding install provider-specific NT instrument filters from
/// [`InstrumentFilterConfig`].
pub fn map_bolt_v3_adapters_with_instrument_filters(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    instrument_filters: &InstrumentFilterConfig,
    clock: BoltV3InstrumentFilterClockFn,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    map_bolt_v3_adapters_with_instrument_filters_and_provider_lookup(
        loaded,
        resolved,
        instrument_filters,
        Some(clock),
        bolt_v3_providers::binding_for_provider_key,
    )
}

fn map_bolt_v3_adapters_with_instrument_filters_and_provider_lookup(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    instrument_filters: &InstrumentFilterConfig,
    clock: Option<BoltV3InstrumentFilterClockFn>,
    binding_for_provider_key: impl Fn(&str) -> Option<&'static bolt_v3_providers::ProviderBinding>,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    validate_instrument_filters_target_venues(loaded, instrument_filters)?;
    let mut venues = BTreeMap::new();
    for (venue_key, venue) in &loaded.root.venues {
        let Some(binding) = binding_for_provider_key(venue.kind.as_str()) else {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: venue_key.clone(),
                field: "kind",
                message: format!(
                    "provider key `{}` is not supported by this build",
                    venue.kind.as_str()
                ),
            });
        };
        validate_provider_market_family_support(venue_key, binding, instrument_filters)?;
        let mapped = (binding.map_adapters)(ProviderAdapterMapContext {
            loaded,
            root: &loaded.root,
            venue_key,
            venue,
            resolved,
            instrument_filters,
            clock: clock.clone(),
        })?;
        venues.insert(venue_key.clone(), mapped);
    }
    Ok(BoltV3AdapterConfigs { venues })
}

fn validate_provider_market_family_support(
    venue_key: &str,
    binding: &bolt_v3_providers::ProviderBinding,
    instrument_filters: &InstrumentFilterConfig,
) -> Result<(), BoltV3AdapterMappingError> {
    // Only venues referenced by an instrument-filter target need family
    // support. A provider with an empty `supported_market_families`
    // remains valid for data-only/reference venues that no strategy
    // target routes through.
    for target in instrument_filters
        .target_refs()
        .filter(|target| target.venue == venue_key)
    {
        if !binding
            .supported_market_families
            .contains(&target.family_key)
        {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: target.venue.to_string(),
                field: "strategy.venue",
                message: format!(
                    "configured target `{}` uses market family `{}` on venue `{}`, but provider kind `{}` does not support that market family",
                    target.configured_target_id, target.family_key, target.venue, binding.key,
                ),
            });
        }
    }
    Ok(())
}

fn validate_instrument_filters_target_venues(
    loaded: &LoadedBoltV3Config,
    instrument_filters: &InstrumentFilterConfig,
) -> Result<(), BoltV3AdapterMappingError> {
    for target in instrument_filters.target_refs() {
        if !loaded.root.venues.contains_key(target.venue) {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                venue_key: target.venue.to_string(),
                field: "strategy.venue",
                message: format!(
                    "configured target `{}` references unknown venue `{}`",
                    target.configured_target_id, target.venue,
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
    use crate::bolt_v3_instrument_filters::{InstrumentFilterConfig, InstrumentFilterTarget};
    use crate::bolt_v3_market_families::updown;
    use crate::bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderBinding, ProviderResolvedSecrets,
        ProviderSecretResolveContext, ResolvedVenueSecrets, SsmSecretResolver,
        binance::{self, ResolvedBoltV3BinanceSecrets},
        polymarket::{self, ResolvedBoltV3PolymarketSecrets},
    };
    use crate::bolt_v3_secrets::{
        BoltV3SecretError, ResolvedBoltV3Secrets, ResolvedBoltV3VenueSecrets,
    };

    const FAKE_UPDOWN_PROVIDER_KEY: &str = "fake_updown_provider";

    #[derive(Debug)]
    struct FakeProviderSecrets;

    impl ProviderResolvedSecrets for FakeProviderSecrets {
        fn provider_key(&self) -> &'static str {
            FAKE_UPDOWN_PROVIDER_KEY
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn validate_fake_provider_venue(
        _key: &str,
        _venue: &crate::bolt_v3_config::VenueBlock,
    ) -> Vec<String> {
        Vec::new()
    }

    fn resolve_fake_provider_secrets(
        _context: ProviderSecretResolveContext<'_>,
        _resolver: &mut dyn SsmSecretResolver,
    ) -> Result<ResolvedVenueSecrets, BoltV3SecretError> {
        Ok(Arc::new(FakeProviderSecrets))
    }

    fn map_fake_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
        assert_eq!(context.venue.kind.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.venue_key, "polymarket_main");
        let targets: Vec<_> = context.instrument_filters.target_refs().collect();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].venue, context.venue_key,);
        assert!(
            context.clock.is_some(),
            "targeted instrument-filter mapping must carry a real clock"
        );
        Ok(BoltV3VenueAdapterConfig {
            data: None,
            execution: None,
        })
    }

    fn map_fake_no_target_provider_adapters(
        context: ProviderAdapterMapContext<'_>,
    ) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
        assert_eq!(context.venue.kind.as_str(), FAKE_UPDOWN_PROVIDER_KEY);
        assert_eq!(context.venue_key, "polymarket_main");
        assert_eq!(context.instrument_filters.target_refs().count(), 0);
        assert!(
            context.clock.is_none(),
            "no-target adapter mapping should not carry a clock sentinel"
        );
        Ok(BoltV3VenueAdapterConfig {
            data: None,
            execution: None,
        })
    }

    static FAKE_UPDOWN_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_venue: validate_fake_provider_venue,
        supported_market_families: &[updown::KEY],
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_provider_adapters,
        build_fee_provider: None,
    };

    static FAKE_UNSUPPORTED_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_venue: validate_fake_provider_venue,
        supported_market_families: &[],
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_provider_adapters,
        build_fee_provider: None,
    };

    static FAKE_UNSUPPORTED_NO_TARGET_PROVIDER_BINDING: ProviderBinding = ProviderBinding {
        key: FAKE_UPDOWN_PROVIDER_KEY,
        validate_venue: validate_fake_provider_venue,
        supported_market_families: &[],
        required_secret_blocks: &[],
        secret_field_names: &[],
        credential_log_modules: &[],
        forbidden_env_vars: &[],
        resolve_secrets: resolve_fake_provider_secrets,
        map_adapters: map_fake_no_target_provider_adapters,
        build_fee_provider: None,
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

    fn fixture_polymarket_data_config(
        loaded: &LoadedBoltV3Config,
    ) -> polymarket::PolymarketDataConfig {
        loaded.root.venues["polymarket_main"]
            .data
            .clone()
            .expect("fixture polymarket venue should define [data]")
            .try_into()
            .expect("fixture polymarket data block should parse")
    }

    fn fixture_polymarket_execution_config(
        loaded: &LoadedBoltV3Config,
    ) -> polymarket::PolymarketExecutionConfig {
        loaded.root.venues["polymarket_main"]
            .execution
            .clone()
            .expect("fixture polymarket venue should define [execution]")
            .try_into()
            .expect("fixture polymarket execution block should parse")
    }

    fn fixture_binance_data_config(loaded: &LoadedBoltV3Config) -> binance::BinanceDataConfig {
        loaded.root.venues["binance_reference"]
            .data
            .clone()
            .expect("fixture binance venue should define [data]")
            .try_into()
            .expect("fixture binance data block should parse")
    }

    fn nt_polymarket_signature_type(
        signature_type: polymarket::PolymarketSignatureType,
    ) -> NtPolymarketSignatureType {
        match signature_type {
            polymarket::PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
            polymarket::PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
            polymarket::PolymarketSignatureType::PolyGnosisSafe => {
                NtPolymarketSignatureType::PolyGnosisSafe
            }
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
        let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_polymarket_secrets()),
        );
        venues.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        ResolvedBoltV3Secrets { venues }
    }

    #[test]
    fn injected_provider_binding_can_accept_updown_target_without_core_provider_edit() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("kind = \"polymarket\"", "kind = \"fake_updown_provider\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .venues
            .retain(|venue_key, _venue| venue_key == "polymarket_main");
        let instrument_filters = InstrumentFilterConfig::new(vec![InstrumentFilterTarget {
            strategy_instance_id: "fake-strategy".to_string(),
            family_key: updown::KEY,
            configured_target_id: "fake-updown".to_string(),
            venue: "polymarket_main".to_string(),
            underlying_asset: "btc".to_string(),
            cadence_seconds: 900,
            cadence_slug_token: "15m".to_string(),
        }]);
        let resolved = ResolvedBoltV3Secrets {
            venues: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let configs = map_bolt_v3_adapters_with_instrument_filters_and_provider_lookup(
            &loaded,
            &resolved,
            &instrument_filters,
            Some(clock),
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
            .venues
            .get("polymarket_main")
            .expect("fake provider venue should map");
        assert!(fake.data.is_none());
        assert!(fake.execution.is_none());
    }

    #[test]
    fn injected_provider_binding_without_family_support_rejects_before_provider_mapping() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("kind = \"polymarket\"", "kind = \"fake_updown_provider\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .venues
            .retain(|venue_key, _venue| venue_key == "polymarket_main");
        let instrument_filters = InstrumentFilterConfig::new(vec![InstrumentFilterTarget {
            strategy_instance_id: "fake-strategy".to_string(),
            family_key: updown::KEY,
            configured_target_id: "fake-updown".to_string(),
            venue: "polymarket_main".to_string(),
            underlying_asset: "btc".to_string(),
            cadence_seconds: 900,
            cadence_slug_token: "15m".to_string(),
        }]);
        let resolved = ResolvedBoltV3Secrets {
            venues: BTreeMap::new(),
        };
        let clock = Arc::new(|| 601_i64);

        let error = map_bolt_v3_adapters_with_instrument_filters_and_provider_lookup(
            &loaded,
            &resolved,
            &instrument_filters,
            Some(clock),
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
                venue_key,
                field,
                message,
            } => {
                assert_eq!(venue_key, "polymarket_main");
                assert_eq!(field, "strategy.venue");
                assert!(message.contains("does not support that market family"));
            }
            other => panic!("expected ValidationInvariant, got {other}"),
        }
    }

    #[test]
    fn provider_without_family_support_can_map_when_no_target_references_venue() {
        let fake_root_text = include_str!("../tests/fixtures/bolt_v3/root.toml")
            .replace("kind = \"polymarket\"", "kind = \"fake_updown_provider\"");
        let mut loaded = LoadedBoltV3Config {
            root_path: PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            root: toml::from_str(&fake_root_text).expect("fake-provider root should parse"),
            strategies: Vec::new(),
        };
        loaded
            .root
            .venues
            .retain(|venue_key, _venue| venue_key == "polymarket_main");
        let instrument_filters = InstrumentFilterConfig::empty();
        let resolved = ResolvedBoltV3Secrets {
            venues: BTreeMap::new(),
        };

        let configs = map_bolt_v3_adapters_with_instrument_filters_and_provider_lookup(
            &loaded,
            &resolved,
            &instrument_filters,
            None,
            |key| {
                if key == FAKE_UPDOWN_PROVIDER_KEY {
                    Some(&FAKE_UNSUPPORTED_NO_TARGET_PROVIDER_BINDING)
                } else {
                    None
                }
            },
        )
        .expect(
            "family support check applies only to venues referenced by instrument filter targets",
        );

        assert!(configs.venues.contains_key("polymarket_main"));
    }

    #[test]
    fn maps_polymarket_venue_data_and_execution_blocks_from_fixture() {
        let loaded = fixture_loaded_config();
        let expected_data = fixture_polymarket_data_config(&loaded);
        let expected_exec = fixture_polymarket_execution_config(&loaded);
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

        let polymarket = configs
            .venues
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
            Some(expected_data.base_url_http.as_str())
        );
        assert_eq!(
            data.base_url_ws.as_deref(),
            Some(expected_data.base_url_ws.as_str())
        );
        assert_eq!(
            data.base_url_gamma.as_deref(),
            Some(expected_data.base_url_gamma.as_str())
        );
        assert_eq!(
            data.base_url_data_api.as_deref(),
            Some(expected_data.base_url_data_api.as_str())
        );
        assert_eq!(data.http_timeout_secs, expected_data.http_timeout_seconds);
        assert_eq!(data.ws_timeout_secs, expected_data.ws_timeout_seconds);
        let expected_ws_max_subscriptions: usize = expected_data
            .websocket_max_subscriptions_per_connection
            .try_into()
            .expect("fixture ws subscription cap should fit usize");
        assert_eq!(data.ws_max_subscriptions, expected_ws_max_subscriptions);
        assert_eq!(
            data.update_instruments_interval_mins,
            expected_data.update_instruments_interval_minutes
        );
        assert_eq!(
            data.subscribe_new_markets,
            expected_data.subscribe_new_markets
        );
        assert_eq!(data.transport_backend, expected_data.transport_backend);
        assert!(
            data.filters.is_empty(),
            "root-only adapter mapping has no configured strategy targets to project"
        );
        assert!(data.new_market_filter.is_none());

        let exec = polymarket
            .execution
            .as_ref()
            .expect("polymarket [execution] block must map")
            .config_as::<PolymarketExecClientConfig>()
            .expect("polymarket execution config should downcast to NT config");
        assert_eq!(exec.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(exec.account_id, AccountId::from(expected_exec.account_id));
        assert_eq!(
            exec.private_key.as_deref(),
            Some("fixture-poly-private-key")
        );
        assert_eq!(exec.api_key.as_deref(), Some("fixture-poly-api-key"));
        assert_eq!(exec.api_secret.as_deref(), Some("fixture-poly-api-secret"));
        assert_eq!(exec.passphrase.as_deref(), Some("fixture-poly-passphrase"));
        assert_eq!(
            exec.funder.as_deref(),
            expected_exec.funder_address.as_deref()
        );
        assert_eq!(
            exec.signature_type,
            nt_polymarket_signature_type(expected_exec.signature_type)
        );
        assert_eq!(
            exec.base_url_http.as_deref(),
            Some(expected_exec.base_url_http.as_str())
        );
        assert_eq!(
            exec.base_url_ws.as_deref(),
            Some(expected_exec.base_url_ws.as_str())
        );
        assert_eq!(
            exec.base_url_data_api.as_deref(),
            Some(expected_exec.base_url_data_api.as_str())
        );
        assert_eq!(exec.http_timeout_secs, expected_exec.http_timeout_seconds);
        let expected_max_retries: u32 = expected_exec
            .max_retries
            .try_into()
            .expect("fixture retry count should fit u32");
        assert_eq!(exec.max_retries, expected_max_retries);
        assert_eq!(
            exec.retry_delay_initial_ms,
            expected_exec.retry_delay_initial_milliseconds
        );
        assert_eq!(
            exec.retry_delay_max_ms,
            expected_exec.retry_delay_max_milliseconds
        );
        assert_eq!(exec.ack_timeout_secs, expected_exec.ack_timeout_seconds);
        assert_eq!(exec.transport_backend, expected_exec.transport_backend);
    }

    #[test]
    fn maps_binance_venue_data_block_from_fixture() {
        let loaded = fixture_loaded_config();
        let expected_data = fixture_binance_data_config(&loaded);
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

        let binance = configs
            .venues
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
            Some(expected_data.base_url_http.as_str())
        );
        assert_eq!(
            data.base_url_ws.as_deref(),
            Some(expected_data.base_url_ws.as_str())
        );
        assert_eq!(data.api_key.as_deref(), Some("fixture-binance-api-key"));
        assert_eq!(
            data.api_secret.as_deref(),
            Some("fixture-binance-api-secret")
        );
        assert_eq!(
            data.instrument_status_poll_secs,
            expected_data.instrument_status_poll_seconds
        );
        assert_eq!(data.transport_backend, expected_data.transport_backend);
    }

    #[test]
    fn missing_resolved_secrets_for_polymarket_execution_is_a_mapping_error() {
        let loaded = fixture_loaded_config();
        // Provide the binance_reference secret entry so map iteration
        // reaches `polymarket_main` (which is alphabetically later in
        // the BTreeMap) and trips on the missing polymarket secrets.
        let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
        venues.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { venues };

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("missing resolved secrets must surface as a mapper error");
        match error {
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                venue_key,
                expected_provider_key,
            } => {
                assert_eq!(venue_key, "polymarket_main");
                assert_eq!(expected_provider_key, polymarket::KEY);
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
        let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_polymarket_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { venues };

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("missing binance resolved secrets must surface as a mapper error");
        match error {
            BoltV3AdapterMappingError::MissingResolvedSecrets {
                venue_key,
                expected_provider_key,
            } => {
                assert_eq!(venue_key, "binance_reference");
                assert_eq!(expected_provider_key, binance::KEY);
            }
            other => panic!("expected MissingResolvedSecrets, got {other}"),
        }
    }

    #[test]
    fn mismatched_resolved_secret_kind_is_a_mapping_error() {
        let loaded = fixture_loaded_config();
        let mut venues: BTreeMap<String, ResolvedBoltV3VenueSecrets> = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        venues.insert(
            "binance_reference".to_string(),
            Arc::new(fixture_binance_secrets()),
        );
        let resolved = ResolvedBoltV3Secrets { venues };

        let error = map_bolt_v3_adapters(&loaded, &resolved)
            .expect_err("mismatched resolved secret kind must surface as a mapper error");
        match error {
            BoltV3AdapterMappingError::SecretKindMismatch {
                venue_key,
                expected_provider_key,
            } => {
                assert_eq!(venue_key, "polymarket_main");
                assert_eq!(expected_provider_key, polymarket::KEY);
            }
            other => panic!("expected SecretKindMismatch, got {other}"),
        }
    }

    #[test]
    fn binance_adapter_debug_redacts_resolved_api_credentials() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();
        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map");
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

    #[test]
    fn adapter_mapping_source_uses_nt_live_clock() {
        let source = include_str!("bolt_v3_adapters.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("production source should precede cfg(test) test module");
        let zero_i64 = format!("{}{}", "0", "_i64");

        assert!(
            !production.contains(&zero_i64),
            "adapter mapping must not install a code-owned clock sentinel"
        );
        assert!(
            production.contains("LiveClock"),
            "adapter mapping must use NT LiveClock for live timestamp projection"
        );
    }

    // The no-trade-boundary source-inspection check lives in the
    // `tests/bolt_v3_adapter_mapping.rs` integration test so the
    // forbidden-strings list is not part of this module's own source
    // (which would otherwise self-trip the assertion).
}
