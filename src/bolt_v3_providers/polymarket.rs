//! Per-provider binding for `polymarket` client config block
//! shapes and per-instance startup validation.
//!
//! Owns the concrete shape of `[clients.<name>.data]`,
//! `[clients.<name>.execution]`, and
//! `[clients.<name>.secrets]` for any client whose
//! `venue = "POLYMARKET"` key is configured. Core config in
//! `crate::bolt_v3_config` only owns the root/strategy envelope
//! and raw venue field; the provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does not
//! reach back into the envelope module.
//!
//! This module also owns the per-instance startup-validation policy for
//! Polymarket clients: typed deserialization of each present block,
//! cross-block presence rule ([secrets] is only allowed alongside
//! [execution]), Polymarket data/execution bounds, EVM funder-address
//! syntax, and Polymarket secret-path ownership. The cross-provider rule
//! that [execution] requires [secrets] is declared by
//! [`REQUIRED_SECRET_BLOCKS`] and enforced centrally in
//! `bolt_v3_providers::validate_client_id_block`. Core startup validation in
//! `crate::bolt_v3_validate`
//! dispatches into `bolt_v3_providers::validate_client_id_block`, which
//! routes Polymarket clients here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

use std::{any::Any, collections::BTreeMap, sync::Arc};

use nautilus_common::cache::Cache;
use nautilus_model::identifiers::{AccountId, TraderId};
use nautilus_model::{
    identifiers::Venue,
    instruments::{InstrumentAny, binary_option::BinaryOption},
};
use nautilus_polymarket::{
    common::credential::Secrets as PolymarketSecrets,
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{PolymarketDataClientConfig, PolymarketExecClientConfig},
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{InstrumentFilter, MarketSlugFilter},
    http::clob::PolymarketClobHttpClient,
};
use serde::Deserialize;

use crate::{
    bolt_v3_adapters::{
        BoltV3ClientConfig, BoltV3ClientMappingError, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig, BoltV3UpdownNowFn,
    },
    bolt_v3_config::ClientBlock,
    bolt_v3_market_families::updown::{
        self, BoltV3MarketIdentityError, MarketIdentityPlan, UpdownTargetPlan,
        candidates_for_target, updown_market_slug, updown_period_pair,
    },
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderInstrumentReadinessContext,
        ProviderInstrumentReadinessFact, ProviderInstrumentReadinessStatus,
        ProviderResolvedSecrets, ProviderSecretRequirement, ProviderSecretResolveContext,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    clients::polymarket::{FeeProvider, PolymarketClobFeeProvider},
    secrets::pad_base64,
};

pub const KEY: &str = "POLYMARKET";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketRole {
    Current,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketFailureReason {
    InstrumentsNotInCache,
    NoSelectedMarket,
    AmbiguousSelectedMarket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownSelectedMarket {
    pub market_selection_type: String,
    pub client_id: String,
    pub venue: String,
    pub rotating_market_family: String,
    pub polymarket_condition_id: String,
    pub polymarket_market_slug: String,
    pub polymarket_question_id: String,
    pub up_instrument_id: String,
    pub down_instrument_id: String,
    pub polymarket_market_start_timestamp_milliseconds: i64,
    pub polymarket_market_end_timestamp_milliseconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdownSelectedMarketResolution {
    Selected {
        role: UpdownSelectedMarketRole,
        selected_market: UpdownSelectedMarket,
    },
    Failed {
        failure_reason: UpdownSelectedMarketFailureReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdownTargetSelectedMarketResolution {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub resolution: UpdownSelectedMarketResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UpdownMarketKey {
    condition_id: String,
    market_slug: String,
    question_id: String,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Clone, Default)]
struct UpdownInstrumentPair {
    up_instrument_id: Option<String>,
    down_instrument_id: Option<String>,
}

pub fn resolve_updown_selected_market_from_cache(
    cache: &Cache,
    target: &UpdownTargetPlan,
    venue: &Venue,
    market_selection_timestamp_milliseconds: i64,
) -> Result<UpdownSelectedMarketResolution, BoltV3MarketIdentityError> {
    if market_selection_timestamp_milliseconds < 0 {
        return Err(BoltV3MarketIdentityError::NegativeNowUnixSeconds {
            now_unix_seconds: market_selection_timestamp_milliseconds.div_euclid(1_000),
        });
    }
    let candidates =
        candidates_for_target(target, market_selection_timestamp_milliseconds / 1_000)?;
    let current = complete_updown_markets_for_slug(
        cache,
        target,
        venue,
        &candidates.current_market_slug,
        |market| {
            market.polymarket_market_start_timestamp_milliseconds
                <= market_selection_timestamp_milliseconds
                && market_selection_timestamp_milliseconds
                    < market.polymarket_market_end_timestamp_milliseconds
        },
    );
    if current.len() > 1 {
        return Ok(UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket,
        });
    }
    if let Some(selected_market) = current.into_iter().next() {
        return Ok(UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Current,
            selected_market,
        });
    }

    let next = complete_updown_markets_for_slug(
        cache,
        target,
        venue,
        &candidates.next_market_slug,
        |market| {
            market.polymarket_market_start_timestamp_milliseconds
                > market_selection_timestamp_milliseconds
        },
    );
    if next.len() > 1 {
        return Ok(UpdownSelectedMarketResolution::Failed {
            failure_reason: UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket,
        });
    }
    if let Some(selected_market) = next.into_iter().next() {
        return Ok(UpdownSelectedMarketResolution::Selected {
            role: UpdownSelectedMarketRole::Next,
            selected_market,
        });
    }

    let has_candidate_instruments =
        cache_contains_slug(cache, venue, &candidates.current_market_slug)
            || cache_contains_slug(cache, venue, &candidates.next_market_slug);
    Ok(UpdownSelectedMarketResolution::Failed {
        failure_reason: if has_candidate_instruments {
            UpdownSelectedMarketFailureReason::NoSelectedMarket
        } else {
            UpdownSelectedMarketFailureReason::InstrumentsNotInCache
        },
    })
}

pub fn resolve_updown_selected_markets_for_client_from_cache(
    cache: &Cache,
    plan: &MarketIdentityPlan,
    client_id_key: &str,
    venue: &Venue,
    market_selection_timestamp_milliseconds: i64,
) -> Result<Vec<UpdownTargetSelectedMarketResolution>, BoltV3MarketIdentityError> {
    plan.updown_targets
        .iter()
        .filter(|target| target.client_id_key == client_id_key)
        .map(|target| {
            Ok(UpdownTargetSelectedMarketResolution {
                strategy_instance_id: target.strategy_instance_id.clone(),
                configured_target_id: target.configured_target_id.clone(),
                resolution: resolve_updown_selected_market_from_cache(
                    cache,
                    target,
                    venue,
                    market_selection_timestamp_milliseconds,
                )?,
            })
        })
        .collect()
}

pub fn check_instrument_readiness(
    context: ProviderInstrumentReadinessContext<'_>,
) -> Result<Vec<ProviderInstrumentReadinessFact>, BoltV3MarketIdentityError> {
    let venue = Venue::new(context.venue_key);
    resolve_updown_selected_markets_for_client_from_cache(
        context.cache,
        context.plan,
        context.client_id_key,
        &venue,
        context.market_selection_timestamp_milliseconds,
    )
    .map(|resolutions| {
        resolutions
            .into_iter()
            .map(|target| {
                let (status, detail) = match target.resolution {
                    UpdownSelectedMarketResolution::Selected {
                        role,
                        selected_market,
                    } => (
                        ProviderInstrumentReadinessStatus::Ready,
                        format!(
                            "selected_market role={} market_slug={} up_instrument_id={} down_instrument_id={}",
                            selected_market_role_as_str(role),
                            selected_market.polymarket_market_slug,
                            selected_market.up_instrument_id,
                            selected_market.down_instrument_id
                        ),
                    ),
                    UpdownSelectedMarketResolution::Failed { failure_reason } => (
                        ProviderInstrumentReadinessStatus::Blocked,
                        updown_selected_market_failure_reason_as_str(failure_reason).to_string(),
                    ),
                };
                ProviderInstrumentReadinessFact {
                    client_id_key: context.client_id_key.to_string(),
                    strategy_instance_id: target.strategy_instance_id,
                    configured_target_id: target.configured_target_id,
                    status,
                    detail,
                }
            })
            .collect()
    })
}

fn complete_updown_markets_for_slug(
    cache: &Cache,
    target: &UpdownTargetPlan,
    venue: &Venue,
    market_slug: &str,
    role_predicate: impl Fn(&UpdownSelectedMarket) -> bool,
) -> Vec<UpdownSelectedMarket> {
    let mut pairs = BTreeMap::<UpdownMarketKey, UpdownInstrumentPair>::new();
    for instrument in cache.instruments(venue, None) {
        let Some((key, outcome, instrument_id)) = updown_instrument_row(instrument, market_slug)
        else {
            continue;
        };
        let pair = pairs.entry(key).or_default();
        if outcome.eq_ignore_ascii_case("up") {
            pair.up_instrument_id = Some(instrument_id);
        } else if outcome.eq_ignore_ascii_case("down") {
            pair.down_instrument_id = Some(instrument_id);
        }
    }

    pairs
        .into_iter()
        .filter_map(|(key, pair)| {
            Some(UpdownSelectedMarket {
                market_selection_type: target.market_selection_type.clone(),
                client_id: target.client_id_key.clone(),
                venue: venue.as_str().to_string(),
                rotating_market_family: updown::KEY.to_string(),
                polymarket_condition_id: key.condition_id,
                polymarket_market_slug: key.market_slug,
                polymarket_question_id: key.question_id,
                up_instrument_id: pair.up_instrument_id?,
                down_instrument_id: pair.down_instrument_id?,
                polymarket_market_start_timestamp_milliseconds: key.start_ms,
                polymarket_market_end_timestamp_milliseconds: key.end_ms,
            })
        })
        .filter(role_predicate)
        .collect()
}

fn cache_contains_slug(cache: &Cache, venue: &Venue, market_slug: &str) -> bool {
    cache
        .instruments(venue, None)
        .into_iter()
        .any(|instrument| instrument_market_slug(instrument) == Some(market_slug))
}

fn updown_instrument_row(
    instrument: &InstrumentAny,
    expected_market_slug: &str,
) -> Option<(UpdownMarketKey, String, String)> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    let market_slug = instrument_info_str(binary, "market_slug")?;
    if market_slug != expected_market_slug {
        return None;
    }
    let key = UpdownMarketKey {
        condition_id: instrument_info_str(binary, "condition_id")?.to_string(),
        market_slug: market_slug.to_string(),
        question_id: instrument_info_str(binary, "question_id")?.to_string(),
        start_ms: unix_nanos_to_millis(binary.activation_ns)?,
        end_ms: unix_nanos_to_millis(binary.expiration_ns)?,
    };
    let outcome = binary.outcome.map(|value| value.to_string())?;
    Some((key, outcome, binary.id.to_string()))
}

fn instrument_market_slug(instrument: &InstrumentAny) -> Option<&str> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    instrument_info_str(binary, "market_slug")
}

fn instrument_info_str<'a>(binary: &'a BinaryOption, key: &str) -> Option<&'a str> {
    binary.info.as_ref()?.get_str(key)
}

fn unix_nanos_to_millis(value: nautilus_core::UnixNanos) -> Option<i64> {
    i64::try_from(value.as_u64() / 1_000_000).ok()
}

fn selected_market_role_as_str(role: UpdownSelectedMarketRole) -> &'static str {
    match role {
        UpdownSelectedMarketRole::Current => "current",
        UpdownSelectedMarketRole::Next => "next",
    }
}

fn updown_selected_market_failure_reason_as_str(
    reason: UpdownSelectedMarketFailureReason,
) -> &'static str {
    match reason {
        UpdownSelectedMarketFailureReason::InstrumentsNotInCache => "instruments_not_in_cache",
        UpdownSelectedMarketFailureReason::NoSelectedMarket => "no_selected_market",
        UpdownSelectedMarketFailureReason::AmbiguousSelectedMarket => "ambiguous_selected_market",
    }
}
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[updown::KEY];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Execution,
    consumer: "Polymarket execution client",
}];
pub const CREDENTIAL_LOG_MODULES: &[&str] = &["nautilus_polymarket::common::credential"];
pub const FORBIDDEN_ENV_VARS: &[&str] = &[
    "POLYMARKET_PK",
    "POLYMARKET_FUNDER",
    "POLYMARKET_API_KEY",
    "POLYMARKET_API_SECRET",
    "POLYMARKET_PASSPHRASE",
];

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketDataConfig {
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_gamma: String,
    pub base_url_data_api: String,
    pub http_timeout_seconds: u64,
    pub ws_timeout_seconds: u64,
    pub subscribe_new_markets: bool,
    pub update_instruments_interval_minutes: u64,
    pub websocket_max_subscriptions_per_connection: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketExecutionConfig {
    pub account_id: String,
    pub signature_type: PolymarketSignatureType,
    /// Public funder address. Required when `signature_type` is
    /// `poly_proxy` or `poly_gnosis_safe` (the proxy/safe routes the
    /// underlying funder wallet); permitted to be absent for `eoa`,
    /// where the EOA is itself the funder. Validation enforces this
    /// per-signature-type requirement and the EVM address syntax.
    #[serde(default)]
    pub funder_address: Option<String>,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_data_api: String,
    pub http_timeout_seconds: u64,
    pub max_retries: u64,
    pub retry_delay_initial_milliseconds: u64,
    pub retry_delay_max_milliseconds: u64,
    pub ack_timeout_seconds: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolymarketSignatureType {
    Eoa,
    PolyProxy,
    PolyGnosisSafe,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketSecretsConfig {
    pub private_key_ssm_path: String,
    pub api_key_ssm_path: String,
    pub api_secret_ssm_path: String,
    pub passphrase_ssm_path: String,
}

#[derive(Clone)]
pub struct ResolvedBoltV3PolymarketSecrets {
    pub private_key: String,
    pub api_key: String,
    pub api_secret: String,
    pub passphrase: String,
}

struct RedactedDebug;

impl std::fmt::Debug for RedactedDebug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl std::fmt::Debug for ResolvedBoltV3PolymarketSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted = RedactedDebug;
        f.debug_struct("ResolvedBoltV3PolymarketSecrets")
            .field("private_key", &redacted)
            .field("api_key", &redacted)
            .field("api_secret", &redacted)
            .field("passphrase", &redacted)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3PolymarketSecrets {
    fn venue_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn validate_client_id(key: &str, client_id: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(data) = &client_id.data {
        match data.clone().try_into::<PolymarketDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(execution) = &client_id.execution {
        match execution.clone().try_into::<PolymarketExecutionConfig>() {
            Ok(parsed) => {
                if parsed.account_id.trim().is_empty() {
                    errors.push(format!(
                        "clients.{key}.execution.account_id must be a non-empty string"
                    ));
                }
                errors.extend(validate_funder_address(key, &parsed));
                errors.extend(validate_execution_bounds(key, &parsed));
            }
            Err(message) => {
                errors.push(format!("clients.{key}.execution: {message}"));
            }
        }
    }
    if let Some(secrets) = &client_id.secrets {
        // Only Polymarket execution consumes Polymarket credentials in
        // this slice. A data-only Polymarket client with `[secrets]`
        // would carry credential paths that no adapter uses, which is a
        // misconfiguration rather than a silent no-op.
        if client_id.execution.is_none() {
            errors.push(format!(
                "clients.{key} (venue=POLYMARKET) declares [secrets] but no [execution] block is configured; \
                 Polymarket [secrets] are only allowed alongside the execution adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<PolymarketSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_funder_address(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let funder = execution
        .funder_address
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let requires_funder = matches!(
        execution.signature_type,
        PolymarketSignatureType::PolyProxy | PolymarketSignatureType::PolyGnosisSafe
    );
    match (requires_funder, funder) {
        (true, None) => errors.push(format!(
            "clients.{key}.execution.funder_address is required when signature_type is `poly_proxy` or `poly_gnosis_safe`"
        )),
        (_, Some(value)) => {
            if let Err(message) = check_evm_address_syntax(value) {
                errors.push(format!(
                    "clients.{key}.execution.funder_address is not a valid EVM public address ({message}): `{value}`"
                ));
            }
        }
        (false, None) => {}
    }
    errors
}

fn check_evm_address_syntax(value: &str) -> Result<(), &'static str> {
    let rest = value.strip_prefix("0x").ok_or("missing `0x` prefix")?;
    if rest.len() != 40 {
        return Err("must be 40 hex characters after `0x`");
    }
    if !rest.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("must contain only hex characters after `0x`");
    }
    if rest.chars().all(|c| c == '0') {
        return Err("zero address is not allowed");
    }
    Ok(())
}

fn validate_data_bounds(key: &str, data: &PolymarketDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", data.http_timeout_seconds),
        ("ws_timeout_seconds", data.ws_timeout_seconds),
        (
            "update_instruments_interval_minutes",
            data.update_instruments_interval_minutes,
        ),
        (
            "websocket_max_subscriptions_per_connection",
            data.websocket_max_subscriptions_per_connection,
        ),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "clients.{key}.data.{field} must be a positive integer"
            ));
        }
    }
    // The pinned NautilusTrader Polymarket data client (`nautilus_polymarket::data`)
    // calls `ws_client.subscribe_market(vec![])` from inside its `connect()`
    // implementation when `subscribe_new_markets = true`, which is effectively
    // an all-markets subscription and violates the bolt-v3 controlled-connect
    // boundary. The flag is forced false in the current bolt-v3 scope until
    // the market-subscription slice owns the controlled-subscribe path; failing
    // closed here keeps that invariant honest.
    if data.subscribe_new_markets {
        errors.push(format!(
            "clients.{key}.data.subscribe_new_markets must be false in the current bolt-v3 scope; \
             the pinned NT Polymarket data client subscribes to all markets via \
             `ws_client.subscribe_market(vec![])` during connect when this flag is true, \
             which violates the bolt-v3 controlled-connect boundary until the \
             market-subscription slice owns it"
        ));
    }
    errors
}

fn validate_execution_bounds(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_seconds", execution.http_timeout_seconds),
        ("max_retries", execution.max_retries),
        (
            "retry_delay_initial_milliseconds",
            execution.retry_delay_initial_milliseconds,
        ),
        (
            "retry_delay_max_milliseconds",
            execution.retry_delay_max_milliseconds,
        ),
        ("ack_timeout_seconds", execution.ack_timeout_seconds),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "clients.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.retry_delay_initial_milliseconds > execution.retry_delay_max_milliseconds {
        errors.push(format!(
            "clients.{key}.execution.retry_delay_initial_milliseconds ({}) must be <= retry_delay_max_milliseconds ({})",
            execution.retry_delay_initial_milliseconds, execution.retry_delay_max_milliseconds
        ));
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &PolymarketSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("private_key_ssm_path", &secrets.private_key_ssm_path),
        ("api_key_ssm_path", &secrets.api_key_ssm_path),
        ("api_secret_ssm_path", &secrets.api_secret_ssm_path),
        ("passphrase_ssm_path", &secrets.passphrase_ssm_path),
    ];
    for (field, value) in path_fields {
        errors.extend(crate::bolt_v3_validate::validate_ssm_parameter_path(
            key, field, value,
        ));
    }
    errors
}

pub fn resolve_secrets(
    context: ProviderSecretResolveContext<'_>,
    resolver: &mut dyn SsmSecretResolver,
) -> Result<ResolvedClientSecrets, BoltV3SecretError> {
    let secrets_value = context
        .client_id
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_id_key: context.client_id_key.to_string(),
            field: "secrets".to_string(),
            ssm_path: String::new(),
            source: "missing [secrets] block".to_string(),
        })?;
    let secrets: PolymarketSecretsConfig =
        secrets_value
            .clone()
            .try_into()
            .map_err(|error: toml::de::Error| BoltV3SecretError {
                client_id_key: context.client_id_key.to_string(),
                field: KEY.to_string(),
                ssm_path: String::new(),
                source: format!("invalid polymarket secrets schema: {error}"),
            })?;
    let private_key = resolve_field(
        context.client_id_key,
        "private_key_ssm_path",
        context.region,
        &secrets.private_key_ssm_path,
        resolver,
    )?;
    let api_key = resolve_field(
        context.client_id_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    let api_secret_raw = resolve_field(
        context.client_id_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    let api_secret = pad_base64(api_secret_raw);
    let passphrase = resolve_field(
        context.client_id_key,
        "passphrase_ssm_path",
        context.region,
        &secrets.passphrase_ssm_path,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3PolymarketSecrets {
        private_key,
        api_key,
        api_secret,
        passphrase,
    }))
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientConfig, BoltV3ClientMappingError> {
    let data = match &context.client_id.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(PolymarketDataClientFactory),
            config: Box::new(map_data(
                context.client_id_key,
                value,
                context.plan,
                context.clock,
            )?),
        }),
        None => None,
    };
    let execution = match &context.client_id.execution {
        Some(value) => {
            let secrets = secrets_for(context.client_id_key, context.resolved)?;
            Some(BoltV3ExecutionClientAdapterConfig {
                factory: Box::new(PolymarketExecutionClientFactory),
                config: Box::new(map_execution(
                    context.root,
                    context.client_id_key,
                    value,
                    secrets,
                )?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientConfig { data, execution })
}

pub fn build_fee_provider(
    execution: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
    timeout_seconds: u64,
) -> Result<Arc<dyn FeeProvider>, String> {
    let cfg: PolymarketExecutionConfig = execution
        .clone()
        .try_into()
        .map_err(|error: toml::de::Error| error.to_string())?;
    let secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.clone()),
        Some(secrets.api_secret.clone()),
        Some(secrets.passphrase.clone()),
        cfg.funder_address,
    )
    .map_err(|error| format!("failed to resolve Polymarket fee credentials: {error}"))?;
    let client = PolymarketClobHttpClient::new(
        secrets.credential,
        secrets.address,
        Some(cfg.base_url_http),
        timeout_seconds,
    )
    .map_err(|error| format!("failed to create Polymarket fee HTTP client: {error}"))?;
    Ok(Arc::new(PolymarketClobFeeProvider::new(client)))
}

fn map_data(
    client_id_key: &str,
    value: &toml::Value,
    plan: &MarketIdentityPlan,
    clock: BoltV3UpdownNowFn,
) -> Result<PolymarketDataClientConfig, BoltV3ClientMappingError> {
    let cfg: PolymarketDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3ClientMappingError::SchemaParse {
                client_id_key: client_id_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    if cfg.subscribe_new_markets {
        return Err(BoltV3ClientMappingError::ValidationInvariant {
            client_id_key: client_id_key.to_string(),
            field: "data.subscribe_new_markets",
            message: "must be false before mapping to NT because pinned NT subscribes to all Polymarket markets when this flag is true".to_string(),
        });
    }
    let ws_max_subscriptions = usize::try_from(cfg.websocket_max_subscriptions_per_connection)
        .map_err(|_| BoltV3ClientMappingError::NumericRange {
            client_id_key: client_id_key.to_string(),
            field: "data.websocket_max_subscriptions_per_connection",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.websocket_max_subscriptions_per_connection
            ),
        })?;
    let filters = build_market_slug_filters_for_client_id(plan, client_id_key, clock);
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

fn build_market_slug_filters_for_client_id(
    plan: &MarketIdentityPlan,
    client_id_key: &str,
    clock: BoltV3UpdownNowFn,
) -> Vec<Arc<dyn InstrumentFilter>> {
    plan.updown_targets
        .iter()
        .filter(|target| target.client_id_key == client_id_key)
        .map(|target| build_market_slug_filter(target, clock.clone()))
        .collect()
}

fn build_market_slug_filter(
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

fn map_execution(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    client_id_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
) -> Result<PolymarketExecClientConfig, BoltV3ClientMappingError> {
    let cfg: PolymarketExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3ClientMappingError::SchemaParse {
                client_id_key: client_id_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    let max_retries =
        u32::try_from(cfg.max_retries).map_err(|_| BoltV3ClientMappingError::NumericRange {
            client_id_key: client_id_key.to_string(),
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
        signature_type: nt_signature_type(cfg.signature_type),
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

fn secrets_for<'a>(
    client_id_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3PolymarketSecrets, BoltV3ClientMappingError> {
    match resolved.clients.get(client_id_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3ClientMappingError::SecretVenueMismatch {
                client_id_key: client_id_key.to_string(),
                expected_venue: KEY,
            }
        }),
        None => Err(BoltV3ClientMappingError::MissingResolvedSecrets {
            client_id_key: client_id_key.to_string(),
            expected_venue: KEY,
        }),
    }
}

fn nt_signature_type(value: PolymarketSignatureType) -> NtPolymarketSignatureType {
    match value {
        PolymarketSignatureType::Eoa => NtPolymarketSignatureType::Eoa,
        PolymarketSignatureType::PolyProxy => NtPolymarketSignatureType::PolyProxy,
        PolymarketSignatureType::PolyGnosisSafe => NtPolymarketSignatureType::PolyGnosisSafe,
    }
}
