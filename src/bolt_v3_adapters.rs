//! Adapter config mapping for Bolt-v3.
//!
//! Converts a validated [`LoadedBoltV3Config`] plus already-resolved SSM
//! secrets ([`ResolvedBoltV3Secrets`]) into NT-native adapter client
//! configuration values (`PolymarketDataClientConfig`,
//! `PolymarketExecClientConfig`, `BinanceDataClientConfig`).
//!
//! The mapper is intentionally a no-trade boundary: it produces config
//! struct values only and never registers clients, opens connections,
//! starts an event loop, selects markets, constructs orders, or enables
//! any submit path. Secrets travel only through the resolved-secrets
//! struct passed in by the caller; AWS Systems Manager is never touched
//! here.

use std::{collections::BTreeMap, sync::Arc};

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
    filters::{InstrumentFilter, MarketSlugFilter},
};

use crate::{
    bolt_v3_config::{BoltV3RootConfig, LoadedBoltV3Config, VenueBlock},
    bolt_v3_market_families::updown::{
        MarketIdentityPlan, UpdownTargetPlan, updown_market_slug, updown_period_pair,
    },
    bolt_v3_providers::{
        binance::{self, BinanceDataConfig, BinanceEnvironment, BinanceProductType},
        polymarket::{
            self, PolymarketDataConfig, PolymarketExecutionConfig, PolymarketSignatureType,
        },
    },
    bolt_v3_secrets::{
        ResolvedBoltV3BinanceSecrets, ResolvedBoltV3PolymarketSecrets, ResolvedBoltV3Secrets,
        ResolvedBoltV3VenueSecrets,
    },
};

/// Boxed closure used by the provider-binding layer to obtain the
/// current unix-seconds value at the moment a provider filter wants
/// fresh slugs. The closure is invoked from inside the provider's
/// `load_all` cycle on every refresh, so it must be `Send + Sync` and
/// own all state it captures. Tests inject a fixed-time closure;
/// future live wiring will inject one backed by an NT runtime clock.
pub type BoltV3UpdownNowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Mapped NT-native adapter configs for one configured Bolt-v3 venue.
/// Sub-configs are present iff the corresponding `[venues.<id>.<block>]`
/// section is present in the validated config.
#[derive(Clone, Debug)]
pub enum BoltV3VenueAdapterConfig {
    Polymarket(Box<BoltV3PolymarketAdapters>),
    Binance(BoltV3BinanceAdapters),
}

/// Polymarket NT-native adapter configs derived from a `[venues.<id>]`
/// block. NT's `PolymarketExecClientConfig` already redacts every secret
/// field in its `Debug` impl, so the bolt-v3 wrapper relies on that.
#[derive(Clone, Debug)]
pub struct BoltV3PolymarketAdapters {
    pub data: Option<PolymarketDataClientConfig>,
    pub execution: Option<PolymarketExecClientConfig>,
}

/// Binance NT-native adapter configs derived from a `[venues.<id>]`
/// block. NT's `BinanceDataClientConfig` derives `Debug` without
/// redacting `api_key` / `api_secret`, so the bolt-v3 wrapper's `Debug`
/// impl masks those fields explicitly.
#[derive(Clone)]
pub struct BoltV3BinanceAdapters {
    pub data: Option<BinanceDataClientConfig>,
}

/// Mapped NT-native adapter configs keyed by the bolt-v3 venue
/// identifier (the TOML `[venues.<id>]` table key).
#[derive(Clone, Debug)]
pub struct BoltV3AdapterConfigs {
    pub venues: BTreeMap<String, BoltV3VenueAdapterConfig>,
}

#[derive(Debug)]
pub enum BoltV3AdapterMappingError {
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

impl std::error::Error for BoltV3AdapterMappingError {}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// NT-native adapter config values, one per configured venue. The mapper
/// never re-resolves SSM and never registers clients; callers receive
/// owned config structs and may pass them to NT factories at a later
/// stage.
///
/// This entry point intentionally installs no provider filter and
/// passes an empty plan into the with-identity variant. Callers that
/// need the rotating-market filter surface MUST use
/// [`map_bolt_v3_adapters_with_market_identity`] directly with a
/// derived [`MarketIdentityPlan`] and a real clock — copying the
/// `Arc::new(|| 0_i64)` sentinel below into a non-empty-plan call site
/// would produce slugs anchored to unix-second 0 every cycle.
pub fn map_bolt_v3_adapters(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
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
    map_bolt_v3_adapters_with_market_identity(loaded, resolved, &empty_plan, zero_clock)
}

/// Map a validated [`LoadedBoltV3Config`] plus resolved SSM secrets into
/// NT-native adapter config values, and additionally install the
/// provider-specific filter surface that corresponds to the supplied
/// provider-neutral [`MarketIdentityPlan`].
///
/// Today this means: every Polymarket venue receives one
/// `MarketSlugFilter` per configured updown target whose
/// `venue_config_key` matches that venue, in the same sequence as the
/// underlying strategies. Each filter's slug closure re-evaluates the
/// injected `clock` on every NT `load_all` cycle so the rotating-market
/// slug pair rolls forward with cadence. The provider-specific filter
/// type is referenced only inside this module: the core
/// market-identity module remains provider-neutral.
pub fn map_bolt_v3_adapters_with_market_identity(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<BoltV3AdapterConfigs, BoltV3AdapterMappingError> {
    validate_updown_target_venue_bindings(&loaded.root.venues, plan)?;
    let mut venues = BTreeMap::new();
    for (venue_key, venue) in &loaded.root.venues {
        let mapped = match venue.kind.as_str() {
            polymarket::KEY => map_polymarket_venue(
                &loaded.root,
                venue_key,
                venue,
                resolved,
                plan,
                clock.clone(),
            )?,
            binance::KEY => map_binance_venue(venue_key, venue, resolved)?,
            _ => {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    venue_key: venue_key.clone(),
                    field: "kind",
                    message: format!(
                        "provider key `{}` is not supported by this build",
                        venue.kind.as_str()
                    ),
                });
            }
        };
        venues.insert(venue_key.clone(), mapped);
    }
    Ok(BoltV3AdapterConfigs { venues })
}

/// Reject the mapping if any configured updown target binds to a
/// venue that is missing from `[venues]` or is not a Polymarket venue.
/// Without this guard the binding layer silently drops the target,
/// because filter installation only runs on the Polymarket branch of
/// `map_bolt_v3_adapters_with_market_identity`. This is the first
/// line of defence; a future schema validator should also reject the
/// same misconfiguration at config-load time.
fn validate_updown_target_venue_bindings(
    venues: &BTreeMap<String, VenueBlock>,
    plan: &MarketIdentityPlan,
) -> Result<(), BoltV3AdapterMappingError> {
    for target in &plan.updown_targets {
        match venues.get(&target.venue_config_key) {
            None => {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    venue_key: target.venue_config_key.clone(),
                    field: "strategy.target.venue_config_key",
                    message: format!(
                        "configured target `{}` references unknown venue `{}`",
                        target.configured_target_id, target.venue_config_key,
                    ),
                });
            }
            Some(venue) if venue.kind.as_str() != polymarket::KEY => {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    venue_key: target.venue_config_key.clone(),
                    field: "strategy.target.venue_config_key",
                    message: format!(
                        "configured target `{}` is bound to venue `{}` of kind `{}`, but rotating-market filter installation requires a venue of kind `{}`",
                        target.configured_target_id,
                        target.venue_config_key,
                        venue.kind.as_str(),
                        polymarket::KEY,
                    ),
                });
            }
            Some(_) => {}
        }
    }
    Ok(())
}

fn map_polymarket_venue(
    root: &BoltV3RootConfig,
    venue_key: &str,
    venue: &VenueBlock,
    resolved: &ResolvedBoltV3Secrets,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &venue.data {
        Some(value) => Some(map_polymarket_data(venue_key, value, plan, clock)?),
        None => None,
    };
    let execution = match &venue.execution {
        Some(value) => {
            let secrets = polymarket_secrets_for(venue_key, resolved)?;
            Some(map_polymarket_execution(root, venue_key, value, secrets)?)
        }
        None => None,
    };
    Ok(BoltV3VenueAdapterConfig::Polymarket(Box::new(
        BoltV3PolymarketAdapters { data, execution },
    )))
}

fn map_binance_venue(
    venue_key: &str,
    venue: &VenueBlock,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3VenueAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &venue.data {
        Some(value) => {
            let secrets = binance_secrets_for(venue_key, resolved)?;
            Some(map_binance_data(venue_key, value, secrets)?)
        }
        None => None,
    };
    Ok(BoltV3VenueAdapterConfig::Binance(BoltV3BinanceAdapters {
        data,
    }))
}

fn map_polymarket_data(
    venue_key: &str,
    value: &toml::Value,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<PolymarketDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    if cfg.subscribe_new_markets {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            venue_key: venue_key.to_string(),
            field: "data.subscribe_new_markets",
            message: "must be false before mapping to NT because pinned NT subscribes to all Polymarket markets when this flag is true".to_string(),
        });
    }
    let ws_max_subscriptions = usize::try_from(cfg.websocket_max_subscriptions_per_connection)
        .map_err(|_| BoltV3AdapterMappingError::NumericRange {
            venue_key: venue_key.to_string(),
            field: "data.websocket_max_subscriptions_per_connection",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.websocket_max_subscriptions_per_connection
            ),
        })?;
    // Build filters AFTER the `subscribe_new_markets` invariant fires
    // above. Reordering would let a misconfigured caller observe a
    // built filter for a config the mapper is required to reject.
    let filters = build_polymarket_market_slug_filters_for_venue(plan, venue_key, clock);
    Ok(PolymarketDataClientConfig {
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_gamma: Some(cfg.base_url_gamma),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_seconds,
        ws_timeout_secs: cfg.ws_timeout_seconds,
        ws_max_subscriptions,
        update_instruments_interval_mins: cfg.update_instruments_interval_minutes,
        subscribe_new_markets: cfg.subscribe_new_markets,
        auto_load_missing_instruments: false,
        auto_load_debounce_ms: 100,
        transport_backend: Default::default(),
        filters,
        new_market_filter: None,
    })
}

/// Provider-binding helper: emit one provider filter per configured
/// updown target whose `venue_config_key` matches `venue_key`, in the
/// same sequence as the underlying strategies. The provider-specific
/// filter type (`MarketSlugFilter`) is built only here; the core
/// market-identity module never references it.
fn build_polymarket_market_slug_filters_for_venue(
    plan: &MarketIdentityPlan,
    venue_key: &str,
    clock: BoltV3UpdownNowFn,
) -> Vec<Arc<dyn InstrumentFilter>> {
    plan.updown_targets
        .iter()
        .filter(|target| target.venue_config_key == venue_key)
        .map(|target| build_polymarket_market_slug_filter(target, clock.clone()))
        .collect()
}

/// Build one `MarketSlugFilter` whose closure recomputes the
/// `[current, next]` slug pair from the injected clock on every
/// invocation, using the provider-neutral cadence and asset already
/// captured in the [`UpdownTargetPlan`]. Cadence is positive (validated
/// by the planner), so the only `Err` paths reachable inside the
/// closure are extreme clock values; those surface as an empty slug
/// list so the provider's `load_all` loop continues without panicking.
///
/// Empty-slug-list semantics are not "no restriction / pass all".
/// At the pinned NT rev (`crates/adapters/polymarket/src/providers.rs`,
/// `fetch_instruments`), the slug-fetch branch is explicitly gated:
///
/// ```text
/// if let Some(slugs) = filter.market_slugs() && !slugs.is_empty() { ... }
/// ```
///
/// `MarketSlugFilter::market_slugs` always returns `Some(_)` (it is
/// never `None`), so an `Err` from `updown_period_pair` returning
/// `Vec::new()` means the provider sees `Some(vec![])`, fails the
/// `!is_empty()` gate, and skips the slug-fetch for this cycle. The
/// strategy is therefore starved (no Polymarket instruments fetched
/// via this filter) rather than flooded (every Polymarket instrument
/// loaded). The `log::warn!` below is the operator-visible signal for
/// that starvation cycle.
fn build_polymarket_market_slug_filter(
    target: &UpdownTargetPlan,
    clock: BoltV3UpdownNowFn,
) -> Arc<dyn InstrumentFilter> {
    let asset = target.underlying_asset.clone();
    let token = target.cadence_slug_token.clone();
    let cadence = target.cadence_seconds;
    Arc::new(MarketSlugFilter::new(move || {
        let now = (clock)();
        match updown_period_pair(cadence, now) {
            Ok((current, next)) => vec![
                updown_market_slug(&asset, &token, current),
                updown_market_slug(&asset, &token, next),
            ],
            Err(error) => {
                log::warn!(
                    "bolt-v3 provider binding: skipping updown filter cycle (cadence={cadence}, now_unix_seconds={now}): {error}"
                );
                Vec::new()
            }
        }
    }))
}

fn map_polymarket_execution(
    root: &BoltV3RootConfig,
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
) -> Result<PolymarketExecClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                venue_key: venue_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    let max_retries =
        u32::try_from(cfg.max_retries).map_err(|_| BoltV3AdapterMappingError::NumericRange {
            venue_key: venue_key.to_string(),
            field: "execution.max_retries",
            message: format!(
                "value {} does not fit in u32 expected by NT",
                cfg.max_retries
            ),
        })?;
    Ok(PolymarketExecClientConfig {
        trader_id: TraderId::from(root.trader_id.as_str()),
        account_id: AccountId::from(cfg.account_id.as_str()),
        private_key: Some(secrets.private_key.clone()),
        api_key: Some(secrets.api_key.clone()),
        api_secret: Some(secrets.api_secret.clone()),
        passphrase: Some(secrets.passphrase.clone()),
        funder: cfg.funder_address,
        signature_type: nt_polymarket_signature_type(cfg.signature_type),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_seconds,
        max_retries,
        retry_delay_initial_ms: cfg.retry_delay_initial_milliseconds,
        retry_delay_max_ms: cfg.retry_delay_max_milliseconds,
        ack_timeout_secs: cfg.ack_timeout_seconds,
        transport_backend: Default::default(),
    })
}

fn map_binance_data(
    venue_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3BinanceSecrets,
) -> Result<BinanceDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: BinanceDataConfig = value.clone().try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            venue_key: venue_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    let product_types = cfg
        .product_types
        .into_iter()
        .map(nt_binance_product_type)
        .collect();
    Ok(BinanceDataClientConfig {
        product_types,
        environment: nt_binance_environment(cfg.environment),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        api_key: Some(secrets.api_key.clone()),
        api_secret: Some(secrets.api_secret.clone()),
        instrument_status_poll_secs: cfg.instrument_status_poll_seconds,
        transport_backend: Default::default(),
    })
}

fn polymarket_secrets_for<'a>(
    venue_key: &str,
    resolved: &'a ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3PolymarketSecrets, BoltV3AdapterMappingError> {
    match resolved.venues.get(venue_key) {
        Some(ResolvedBoltV3VenueSecrets::Polymarket(inner)) => Ok(inner),
        Some(ResolvedBoltV3VenueSecrets::Binance(_)) => {
            Err(BoltV3AdapterMappingError::SecretKindMismatch {
                venue_key: venue_key.to_string(),
                expected_provider_key: polymarket::KEY,
            })
        }
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            venue_key: venue_key.to_string(),
            expected_provider_key: polymarket::KEY,
        }),
    }
}

fn binance_secrets_for<'a>(
    venue_key: &str,
    resolved: &'a ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3BinanceSecrets, BoltV3AdapterMappingError> {
    match resolved.venues.get(venue_key) {
        Some(ResolvedBoltV3VenueSecrets::Binance(inner)) => Ok(inner),
        Some(ResolvedBoltV3VenueSecrets::Polymarket(_)) => {
            Err(BoltV3AdapterMappingError::SecretKindMismatch {
                venue_key: venue_key.to_string(),
                expected_provider_key: binance::KEY,
            })
        }
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            venue_key: venue_key.to_string(),
            expected_provider_key: binance::KEY,
        }),
    }
}

fn nt_polymarket_signature_type(value: PolymarketSignatureType) -> NtPolymarketSignatureType {
    match value {
        PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
        PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
        PolymarketSignatureType::PolyGnosisSafe => NtPolymarketSignatureType::PolyGnosisSafe,
    }
}

fn nt_binance_product_type(value: BinanceProductType) -> NtBinanceProductType {
    match value {
        BinanceProductType::Spot => NtBinanceProductType::Spot,
    }
}

fn nt_binance_environment(value: BinanceEnvironment) -> NtBinanceEnvironment {
    match value {
        BinanceEnvironment::Mainnet => NtBinanceEnvironment::Mainnet,
    }
}

impl std::fmt::Debug for BoltV3BinanceAdapters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let data = self.data.as_ref().map(BinanceDataClientConfigRedacted);
        f.debug_struct("BoltV3BinanceAdapters")
            .field("data", &data)
            .finish()
    }
}

struct BinanceDataClientConfigRedacted<'a>(&'a BinanceDataClientConfig);

impl std::fmt::Debug for BinanceDataClientConfigRedacted<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cfg = self.0;
        let instrument_status_poll_secs = &cfg.instrument_status_poll_secs;
        f.debug_struct("BinanceDataClientConfig")
            .field("product_types", &cfg.product_types)
            .field("environment", &cfg.environment)
            .field("base_url_http", &cfg.base_url_http)
            .field("base_url_ws", &cfg.base_url_ws)
            .field("api_key", &cfg.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("api_secret", &cfg.api_secret.as_ref().map(|_| "[REDACTED]"))
            .field("instrument_status_poll_secs", instrument_status_poll_secs)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::bolt_v3_secrets::{
        ResolvedBoltV3BinanceSecrets, ResolvedBoltV3PolymarketSecrets, ResolvedBoltV3Secrets,
        ResolvedBoltV3VenueSecrets,
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

    #[test]
    fn nt_polymarket_signature_type_translation_is_exhaustive() {
        assert_eq!(
            nt_polymarket_signature_type(PolymarketSignatureType::Eoa),
            NtPolymarketSignatureType::Eoa
        );
        assert_eq!(
            nt_polymarket_signature_type(PolymarketSignatureType::PolyProxy),
            NtPolymarketSignatureType::PolyProxy
        );
        assert_eq!(
            nt_polymarket_signature_type(PolymarketSignatureType::PolyGnosisSafe),
            NtPolymarketSignatureType::PolyGnosisSafe
        );
    }

    #[test]
    fn nt_binance_enum_translations_are_exhaustive() {
        assert_eq!(
            nt_binance_product_type(BinanceProductType::Spot),
            NtBinanceProductType::Spot
        );
        assert_eq!(
            nt_binance_environment(BinanceEnvironment::Mainnet),
            NtBinanceEnvironment::Mainnet
        );
    }

    #[test]
    fn maps_polymarket_venue_data_and_execution_blocks_from_fixture() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

        let polymarket = match configs
            .venues
            .get("polymarket_main")
            .expect("polymarket_main must be present")
        {
            BoltV3VenueAdapterConfig::Polymarket(inner) => inner,
            other => panic!("expected polymarket adapter config, got {other:?}"),
        };

        let data = polymarket
            .data
            .as_ref()
            .expect("polymarket [data] block must map");
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
            .expect("polymarket [execution] block must map");
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
    fn maps_binance_venue_data_block_from_fixture() {
        let loaded = fixture_loaded_config();
        let resolved = fixture_resolved_secrets();

        let configs = map_bolt_v3_adapters(&loaded, &resolved).expect("fixture should map cleanly");

        let binance = match configs
            .venues
            .get("binance_reference")
            .expect("binance_reference must be present")
        {
            BoltV3VenueAdapterConfig::Binance(inner) => inner,
            other => panic!("expected binance adapter config, got {other:?}"),
        };
        let data = binance
            .data
            .as_ref()
            .expect("binance [data] block must map");

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
        let mut venues = BTreeMap::new();
        venues.insert(
            "binance_reference".to_string(),
            ResolvedBoltV3VenueSecrets::Binance(fixture_binance_secrets()),
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
        let mut venues = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            ResolvedBoltV3VenueSecrets::Polymarket(fixture_polymarket_secrets()),
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
        let mut venues = BTreeMap::new();
        venues.insert(
            "polymarket_main".to_string(),
            ResolvedBoltV3VenueSecrets::Binance(fixture_binance_secrets()),
        );
        venues.insert(
            "binance_reference".to_string(),
            ResolvedBoltV3VenueSecrets::Binance(fixture_binance_secrets()),
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
        assert!(debug.contains("[REDACTED]"));
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
