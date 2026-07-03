//! Per-provider binding for `POLYMARKET` client config block shapes
//! and per-client startup validation.
//!
//! Owns the concrete shape of `[clients.<name>.data]`,
//! `[clients.<name>.execution]`, and `[clients.<name>.secrets]` for any
//! client whose `venue = "POLYMARKET"` NT venue is configured. Core
//! config in `crate::bolt_v3_config` only owns the root/strategy envelope
//! and raw NT venue field; the provider-shaped block types and their
//! serde rules live here so provider-specific schema evolution does not
//! reach back into the envelope module.
//!
//! This module also owns the per-client startup-validation policy for
//! Polymarket clients: typed deserialization of each present block,
//! cross-block presence rule ([secrets] is only allowed alongside
//! [execution]), Polymarket data/execution bounds, EVM funder-address
//! syntax, and Polymarket secret-path ownership. The cross-provider rule
//! that [execution] requires [secrets] is declared by
//! [`REQUIRED_SECRET_BLOCKS`] and enforced centrally in
//! `bolt_v3_providers::validate_client_block`. Core startup validation in
//! `crate::bolt_v3_validate`
//! dispatches into `bolt_v3_providers::validate_client_block`, which
//! routes Polymarket clients here. The neutral SSM-path utility
//! (`crate::bolt_v3_validate::validate_ssm_parameter_path`) stays in
//! core and is called from this module the same way the archetype
//! binding calls `parse_decimal_string`.

mod adapter_signing_source;
mod balance_allowance_cache;
mod collateral_accounting_source;
mod fee_behavior_source;
mod fees;
mod venue_account_state_source;

pub use adapter_signing_source::materialize_clob_v2_adapter_signing_source_from_nt_signing_source;
pub use balance_allowance_cache::sync_clob_v2_balance_allowance_cache_from_configured_account;
pub use collateral_accounting_source::materialize_clob_v2_collateral_accounting_source_from_configured_balance_allowance;
pub use fee_behavior_source::materialize_clob_v2_fee_behavior_source_from_nt_fee_sources;
pub use venue_account_state_source::materialize_venue_account_state_source_from_configured_account_queries;

use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use nautilus_core::string::secret::REDACTED;
use nautilus_model::identifiers::AccountId;
use nautilus_polymarket::{
    common::consts::{HTTP_RATE_LIMIT, LOT_SIZE_SCALE},
    common::credential::{EvmPrivateKey, Secrets as PolymarketSecrets},
    common::enums::SignatureType as NtPolymarketSignatureType,
    config::{
        PolymarketDataClientConfig, PolymarketExecClientConfig, PolymarketInstrumentProviderConfig,
    },
    factories::{PolymarketDataClientFactory, PolymarketExecutionClientFactory},
    filters::{
        EventQueryFilter, EventSlugFilter, GammaQueryFilter, InstrumentFilter, MarketSlugFilter,
        SearchFilter,
    },
    http::clob::PolymarketClobHttpClient,
    http::query::{GetGammaMarketsParams, GetSearchParams},
};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
        BoltV3ExecutionClientAdapterConfig, BoltV3MarketClockFn,
    },
    bolt_v3_config::{BoltV3RootConfig, ClientBlock},
    bolt_v3_market_families::{
        MarketIdentityPlan, outcome_group,
        static_binary_event::{self, StaticBinaryEventTargetPlan},
        updown::{self, UpdownTargetPlan, updown_market_slug, updown_period_pair},
    },
    bolt_v3_outcome_group_sources::{
        GammaQueryBlock, OutcomeGroupSourceConfig, OutcomeGroupSourceKind,
    },
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ProviderSsmPathReference,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{BoltV3SecretError, resolve_field},
    bolt_v3_wire_boundary::TransportBackend,
    strategies::registry::FeeProvider,
};

use self::fees::PolymarketClobFeeProvider;

pub const KEY: &str = "POLYMARKET";
/// Per-minute REST egress ceiling for the Polymarket HTTP clients, taken from
/// the NT adapter's own quota constant so bolt-v3 and NT share one source of
/// truth for the venue capability.
pub const REST_EGRESS_CAP_PER_MINUTE: u32 = HTTP_RATE_LIMIT;

pub fn normalize_base_order_quantity(quantity: Decimal) -> Option<Decimal> {
    if quantity <= Decimal::ZERO {
        return None;
    }
    let normalized = quantity.round_dp_with_strategy(LOT_SIZE_SCALE, RoundingStrategy::ToZero);
    if normalized > Decimal::ZERO {
        Some(normalized)
    } else {
        None
    }
}
/// Worst-case REST requests a single NT order command issues against Polymarket,
/// used to derate the command-rate ceiling so a config cannot pass validation
/// yet over-drive the venue's REST quota. Taken from the pinned NT adapter
/// (`adapters/polymarket/src/execution/`): a MARKET submit issues `get_book` +
/// `post_order` = 2 requests (`submitter.rs`), and a market quote-quantity BUY
/// issues a 3rd pre-submit `fetch_collateral_balance_pusd` request
/// (`execution/mod.rs`, only when `side==Buy && is_quote_quantity`). A LIMIT
/// submit issues 1; a modify issues 0 (rejected locally). The global
/// submit/modify throttle does not distinguish order type, so this worst case is
/// the only sound bound. Excludes transient
/// RetryManager retries and non-submit calls (cancels, status, readiness/account
/// probes); the full shared REST budget is the venue egress-capability contract
/// tracked in #501.
pub const MAX_REST_REQUESTS_PER_ORDER_COMMAND: u32 = 3;
/// Minimum quote notional accepted by the Polymarket CLOB for marketable BUYs.
pub const MARKET_QUOTE_BUY_MIN_NOTIONAL: Decimal = Decimal::ONE;
pub const SUPPORTED_MARKET_FAMILIES: &[&str] =
    &[updown::KEY, outcome_group::KEY, static_binary_event::KEY];
const URL_SAFE_BASE64_BLOCK_WIDTH: usize = 4;
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Execution,
    consumer: "Polymarket execution client",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &[
    "private_key_ssm_path",
    "api_key_ssm_path",
    "api_secret_ssm_path",
    "passphrase_ssm_path",
];
/// NT module path(s) whose info-level logs can echo Polymarket credential
/// metadata; the live-node builder installs `WARN` filters for these so secret
/// material never reaches operator logs. The path is pinned to the NT revision
/// declared by `nautilus-polymarket` in `Cargo.toml` (single source of truth
/// for the rev) and is kept honest at compile time by the
/// `use nautilus_polymarket::common::credential::{..}` import above: if the NT
/// rev moved this module, that import — and therefore the build — would fail
/// before this string could silently drift.
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
    pub base_url_rtds: String,
    pub base_url_gamma: String,
    pub base_url_data_api: String,
    pub http_timeout_secs: u64,
    pub ws_timeout_secs: u64,
    pub subscribe_new_markets: bool,
    pub new_market_fetch_max_concurrency: u64,
    pub auto_load_missing_instruments: bool,
    pub auto_load_debounce_ms: u64,
    pub auto_load_max_retries: u32,
    pub auto_load_retry_delay_initial_secs: u64,
    pub auto_load_retry_delay_max_secs: u64,
    pub resolve_poll_enabled: bool,
    pub resolve_poll_interval_secs: u64,
    pub resolve_poll_grace_secs: u64,
    pub resolve_poll_max_wait_secs: u64,
    pub update_instruments_interval_mins: u64,
    pub ws_max_subscriptions: u64,
    pub transport_backend: TransportBackend,
}

pub fn metadata_refresh_interval_mins(client: &ClientBlock) -> Result<Option<u64>, String> {
    let Some(data) = client.data.as_ref() else {
        return Ok(None);
    };
    let data = data
        .clone()
        .try_into::<PolymarketDataConfig>()
        .map_err(|error| error.to_string())?;
    Ok(Some(data.update_instruments_interval_mins))
}

/// NT's `impl_serialization_for_identifier!` macro deserializes typed
/// identifiers through `&str: Deserialize`, which only the borrowed
/// (zero-copy) visitor path supports. `toml::Value::try_into` — used
/// by the adapter mapping to parse each `[clients.<id>.execution]`
/// block from an already-owned `toml::Value` tree — drives the owned
/// String visitor, so NT's typed serde rejects it with "expected a
/// borrowed string". This helper deserializes through `String` first
/// (works for both borrowed and owned source) and then routes through
/// `AccountId::new_checked` for typed validation, preserving NT's
/// parse-time rejection of empty / invalid identifiers.
fn deserialize_account_id<'de, D>(deserializer: D) -> Result<AccountId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value: String = String::deserialize(deserializer)?;
    AccountId::new_checked(value.as_str()).map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketExecutionConfig {
    #[serde(deserialize_with = "deserialize_account_id")]
    pub account_id: AccountId,
    pub signature_type: PolymarketSignatureType,
    /// Public funder address. Required when `signature_type` is
    /// `poly_proxy` or `poly_gnosis_safe` (the proxy/safe routes the
    /// underlying funder wallet); permitted to be absent for `eoa`,
    /// where the EOA is itself the funder. Validation enforces this
    /// per-signature-type requirement and the EVM address syntax.
    pub funder: Option<String>,
    pub base_url_http: String,
    pub base_url_ws: String,
    pub base_url_data_api: String,
    pub http_timeout_secs: u64,
    pub max_retries: u64,
    pub retry_delay_initial_ms: u64,
    pub retry_delay_max_ms: u64,
    pub ack_timeout_secs: u64,
    pub fee_cache_ttl_secs: u64,
    pub transport_backend: TransportBackend,
    pub on_chain_collateral: Option<PolymarketOnChainCollateralConfig>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PolymarketOnChainCollateralConfig {
    pub rpc_url: String,
    pub chain_id: u64,
    pub collateral_token_address: String,
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

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3PolymarketSecrets {
    /// Each secret field is wrapped in [`Zeroizing`] so the individual secret
    /// bytes are scrubbed on drop even when a field is moved out of the
    /// container — per-field zeroize in addition to the container-level
    /// `ZeroizeOnDrop`. All four fields deref to `String`; the redacting
    /// `Debug` impl below keeps them out of logs.
    pub private_key: Zeroizing<String>,
    pub api_key: Zeroizing<String>,
    /// Canonical URL-safe base64 `api_secret` (padded) handed to the NT
    /// Polymarket credential, which decodes it with the padded `URL_SAFE`
    /// engine. The raw SSM value is canonicalized via
    /// [`normalize_api_secret_padding`] before storage so NT never re-derives
    /// padding silently. `normalize_api_secret_padding` only ever APPENDS `=`
    /// padding and never rewrites the data characters, so the raw SSM byte
    /// string is recoverable as this value with trailing `=` removed — see
    /// [`redaction_values`](Self::redaction_values), which redacts both forms.
    pub api_secret: Zeroizing<String>,
    pub passphrase: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3PolymarketSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3PolymarketSecrets")
            .field("private_key", &REDACTED)
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
            .field("passphrase", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3PolymarketSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        // Redact BOTH the canonical (padded) `api_secret` and the raw SSM byte
        // string so the post-run residue scan catches the secret in either
        // representation. `normalize_api_secret_padding` only appends `=`
        // padding, so the raw SSM value is exactly the stored value with its
        // trailing `=` removed; callers dedup identical entries, so an
        // already-canonical secret contributes a single redaction value.
        vec![
            self.private_key.as_str(),
            self.api_key.as_str(),
            self.api_secret.as_str(),
            self.api_secret.trim_end_matches('='),
            self.passphrase.as_str(),
        ]
    }
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(data) = &client.data {
        match data.clone().try_into::<PolymarketDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(execution) = &client.execution {
        // Polymarket per-target market-slug filters are attached during
        // data-client mapping by `build_market_slug_filters_for_client`
        // and bind by `client_key`. A Polymarket client_key that carries
        // [execution] but no [data] block cannot receive those filters,
        // so any strategy routing execution through this client_key
        // would silently lose its configured target market restriction.
        // Fail closed by requiring the [data] adapter to be co-located
        // on the same `clients.<id>` as the [execution] adapter.
        if client.data.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [execution] but no [data] block is configured; \
                 Polymarket per-target market-slug filters are attached during data-client mapping and bind by \
                 client_key, so the [data] adapter must be co-located on the same `clients.<id>` as the \
                 [execution] adapter to keep configured target market filters bound to this client_key"
            ));
        }
        match execution.clone().try_into::<PolymarketExecutionConfig>() {
            Ok(parsed) => {
                errors.extend(validate_funder(key, &parsed));
                errors.extend(validate_execution_bounds(key, &parsed));
                errors.extend(validate_on_chain_collateral(key, &parsed));
            }
            Err(message) => {
                errors.push(format!("clients.{key}.execution: {message}"));
            }
        }
    }
    if let Some(secrets) = &client.secrets {
        // Only Polymarket execution consumes Polymarket credentials in
        // this slice. A data-only Polymarket venue with `[secrets]`
        // would carry credential paths that no adapter uses, which is a
        // misconfiguration rather than a silent no-op.
        if client.execution.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [execution] block is configured; \
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

fn validate_funder(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let funder = execution
        .funder
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let requires_funder = matches!(
        execution.signature_type,
        PolymarketSignatureType::PolyProxy | PolymarketSignatureType::PolyGnosisSafe
    );
    match (requires_funder, funder) {
        (true, None) => errors.push(format!(
            "clients.{key}.execution.funder is required when signature_type is `poly_proxy` or `poly_gnosis_safe`"
        )),
        (_, Some(value)) => {
            if let Err(message) = check_evm_address_syntax(value) {
                errors.push(format!(
                    "clients.{key}.execution.funder is not a valid EVM public address ({message}): `{value}`"
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
        ("http_timeout_secs", data.http_timeout_secs),
        ("ws_timeout_secs", data.ws_timeout_secs),
        (
            "update_instruments_interval_mins",
            data.update_instruments_interval_mins,
        ),
        (
            stringify!(new_market_fetch_max_concurrency),
            data.new_market_fetch_max_concurrency,
        ),
        (
            stringify!(resolve_poll_interval_secs),
            data.resolve_poll_interval_secs,
        ),
        (
            stringify!(resolve_poll_grace_secs),
            data.resolve_poll_grace_secs,
        ),
        (
            stringify!(resolve_poll_max_wait_secs),
            data.resolve_poll_max_wait_secs,
        ),
        ("ws_max_subscriptions", data.ws_max_subscriptions),
        ("auto_load_debounce_ms", data.auto_load_debounce_ms),
        ("auto_load_max_retries", data.auto_load_max_retries as u64),
        (
            "auto_load_retry_delay_initial_secs",
            data.auto_load_retry_delay_initial_secs,
        ),
        (
            "auto_load_retry_delay_max_secs",
            data.auto_load_retry_delay_max_secs,
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
    if data.auto_load_retry_delay_initial_secs > data.auto_load_retry_delay_max_secs {
        errors.push(format!(
            "clients.{key}.data.auto_load_retry_delay_initial_secs ({}) must be <= auto_load_retry_delay_max_secs ({})",
            data.auto_load_retry_delay_initial_secs, data.auto_load_retry_delay_max_secs
        ));
    }
    if data.auto_load_missing_instruments {
        errors.push(format!(
            "clients.{key}.data.auto_load_missing_instruments must be false in the current bolt-v3 scope; \
             missing-instrument auto-load can trigger ad-hoc Gamma loads outside the configured \
             market-identity plan"
        ));
    }
    errors
}

fn validate_execution_bounds(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        ("http_timeout_secs", execution.http_timeout_secs),
        ("max_retries", execution.max_retries),
        ("retry_delay_initial_ms", execution.retry_delay_initial_ms),
        ("retry_delay_max_ms", execution.retry_delay_max_ms),
        ("ack_timeout_secs", execution.ack_timeout_secs),
        ("fee_cache_ttl_secs", execution.fee_cache_ttl_secs),
    ];
    for (field, value) in positive_fields {
        if *value == 0 {
            errors.push(format!(
                "clients.{key}.execution.{field} must be a positive integer"
            ));
        }
    }
    if execution.retry_delay_initial_ms > execution.retry_delay_max_ms {
        errors.push(format!(
            "clients.{key}.execution.retry_delay_initial_ms ({}) must be <= retry_delay_max_ms ({})",
            execution.retry_delay_initial_ms, execution.retry_delay_max_ms
        ));
    }
    errors
}

fn validate_on_chain_collateral(key: &str, execution: &PolymarketExecutionConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(on_chain) = execution.on_chain_collateral.as_ref() else {
        return errors;
    };
    if !on_chain.rpc_url.starts_with("http://") && !on_chain.rpc_url.starts_with("https://") {
        errors.push(format!(
            "clients.{key}.execution.on_chain_collateral.rpc_url must start with http:// or https://"
        ));
    }
    if on_chain.chain_id == 0 {
        errors.push(format!(
            "clients.{key}.execution.on_chain_collateral.chain_id must be a positive integer"
        ));
    }
    if let Err(message) = check_evm_address_syntax(&on_chain.collateral_token_address) {
        errors.push(format!(
            "clients.{key}.execution.on_chain_collateral.collateral_token_address is not a valid EVM public address ({message}): `{}`",
            on_chain.collateral_token_address
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
    let secrets = parse_secrets_config(&context)?;
    let private_key = resolve_field(
        context.client_key,
        "private_key_ssm_path",
        context.region,
        &secrets.private_key_ssm_path,
        resolver,
    )?;
    if let Err(reason) = validate_private_key_shape(&private_key) {
        return Err(BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "private_key_ssm_path".to_string(),
            source: format!(
                "resolved polymarket private_key is not valid EVM private key material accepted by the NautilusTrader polymarket adapter: {reason}"
            ),
        });
    }
    let api_key = resolve_field(
        context.client_key,
        "api_key_ssm_path",
        context.region,
        &secrets.api_key_ssm_path,
        resolver,
    )?;
    let api_secret_raw = resolve_field(
        context.client_key,
        "api_secret_ssm_path",
        context.region,
        &secrets.api_secret_ssm_path,
        resolver,
    )?;
    let api_secret = normalize_api_secret_padding(api_secret_raw);
    // Symmetric with `validate_private_key_shape` (Polymarket) and
    // `validate_binance_api_secret_shape` (Binance): reject api_secret material
    // the NT Polymarket credential cannot decode BEFORE it is stored, so a
    // malformed secret fails loud at SSM resolution rather than deep inside NT
    // client construction.
    if let Err(reason) = validate_api_secret_shape(&api_secret) {
        return Err(BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "api_secret_ssm_path".to_string(),
            source: format!(
                "resolved polymarket api_secret is not valid URL-safe base64 material accepted by the NautilusTrader polymarket adapter: {reason}"
            ),
        });
    }
    let passphrase = resolve_field(
        context.client_key,
        "passphrase_ssm_path",
        context.region,
        &secrets.passphrase_ssm_path,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3PolymarketSecrets {
        private_key: Zeroizing::new(private_key),
        api_key: Zeroizing::new(api_key),
        api_secret: Zeroizing::new(api_secret),
        passphrase: Zeroizing::new(passphrase),
    }))
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    Ok(vec![
        ProviderSsmPathReference {
            field_name: "private_key_ssm_path",
            ssm_path: secrets.private_key_ssm_path,
        },
        ProviderSsmPathReference {
            field_name: "api_key_ssm_path",
            ssm_path: secrets.api_key_ssm_path,
        },
        ProviderSsmPathReference {
            field_name: "api_secret_ssm_path",
            ssm_path: secrets.api_secret_ssm_path,
        },
        ProviderSsmPathReference {
            field_name: "passphrase_ssm_path",
            ssm_path: secrets.passphrase_ssm_path,
        },
    ])
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<PolymarketSecretsConfig, BoltV3SecretError> {
    let secrets_value = context
        .client
        .secrets
        .as_ref()
        .ok_or_else(|| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: "secrets".to_string(),
            source: "missing [secrets] block".to_string(),
        })?;
    secrets_value
        .clone()
        .try_into()
        .map_err(|error: toml::de::Error| BoltV3SecretError {
            client_key: context.client_key.to_string(),
            field: KEY.to_string(),
            source: format!("invalid polymarket secrets schema: {error}"),
        })
}

fn validate_private_key_shape(private_key: &str) -> Result<(), String> {
    EvmPrivateKey::new(private_key)
        .map(|_| ())
        .map_err(|source| source.to_string())
}

fn normalize_api_secret_padding(mut api_secret: String) -> String {
    let pad_len = (URL_SAFE_BASE64_BLOCK_WIDTH - api_secret.len() % URL_SAFE_BASE64_BLOCK_WIDTH)
        % URL_SAFE_BASE64_BLOCK_WIDTH;
    api_secret.extend(std::iter::repeat_n('=', pad_len));
    api_secret
}

/// Validates that the (padding-canonicalized) Polymarket `api_secret` decodes
/// under the same padded `URL_SAFE` base64 engine the pinned NT Polymarket
/// `Credential::new` uses, so an unusable secret is rejected at SSM resolution
/// time rather than surfacing as an opaque NT client-construction failure
/// later. Mirrors the resolve-time shape checks for `private_key`
/// ([`validate_private_key_shape`]) and the Binance `api_secret`
/// (`binance::validate_binance_api_secret_shape`).
fn validate_api_secret_shape(api_secret: &str) -> Result<(), String> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE
        .decode(api_secret)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => Some(BoltV3DataClientAdapterConfig {
            factory: Box::new(PolymarketDataClientFactory),
            config: Box::new(map_data(
                context.root,
                context.client_key,
                value,
                context.plan,
                context.clock,
            )?),
        }),
        None => None,
    };
    let execution = match &context.client.execution {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3ExecutionClientAdapterConfig {
                factory: Box::new(PolymarketExecutionClientFactory),
                config: Box::new(map_execution(
                    context.root,
                    context.client_key,
                    value,
                    secrets,
                )?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig { data, execution })
}

pub fn build_fee_provider(
    client_key: &str,
    client: &ClientBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<Arc<dyn FeeProvider>, BoltV3AdapterMappingError> {
    let value = client.execution.as_ref().ok_or_else(|| {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution",
            message: "is required by the existing taker fee-provider boundary".to_string(),
        }
    })?;
    let cfg: PolymarketExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    let secrets = secrets_for(client_key, resolved)?;
    let secrets = PolymarketSecrets::resolve(
        Some(secrets.private_key.as_str()),
        Some(secrets.api_key.as_str().to_owned()),
        Some(secrets.api_secret.as_str().to_owned()),
        Some(secrets.passphrase.as_str().to_owned()),
        cfg.funder.clone(),
    )
    .map_err(|error| BoltV3AdapterMappingError::ValidationInvariant {
        client_key: client_key.to_string(),
        field: "execution",
        message: format!("failed to resolve Polymarket fee credentials: {error}"),
    })?;
    if !cfg.base_url_http.starts_with("http://") && !cfg.base_url_http.starts_with("https://") {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "execution.base_url_http",
            message: "failed to create Polymarket fee HTTP client: base_url_http must start with http:// or https://"
                .to_string(),
        });
    }
    let client = PolymarketClobHttpClient::new(
        secrets.credential,
        secrets.address,
        Some(cfg.base_url_http),
        cfg.http_timeout_secs,
    )
    .map_err(|error| BoltV3AdapterMappingError::ValidationInvariant {
        client_key: client_key.to_string(),
        field: "execution.base_url_http",
        message: format!("failed to create Polymarket fee HTTP client: {error}"),
    })?;

    Ok(Arc::new(PolymarketClobFeeProvider::new(
        client,
        Duration::from_secs(cfg.fee_cache_ttl_secs),
    )))
}

fn map_data(
    root: &BoltV3RootConfig,
    client_key: &str,
    value: &toml::Value,
    plan: &MarketIdentityPlan,
    clock: BoltV3MarketClockFn,
) -> Result<PolymarketDataClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketDataConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "data",
                message: error.to_string(),
            }
        })?;
    if cfg.subscribe_new_markets {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "data.subscribe_new_markets",
            message: "must be false before mapping to NT because pinned NT subscribes to all Polymarket markets when this flag is true".to_string(),
        });
    }
    let ws_max_subscriptions = usize::try_from(cfg.ws_max_subscriptions).map_err(|_| {
        BoltV3AdapterMappingError::NumericRange {
            client_key: client_key.to_string(),
            field: "data.ws_max_subscriptions",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.ws_max_subscriptions
            ),
        }
    })?;
    let new_market_fetch_max_concurrency = usize::try_from(cfg.new_market_fetch_max_concurrency)
        .map_err(|_| BoltV3AdapterMappingError::NumericRange {
            client_key: client_key.to_string(),
            field: "data.new_market_fetch_max_concurrency",
            message: format!(
                "value {} does not fit in usize on this target",
                cfg.new_market_fetch_max_concurrency
            ),
        })?;
    let filters = build_instrument_filters_for_client(root, plan, client_key, clock)?;
    Ok(PolymarketDataClientConfig {
        // Restore the OLD-NT connect-time filtered bootstrap. In the pinned NT
        // rev the provider only loads instruments when `instrument_config`
        // triggers `should_load_all()`/`has_load_ids()`; `None` loaded nothing,
        // so with `auto_load_missing_instruments = false` every subscribe bailed
        // and the strategy received zero Polymarket data. `load_all = true` with
        // no slug scope routes `initialize()` -> `load_scoped_all()` ->
        // `load_all()` -> `load_filtered()`, applying the runtime `filters` below
        // exactly as OLD `provider.load_all(None)` did, and re-enables the
        // `update_instruments` refresh task (which early-returns when None).
        instrument_config: Some(
            PolymarketInstrumentProviderConfig::builder()
                .load_all(true)
                .build(),
        ),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_rtds: Some(cfg.base_url_rtds),
        base_url_gamma: Some(cfg.base_url_gamma),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_secs,
        ws_timeout_secs: cfg.ws_timeout_secs,
        ws_max_subscriptions,
        update_instruments_interval_mins: Some(cfg.update_instruments_interval_mins),
        subscribe_new_markets: cfg.subscribe_new_markets,
        new_market_fetch_max_concurrency,
        auto_load_missing_instruments: cfg.auto_load_missing_instruments,
        auto_load_debounce_ms: cfg.auto_load_debounce_ms,
        auto_load_max_retries: cfg.auto_load_max_retries,
        auto_load_retry_delay_initial_secs: cfg.auto_load_retry_delay_initial_secs as f64,
        auto_load_retry_delay_max_secs: cfg.auto_load_retry_delay_max_secs as f64,
        resolve_poll_enabled: cfg.resolve_poll_enabled,
        resolve_poll_interval_secs: cfg.resolve_poll_interval_secs,
        resolve_poll_grace_secs: cfg.resolve_poll_grace_secs,
        resolve_poll_max_wait_secs: cfg.resolve_poll_max_wait_secs,
        transport_backend: cfg.transport_backend,
        filters,
        new_market_filter: None,
    })
}

fn build_instrument_filters_for_client(
    root: &BoltV3RootConfig,
    plan: &MarketIdentityPlan,
    client_key: &str,
    clock: BoltV3MarketClockFn,
) -> Result<Vec<Arc<dyn InstrumentFilter>>, BoltV3AdapterMappingError> {
    // All filters composed here must remain fetch-only filters. NT applies
    // `accept` predicates as an intersection across filters, so adding a
    // predicate-style filter here would turn independent target families into
    // a data-starvation path.
    let mut filters = updown::target_plans(plan)
        .filter(|target| target.execution_client_id == client_key)
        .map(|target| build_market_slug_filter(target, clock.clone()))
        .collect::<Vec<_>>();
    filters.extend(build_outcome_group_filters_for_client(
        root, plan, client_key,
    )?);
    filters.extend(
        static_binary_event::target_plans(plan)
            .filter(|target| target.execution_client_id == client_key)
            .map(build_static_market_slug_filter),
    );
    Ok(filters)
}

fn build_market_slug_filter(
    target: &UpdownTargetPlan,
    clock: BoltV3MarketClockFn,
) -> Arc<dyn InstrumentFilter> {
    let asset = target.underlying_asset.clone();
    let token = target.cadence_slug_token.clone();
    let cadence = target.cadence_secs;
    Arc::new(MarketSlugFilter::new(move || {
        let now = (clock)();
        match updown_period_pair(cadence, now) {
            Ok((current, next)) => vec![
                updown_market_slug(&asset, &token, current),
                updown_market_slug(&asset, &token, next),
            ],
            Err(error) => {
                // Fail closed: returning an empty slug set narrows the
                // Polymarket instrument universe to zero for this cycle, which
                // starves the strategy of tradeable instruments. That is the
                // safe direction, but it must never be silent — emit at
                // `error!` so an operator sees the data-starvation instead of a
                // quiet warning that scrolls past.
                log::error!(
                    "bolt-v3 provider binding: failing closed on updown filter cycle (cadence={cadence}, now_unix_secs={now}); instrument universe narrowed to zero for this cycle: {error}"
                );
                Vec::new()
            }
        }
    }))
}

fn build_outcome_group_filters_for_client(
    root: &BoltV3RootConfig,
    plan: &MarketIdentityPlan,
    client_key: &str,
) -> Result<Vec<Arc<dyn InstrumentFilter>>, BoltV3AdapterMappingError> {
    let configured_sources = match root.outcome_group_sources.as_deref() {
        Some(sources) => sources,
        None => &[],
    };
    let sources_by_id = configured_sources
        .iter()
        .map(|source| (source.source_id.as_str(), source))
        .collect::<BTreeMap<_, _>>();
    let mut filters = Vec::new();
    let mut added_source_ids = BTreeSet::new();

    for target in
        outcome_group::target_plans(plan).filter(|target| target.execution_client_id == client_key)
    {
        for source_id in &target.group_sources {
            if !added_source_ids.insert(source_id.as_str()) {
                continue;
            }
            let source = sources_by_id.get(source_id.as_str()).ok_or_else(|| {
                BoltV3AdapterMappingError::ValidationInvariant {
                    client_key: client_key.to_string(),
                    field: "outcome_group_sources",
                    message: format!(
                        "target.group_sources references unknown outcome_group_sources source_id `{source_id}`"
                    ),
                }
            })?;
            if !source.enabled {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    client_key: client_key.to_string(),
                    field: "outcome_group_sources.enabled",
                    message: format!(
                        "target.group_sources references disabled outcome_group_sources source_id `{source_id}`"
                    ),
                });
            }
            if source.client_id.to_string() != client_key {
                return Err(BoltV3AdapterMappingError::ValidationInvariant {
                    client_key: client_key.to_string(),
                    field: "outcome_group_sources.client_id",
                    message: format!(
                        "target.group_sources source_id `{source_id}` maps to client_id `{}`",
                        source.client_id
                    ),
                });
            }
            filters.push(build_outcome_group_filter(client_key, source)?);
        }
    }

    Ok(filters)
}

fn build_outcome_group_filter(
    client_key: &str,
    source: &OutcomeGroupSourceConfig,
) -> Result<Arc<dyn InstrumentFilter>, BoltV3AdapterMappingError> {
    match source.kind {
        OutcomeGroupSourceKind::GammaEvent => {
            let event_slugs = required_values(
                client_key,
                "outcome_group_sources.event_slugs",
                source.event_slugs.as_ref(),
            )?;
            if source.max_markets.is_some()
                || source
                    .sports_market_types
                    .as_ref()
                    .is_some_and(|values| !values.is_empty())
            {
                let params = gamma_market_params(
                    client_key,
                    "outcome_group_sources.max_markets",
                    source.max_markets,
                    source.sports_market_types.as_ref(),
                    None,
                )?;
                let queries = event_slugs
                    .into_iter()
                    .map(|event_slug| (event_slug, params.clone()))
                    .collect();
                Ok(Arc::new(EventQueryFilter::from_queries(queries)))
            } else {
                Ok(Arc::new(EventSlugFilter::from_slugs(event_slugs)))
            }
        }
        OutcomeGroupSourceKind::GammaMarketSlug => {
            let market_slugs = required_values(
                client_key,
                "outcome_group_sources.market_slugs",
                source.market_slugs.as_ref(),
            )?;
            Ok(Arc::new(MarketSlugFilter::from_slugs(market_slugs)))
        }
        OutcomeGroupSourceKind::GammaQuery => {
            let query = source.gamma_query.as_ref().ok_or_else(|| {
                BoltV3AdapterMappingError::ValidationInvariant {
                    client_key: client_key.to_string(),
                    field: "outcome_group_sources.gamma_query",
                    message: "is required for polymarket_gamma_query".to_string(),
                }
            })?;
            build_gamma_query_filter(client_key, query)
        }
        OutcomeGroupSourceKind::Hip4 => Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "outcome_group_sources.kind",
            message: "hyperliquid_hip4 source cannot map to Polymarket NT filters".to_string(),
        }),
    }
}

fn build_gamma_query_filter(
    client_key: &str,
    query: &GammaQueryBlock,
) -> Result<Arc<dyn InstrumentFilter>, BoltV3AdapterMappingError> {
    let max_markets = required_cap_to_u32(
        client_key,
        "outcome_group_sources.gamma_query.max_markets",
        query.max_markets,
    )?;
    if let Some(event_query) = query
        .event_query
        .as_ref()
        .filter(|value| !value.trim().is_empty())
    {
        let params = gamma_market_params(
            client_key,
            "outcome_group_sources.gamma_query.max_markets",
            Some(query.max_markets),
            query.sports_market_types.as_ref(),
            query.tag_id.as_ref(),
        )?;
        return Ok(Arc::new(EventQueryFilter::new(event_query.clone(), params)));
    }
    if let Some(search) = query
        .search
        .as_ref()
        .or(query.market_query.as_ref())
        .filter(|value| !value.trim().is_empty())
    {
        if query
            .sports_market_types
            .as_ref()
            .is_some_and(|values| !values.is_empty())
        {
            return Err(BoltV3AdapterMappingError::ValidationInvariant {
                client_key: client_key.to_string(),
                field: "outcome_group_sources.gamma_query.sports_market_types",
                message: "cannot be combined with search or market_query".to_string(),
            });
        }
        return Ok(Arc::new(SearchFilter::new(GetSearchParams {
            q: Some(search.clone()),
            events_status: None,
            events_tag: query.tag_id.clone(),
            sort: None,
            ascending: None,
            limit_per_type: Some(max_markets),
            page: None,
            keep_closed_markets: None,
        })));
    }
    if query
        .tag_id
        .as_ref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: stringify!(outcome_group_sources.gamma_query),
            message: "must include at least one bounded selector".to_string(),
        });
    }

    Ok(Arc::new(GammaQueryFilter::new(gamma_market_params(
        client_key,
        "outcome_group_sources.gamma_query.max_markets",
        Some(query.max_markets),
        query.sports_market_types.as_ref(),
        query.tag_id.as_ref(),
    )?)))
}

fn gamma_market_params(
    client_key: &str,
    cap_field: &'static str,
    max_markets: Option<usize>,
    sports_market_types: Option<&Vec<String>>,
    tag_id: Option<&String>,
) -> Result<GetGammaMarketsParams, BoltV3AdapterMappingError> {
    Ok(GetGammaMarketsParams {
        active: None,
        closed: None,
        archived: None,
        id: None,
        limit: None,
        offset: None,
        order: None,
        ascending: None,
        slug: None,
        clob_token_ids: None,
        condition_ids: None,
        liquidity_num_min: None,
        liquidity_num_max: None,
        volume_num_min: None,
        volume_num_max: None,
        start_date_min: None,
        start_date_max: None,
        end_date_min: None,
        end_date_max: None,
        tag_id: tag_id.cloned(),
        related_tags: None,
        rewards_min_size: None,
        include_tag: None,
        question_ids: None,
        game_id: None,
        sports_market_types: joined_sports_market_types(sports_market_types),
        market_maker_address: None,
        max_markets: optional_cap_to_u32(client_key, cap_field, max_markets)?,
    })
}

fn joined_sports_market_types(values: Option<&Vec<String>>) -> Option<String> {
    values
        .filter(|values| !values.is_empty())
        .map(|values| values.join(","))
}

fn required_values(
    client_key: &str,
    field: &'static str,
    values: Option<&Vec<String>>,
) -> Result<Vec<String>, BoltV3AdapterMappingError> {
    match values.filter(|values| !values.is_empty()) {
        Some(values) => Ok(values.clone()),
        None => Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field,
            message: "must contain at least one value".to_string(),
        }),
    }
}

fn optional_cap_to_u32(
    client_key: &str,
    field: &'static str,
    value: Option<usize>,
) -> Result<Option<u32>, BoltV3AdapterMappingError> {
    value
        .map(|value| required_cap_to_u32(client_key, field, value))
        .transpose()
}

fn required_cap_to_u32(
    client_key: &str,
    field: &'static str,
    value: usize,
) -> Result<u32, BoltV3AdapterMappingError> {
    u32::try_from(value).map_err(|_| BoltV3AdapterMappingError::NumericRange {
        client_key: client_key.to_string(),
        field,
        message: format!("value {value} does not fit in u32 expected by NT"),
    })
}

fn build_static_market_slug_filter(
    target: &StaticBinaryEventTargetPlan,
) -> Arc<dyn InstrumentFilter> {
    Arc::new(MarketSlugFilter::from_slugs(vec![
        target.market_slug.clone(),
    ]))
}

fn map_execution(
    root: &crate::bolt_v3_config::BoltV3RootConfig,
    client_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3PolymarketSecrets,
) -> Result<PolymarketExecClientConfig, BoltV3AdapterMappingError> {
    let cfg: PolymarketExecutionConfig =
        value.clone().try_into().map_err(|error: toml::de::Error| {
            BoltV3AdapterMappingError::SchemaParse {
                client_key: client_key.to_string(),
                block: "execution",
                message: error.to_string(),
            }
        })?;
    let max_retries =
        u32::try_from(cfg.max_retries).map_err(|_| BoltV3AdapterMappingError::NumericRange {
            client_key: client_key.to_string(),
            field: "execution.max_retries",
            message: format!(
                "value {} does not fit in u32 expected by NT",
                cfg.max_retries
            ),
        })?;
    Ok(PolymarketExecClientConfig {
        trader_id: root.trader_id,
        account_id: cfg.account_id,
        private_key: Some(secrets.private_key.as_str().to_owned()),
        api_key: Some(secrets.api_key.as_str().to_owned()),
        api_secret: Some(secrets.api_secret.as_str().to_owned()),
        passphrase: Some(secrets.passphrase.as_str().to_owned()),
        funder: cfg.funder,
        signature_type: nt_signature_type(cfg.signature_type),
        base_url_http: Some(cfg.base_url_http),
        base_url_ws: Some(cfg.base_url_ws),
        base_url_data_api: Some(cfg.base_url_data_api),
        http_timeout_secs: cfg.http_timeout_secs,
        max_retries,
        retry_delay_initial_ms: cfg.retry_delay_initial_ms,
        retry_delay_max_ms: cfg.retry_delay_max_ms,
        ack_timeout_secs: cfg.ack_timeout_secs,
        transport_backend: cfg.transport_backend,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_slug_filters_include_static_binary_event_targets_for_matching_client() {
        let mut plan = MarketIdentityPlan::empty();
        plan.push_target(StaticBinaryEventTargetPlan {
            strategy_instance_id: "sample-static-alpha".to_string(),
            configured_target_id: "sample-static-alpha-target".to_string(),
            execution_client_id: "polymarket_main".to_string(),
            event_key: "sample_event_2026".to_string(),
            market_slug: "will-sample-alpha-resolve-yes".to_string(),
            condition_id: Some("condition-sample-alpha".to_string()),
            yes_outcome: "Yes".to_string(),
            no_outcome: "No".to_string(),
        });
        plan.push_target(StaticBinaryEventTargetPlan {
            strategy_instance_id: "sample-static-beta".to_string(),
            configured_target_id: "sample-static-beta-target".to_string(),
            execution_client_id: "polymarket_secondary".to_string(),
            event_key: "sample_event_2026".to_string(),
            market_slug: "will-sample-beta-resolve-yes".to_string(),
            condition_id: Some("condition-sample-beta".to_string()),
            yes_outcome: "Yes".to_string(),
            no_outcome: "No".to_string(),
        });
        let root: BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root config should parse");
        let filters = build_instrument_filters_for_client(
            &root,
            &plan,
            "polymarket_main",
            Arc::new(|| 1_746_000_000),
        )
        .expect("static-only target plan should build filters");

        let slugs = filters
            .iter()
            .flat_map(|filter| filter.market_slugs().unwrap_or_default())
            .collect::<Vec<_>>();

        assert_eq!(slugs, vec!["will-sample-alpha-resolve-yes".to_string()]);
    }

    #[test]
    fn map_data_enables_filtered_instrument_bootstrap_for_targeted_polymarket_client() {
        // Regression for the NT-bump bug where `instrument_config: None` disabled
        // the connect-time instrument bootstrap. In the pinned NT rev the provider
        // only loads instruments when `instrument_config` triggers
        // `should_load_all()`/`has_load_ids()`; with `auto_load_missing_instruments
        // = false` (prod/fixture default) every subscribe then bailed and the
        // strategy received zero Polymarket data.
        let mut plan = MarketIdentityPlan::empty();
        plan.push_target(StaticBinaryEventTargetPlan {
            strategy_instance_id: "sample-static-alpha".to_string(),
            configured_target_id: "sample-static-alpha-target".to_string(),
            execution_client_id: "polymarket_main".to_string(),
            event_key: "sample_event_2026".to_string(),
            market_slug: "will-sample-alpha-resolve-yes".to_string(),
            condition_id: Some("condition-sample-alpha".to_string()),
            yes_outcome: "Yes".to_string(),
            no_outcome: "No".to_string(),
        });
        let root: BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml"))
                .expect("fixture root config should parse");
        let data = root
            .clients
            .get("polymarket_main")
            .expect("fixture must define polymarket_main client")
            .data
            .clone()
            .expect("polymarket_main must carry a [data] block");

        let cfg = map_data(
            &root,
            "polymarket_main",
            &data,
            &plan,
            Arc::new(|| 1_746_000_000),
        )
        .expect("polymarket data mapping should succeed");

        // Bootstrap must be enabled (the bug shipped `None`) and must stay scoped
        // through the runtime `filters` rather than explicit slug/id scope, which
        // would bypass the filters' `accept()` intersection in NT.
        let ic = cfg
            .instrument_config
            .expect("instrument_config must be Some so initialize() performs a bootstrap");
        assert!(
            ic.should_load_all(),
            "instrument_config must trigger a connect-time bootstrap"
        );
        assert!(
            ic.load_all,
            "bootstrap must run via load_all + runtime filters"
        );
        assert!(
            ic.event_slugs.is_none()
                && ic.market_slugs.is_none()
                && ic.event_slug_builder.is_none()
                && ic.load_ids.is_none(),
            "must not set explicit slug/id scope, or NT bypasses the runtime filters"
        );
        assert!(
            !cfg.filters.is_empty(),
            "runtime fetch-only filters must remain populated to scope the bootstrap"
        );
    }
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3PolymarketSecrets, BoltV3AdapterMappingError> {
    match resolved.clients.get(client_key) {
        Some(inner) => inner.as_any().downcast_ref().ok_or_else(|| {
            BoltV3AdapterMappingError::SecretProviderMismatch {
                client_key: client_key.to_string(),
                expected_provider_key: KEY,
            }
        }),
        None => Err(BoltV3AdapterMappingError::MissingResolvedSecrets {
            client_key: client_key.to_string(),
            expected_provider_key: KEY,
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
