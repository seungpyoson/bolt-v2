//! Per-provider binding for the `CHAINLINK_DATA_STREAMS` client block shape
//! and per-client startup validation.
//!
//! Owns the concrete shape of `[clients.<name>.data]` and
//! `[clients.<name>.secrets]` for any client whose `venue = "CHAINLINK_DATA_STREAMS"`
//! NT venue is configured. Core config in `crate::bolt_v3_config` only owns the
//! root/strategy envelope and raw NT venue field; the provider-shaped block
//! types and their serde rules live here so provider-specific schema evolution
//! does not reach back into the envelope module.
//!
//! The Chainlink client is a point-in-time strike (price-to-beat) source: it
//! fetches the Data Streams report AT a window-open timestamp and delivers it
//! as one NT `IndexPriceUpdate`. It is NOT a continuous stream and declares no
//! `[execution]` block. The bolt-owned HMAC request signer never routes
//! credentials through an NT adapter, so this binding contributes no
//! credential-log modules and no forbidden environment variables.

mod auth;
mod report;
mod strike_source;

pub(crate) use auth::{
    chainlink_data_streams_auth_headers, chainlink_data_streams_credentials,
    chainlink_data_streams_report_request_url,
};
pub(crate) use report::{
    CHAINLINK_REPORT_MILLISECONDS_PER_SECOND, ChainlinkDataStreamsReportApiResponse,
    DecodedPriceToBeatReport, PriceToBeatReportBinding, decode_price_to_beat_report,
    is_lowercase_chainlink_feed_id,
};
pub use strike_source::{
    ChainlinkStrikeFeedBinding, ChainlinkStrikeLiveProbeResult, ChainlinkStrikeLiveProbeVerdict,
    ChainlinkStrikeSourceConfig, run_strike_live_probe,
};
pub(crate) use strike_source::{
    ChainlinkStrikeSourceFactory, STRIKE_FETCH_INSTRUMENT_ID_PARAM,
    STRIKE_WINDOW_OPEN_UNIX_SECONDS_PARAM, parse_feed_binding, strike_fetch_request_data_type,
};

use std::{
    any::Any,
    path::Path,
    str::FromStr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use nautilus_core::string::secret::REDACTED;
use nautilus_model::identifiers::{ClientId, InstrumentId};
use serde::Deserialize;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterMappingError, BoltV3ClientAdapterConfig, BoltV3DataClientAdapterConfig,
    },
    bolt_v3_config::{BoltV3RootConfig, ClientBlock, LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_providers::{
        ProviderAdapterMapContext, ProviderCredentialedBlock, ProviderResolvedSecrets,
        ProviderSecretRequirement, ProviderSecretResolveContext, ProviderSsmPathReference,
        ResolvedClientSecrets, SsmSecretResolver,
    },
    bolt_v3_secrets::{
        BoltV3SecretError, check_no_forbidden_credential_env_vars, resolve_bolt_v3_client_secrets,
        resolve_field,
    },
    secrets::SsmResolverSession,
};

/// NT venue identifier for the live Chainlink Data Streams strike-source client.
pub const KEY: &str = "CHAINLINK_DATA_STREAMS";

/// `gate_providers.<id>.provider_kind` discriminator for the resolution oracle
/// backed by Chainlink Data Streams. Core config resolution (`crate::bolt_v3_config`)
/// and core validation (`crate::bolt_v3_validate`) reach this through the neutral
/// re-export in `crate::bolt_v3_providers`, so the provider-key literal stays
/// owned by this binding module.
pub const PROVIDER_KIND: &str = "chainlink_data_streams";
pub const SUPPORTED_MARKET_FAMILIES: &[&str] = &[];
pub const REQUIRED_SECRET_BLOCKS: &[ProviderSecretRequirement] = &[ProviderSecretRequirement {
    block: ProviderCredentialedBlock::Data,
    consumer: "Chainlink Data Streams strike source",
}];
pub const SECRET_FIELD_NAMES: &[&str] = &["api_key_ssm_parameter", "api_secret_ssm_parameter"];
/// Chainlink credentials are consumed by the bolt-owned HMAC request signer in
/// this binding's `auth` submodule, never by an NT adapter, so no NT module can
/// echo them at info level.
pub const CREDENTIAL_LOG_MODULES: &[&str] = &[];
/// The bolt-owned Chainlink signer resolves credentials only from SSM; it never
/// reads any environment variable as a secret fallback.
pub const FORBIDDEN_ENV_VARS: &[&str] = &[];
const STRIKE_LIVE_PROBE_TARGET_CADENCE_SECS_FIELD: &str = "cadence_secs";
const STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD: &str = "unavailable";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkDataConfig {
    pub rest_base_url: String,
    pub report_endpoint_path: String,
    pub http_timeout_secs: u64,
    pub feed_bindings: Vec<toml::Value>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChainlinkSecretsConfig {
    pub api_key_ssm_parameter: String,
    pub api_secret_ssm_parameter: String,
}

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct ResolvedBoltV3ChainlinkSecrets {
    pub api_key: Zeroizing<String>,
    pub api_secret: Zeroizing<String>,
}

impl std::fmt::Debug for ResolvedBoltV3ChainlinkSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedBoltV3ChainlinkSecrets")
            .field("api_key", &REDACTED)
            .field("api_secret", &REDACTED)
            .finish()
    }
}

impl ProviderResolvedSecrets for ResolvedBoltV3ChainlinkSecrets {
    fn provider_key(&self) -> &'static str {
        KEY
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn redaction_values(&self) -> Vec<&str> {
        vec![self.api_key.as_str(), self.api_secret.as_str()]
    }
}

pub fn validate_client(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if client.execution.is_some() {
        errors.push(format!(
            "clients.{key} (provider={KEY}) is not allowed to declare an [execution] block; the Chainlink Data Streams strike source is data-only"
        ));
    }
    if let Some(data) = &client.data {
        match data.clone().try_into::<ChainlinkDataConfig>() {
            Ok(parsed) => errors.extend(validate_data_bounds(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.data: {message}")),
        }
    }
    if let Some(secrets) = &client.secrets {
        if client.data.is_none() {
            errors.push(format!(
                "clients.{key} (provider={KEY}) declares [secrets] but no [data] block is configured; \
                 Chainlink [secrets] are only allowed alongside the data adapter that consumes them"
            ));
        }
        match secrets.clone().try_into::<ChainlinkSecretsConfig>() {
            Ok(parsed) => errors.extend(validate_secret_paths(key, &parsed)),
            Err(message) => errors.push(format!("clients.{key}.secrets: {message}")),
        }
    }
    errors
}

fn validate_data_bounds(key: &str, data: &ChainlinkDataConfig) -> Vec<String> {
    let mut errors = Vec::new();
    if data.rest_base_url.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.data.rest_base_url must be a non-empty URL"
        ));
    } else {
        crate::bolt_v3_validate::validate_https_rest_base_url(
            &format!("clients.{key}.data.rest_base_url"),
            &data.rest_base_url,
            &mut errors,
        );
    }
    if data.report_endpoint_path.trim().is_empty() {
        errors.push(format!(
            "clients.{key}.data.report_endpoint_path must be a non-empty path"
        ));
    } else {
        crate::bolt_v3_validate::validate_chainlink_report_endpoint_path(
            &format!("clients.{key}.data.report_endpoint_path"),
            &data.rest_base_url,
            &data.report_endpoint_path,
            &mut errors,
        );
    }
    if data.http_timeout_secs == 0 {
        errors.push(format!(
            "clients.{key}.data.http_timeout_secs must be a positive integer"
        ));
    }
    if data.feed_bindings.is_empty() {
        errors.push(format!(
            "clients.{key}.data.feed_bindings must declare at least one feed-to-instrument binding"
        ));
    }
    let mut seen_feed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_instrument_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for (index, binding) in data.feed_bindings.iter().enumerate() {
        match parse_feed_binding(key, index, binding) {
            Ok(parsed) => {
                if !seen_feed_ids.insert(parsed.feed_id.clone()) {
                    errors.push(format!(
                        "clients.{key}.data.feed_bindings[{index}].feed_id duplicates an earlier binding; each feed_id must map to exactly one instrument_id"
                    ));
                }
                let instrument_id = parsed.instrument_id.to_string();
                if !seen_instrument_ids.insert(instrument_id) {
                    errors.push(format!(
                        "clients.{key}.data.feed_bindings[{index}].instrument_id duplicates an earlier binding; each instrument_id must map to exactly one feed_id"
                    ));
                }
            }
            Err(message) => errors.push(message),
        }
    }
    errors
}

fn validate_secret_paths(key: &str, secrets: &ChainlinkSecretsConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let path_fields: &[(&str, &str)] = &[
        ("api_key_ssm_parameter", &secrets.api_key_ssm_parameter),
        (
            "api_secret_ssm_parameter",
            &secrets.api_secret_ssm_parameter,
        ),
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
    let api_key = resolve_field(
        context.client_key,
        "api_key_ssm_parameter",
        context.region,
        &secrets.api_key_ssm_parameter,
        resolver,
    )?;
    let api_secret = resolve_field(
        context.client_key,
        "api_secret_ssm_parameter",
        context.region,
        &secrets.api_secret_ssm_parameter,
        resolver,
    )?;
    Ok(Arc::new(ResolvedBoltV3ChainlinkSecrets {
        api_key: Zeroizing::new(api_key),
        api_secret: Zeroizing::new(api_secret),
    }))
}

pub fn configured_secret_paths(
    context: ProviderSecretResolveContext<'_>,
) -> Result<Vec<ProviderSsmPathReference>, BoltV3SecretError> {
    let secrets = parse_secrets_config(&context)?;
    Ok(vec![
        ProviderSsmPathReference {
            field_name: "api_key_ssm_parameter",
            ssm_path: secrets.api_key_ssm_parameter,
        },
        ProviderSsmPathReference {
            field_name: "api_secret_ssm_parameter",
            ssm_path: secrets.api_secret_ssm_parameter,
        },
    ])
}

fn parse_secrets_config(
    context: &ProviderSecretResolveContext<'_>,
) -> Result<ChainlinkSecretsConfig, BoltV3SecretError> {
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
            source: format!("invalid chainlink secrets schema: {error}"),
        })
}

pub fn map_adapters(
    context: ProviderAdapterMapContext<'_>,
) -> Result<BoltV3ClientAdapterConfig, BoltV3AdapterMappingError> {
    let data = match &context.client.data {
        Some(value) => {
            let secrets = secrets_for(context.client_key, context.resolved)?;
            Some(BoltV3DataClientAdapterConfig {
                factory: Box::new(ChainlinkStrikeSourceFactory),
                config: Box::new(map_data(context.root, context.client_key, value, secrets)?),
            })
        }
        None => None,
    };
    Ok(BoltV3ClientAdapterConfig {
        data,
        execution: None,
    })
}

pub fn strike_source_config(
    root: &BoltV3RootConfig,
    client_key: &str,
    client: &ClientBlock,
    resolved: &crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<ChainlinkStrikeSourceConfig, BoltV3AdapterMappingError> {
    let data = client
        .data
        .as_ref()
        .ok_or_else(|| BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: "Chainlink strike live probe requires the selected client to declare [data]"
                .to_string(),
        })?;
    let secrets = secrets_for(client_key, resolved)?;
    map_data(root, client_key, data, secrets)
}

pub fn run_strike_live_probe_command(
    config: &Path,
    client_key: &str,
    requested_instrument_id: Option<&str>,
    requested_window_open_unix_seconds: Option<u64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let loaded = load_bolt_v3_config(config)?;
    check_no_forbidden_credential_env_vars(&loaded.root)?;
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("strike-live-probe client_key `{client_key}` is not configured"),
        )
    })?;
    let ssm_resolver_session = SsmResolverSession::new()?;
    let resolved = resolve_bolt_v3_client_secrets(&ssm_resolver_session, &loaded, client_key)?;
    let strike_config = strike_source_config(&loaded.root, client_key, client, &resolved)?;
    let instrument_id =
        selected_strike_live_probe_instrument_id(&strike_config, requested_instrument_id)?;
    let window_open_unix_seconds = match requested_window_open_unix_seconds {
        Some(window_open_unix_seconds) => window_open_unix_seconds,
        None => {
            let cadence_secs =
                configured_strike_live_probe_cadence_secs(&loaded, client_key, instrument_id)?;
            recent_already_open_boundary_unix_seconds(cadence_secs)?
        }
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let report = runtime.block_on(run_strike_live_probe(
        &strike_config,
        instrument_id,
        window_open_unix_seconds,
    ));
    print_strike_live_probe_report(&report);
    if report.is_pass() {
        Ok(())
    } else {
        Err("Chainlink strike live probe failed".into())
    }
}

fn selected_strike_live_probe_instrument_id(
    config: &ChainlinkStrikeSourceConfig,
    requested_instrument_id: Option<&str>,
) -> Result<InstrumentId, Box<dyn std::error::Error>> {
    if let Some(requested_instrument_id) = requested_instrument_id {
        let instrument_id = InstrumentId::from_str(requested_instrument_id)?;
        if config
            .feed_bindings
            .iter()
            .any(|binding| binding.instrument_id == instrument_id)
        {
            return Ok(instrument_id);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "strike-live-probe instrument_id `{requested_instrument_id}` has no feed binding"
            ),
        )
        .into());
    }
    match config.feed_bindings.as_slice() {
        [binding] => Ok(binding.instrument_id),
        [] => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "strike-live-probe requires at least one Chainlink strike feed binding",
        )
        .into()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "strike-live-probe requires --instrument-id when multiple feed bindings are configured",
        )
        .into()),
    }
}

fn configured_strike_live_probe_cadence_secs(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
    instrument_id: InstrumentId,
) -> Result<u64, Box<dyn std::error::Error>> {
    let client_id = ClientId::from(client_key);
    let mut cadences = Vec::new();
    for strategy in &loaded.strategies {
        let Some(resolution_data) = &strategy.config.resolution_data else {
            continue;
        };
        if resolution_data.data_client_id != client_id
            || resolution_data.instrument_id != instrument_id
        {
            continue;
        }
        let Some(cadence_secs) = strategy
            .config
            .target
            .get(STRIKE_LIVE_PROBE_TARGET_CADENCE_SECS_FIELD)
            .and_then(toml::Value::as_integer)
            .and_then(|value| u64::try_from(value).ok())
            .and_then(std::num::NonZeroU64::new)
            .map(std::num::NonZeroU64::get)
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "{} target.cadence_secs must be a positive integer",
                    strategy.relative_path
                ),
            )
            .into());
        };
        if !cadences.contains(&cadence_secs) {
            cadences.push(cadence_secs);
        }
    }
    match cadences.as_slice() {
        [cadence_secs] => Ok(*cadence_secs),
        [] => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "strike-live-probe could not derive target.cadence_secs for client_key={client_key} instrument_id={instrument_id}"
            ),
        )
        .into()),
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "strike-live-probe found multiple target.cadence_secs values for client_key={client_key} instrument_id={instrument_id}: {cadences:?}"
            ),
        )
        .into()),
    }
}

fn recent_already_open_boundary_unix_seconds(
    cadence_secs: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    if cadence_secs == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "strike-live-probe cadence_secs must be positive",
        )
        .into());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "system clock is before Unix epoch",
            )
        })?
        .as_secs();
    recent_already_open_boundary_unix_seconds_for_now(now, cadence_secs)
}

fn recent_already_open_boundary_unix_seconds_for_now(
    now_unix_seconds: u64,
    cadence_secs: u64,
) -> Result<u64, Box<dyn std::error::Error>> {
    if cadence_secs == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "strike-live-probe cadence_secs must be positive",
        )
        .into());
    }
    let current_boundary = now_unix_seconds - (now_unix_seconds % cadence_secs);
    current_boundary.checked_sub(cadence_secs).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "strike-live-probe cannot select an already-open boundary before Unix epoch",
        )
        .into()
    })
}

fn print_strike_live_probe_report(report: &ChainlinkStrikeLiveProbeResult) {
    println!(
        "REQUESTED window_open_unix_seconds={} feed_id={} instrument_id={}",
        report.requested_window_open_unix_seconds, report.feed_id, report.instrument_id
    );
    println!(
        "HTTP status={}",
        report
            .http_status
            .map(|status| status.to_string())
            .unwrap_or_else(|| STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD.to_string())
    );
    println!(
        "DECODED validFrom_ms={} benchmark_price={}",
        report
            .decoded_valid_from_timestamp_ms
            .map(|timestamp| timestamp.to_string())
            .unwrap_or_else(|| STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD.to_string()),
        report
            .decoded_benchmark_price
            .map(|price| price.to_string())
            .unwrap_or_else(|| STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD.to_string())
    );
    println!(
        "OFFSET = {}",
        report
            .offset_ms
            .map(|offset| offset.to_string())
            .unwrap_or_else(|| STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD.to_string())
    );
    match &report.verdict {
        ChainlinkStrikeLiveProbeVerdict::Pass => println!("VERDICT: PASS"),
        ChainlinkStrikeLiveProbeVerdict::Fail { reason } => println!(
            "VERDICT: FAIL reason={} offset={}",
            reason,
            report
                .offset_ms
                .map(|offset| offset.to_string())
                .unwrap_or_else(|| STRIKE_LIVE_PROBE_UNAVAILABLE_FIELD.to_string())
        ),
    }
}

pub fn reference_price_instrument_in_shared_catalog(
    root: &BoltV3RootConfig,
    instrument_id: &str,
) -> Result<bool, String> {
    let Some(catalog) = root.chainlink_data_streams.as_ref() else {
        return Ok(false);
    };
    for (index, binding) in catalog.feed_bindings.iter().enumerate() {
        let binding = parse_feed_binding(KEY, index, binding)?;
        if binding.instrument_id.to_string() == instrument_id {
            return Ok(true);
        }
    }
    Ok(false)
}

fn map_data(
    root: &BoltV3RootConfig,
    client_key: &str,
    value: &toml::Value,
    secrets: &ResolvedBoltV3ChainlinkSecrets,
) -> Result<ChainlinkStrikeSourceConfig, BoltV3AdapterMappingError> {
    let value = data_value_with_root_feed_catalog(root, client_key, value)?;
    let cfg: ChainlinkDataConfig = value.try_into().map_err(|error: toml::de::Error| {
        BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: error.to_string(),
        }
    })?;
    let validation_errors = validate_data_bounds(client_key, &cfg);
    if let Some(message) = validation_errors.into_iter().next() {
        return Err(BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        });
    }
    let feed_bindings = cfg
        .feed_bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| parse_feed_binding(client_key, index, binding))
        .collect::<Result<Vec<ChainlinkStrikeFeedBinding>, String>>()
        .map_err(|message| BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message,
        })?;
    Ok(ChainlinkStrikeSourceConfig {
        rest_base_url: cfg.rest_base_url,
        report_endpoint_path: cfg.report_endpoint_path,
        http_timeout_secs: cfg.http_timeout_secs,
        feed_bindings,
        api_key: secrets.api_key.clone(),
        api_secret: secrets.api_secret.clone(),
    })
}

fn data_value_with_root_feed_catalog(
    root: &BoltV3RootConfig,
    client_key: &str,
    value: &toml::Value,
) -> Result<toml::Value, BoltV3AdapterMappingError> {
    let table = value
        .as_table()
        .ok_or_else(|| BoltV3AdapterMappingError::SchemaParse {
            client_key: client_key.to_string(),
            block: "data",
            message: "expected a TOML table".to_string(),
        })?;
    if table.contains_key("feed_bindings") {
        return Err(BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "data.feed_bindings",
            message: format!(
                "chainlink_data_streams.feed_bindings is root-owned; clients.{client_key}.data.feed_bindings must be removed"
            ),
        });
    }
    let catalog = root.chainlink_data_streams.as_ref().ok_or_else(|| {
        BoltV3AdapterMappingError::ValidationInvariant {
            client_key: client_key.to_string(),
            field: "chainlink_data_streams.feed_bindings",
            message: format!(
                "chainlink_data_streams.feed_bindings must be configured for clients.{client_key}"
            ),
        }
    })?;

    let mut table = table.clone();
    table.insert(
        "feed_bindings".to_string(),
        toml::Value::Array(catalog.feed_bindings.clone()),
    );
    Ok(toml::Value::Table(table))
}

fn secrets_for<'a>(
    client_key: &str,
    resolved: &'a crate::bolt_v3_secrets::ResolvedBoltV3Secrets,
) -> Result<&'a ResolvedBoltV3ChainlinkSecrets, BoltV3AdapterMappingError> {
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

/// F3 single-source guard: when both a live `CHAINLINK_DATA_STREAMS` data
/// client and a `chainlink_data_streams` gate provider are configured, their
/// shared connection config (REST endpoint + SSM credential paths) must match
/// exactly, so the offline-evidence path and the live strike path cannot
/// silently drift onto different endpoints/testnets/credentials. Fails closed
/// on any divergence.
///
/// Feed bindings are intentionally NOT compared here: the live client maps
/// `feed_id -> instrument_id` while the gate provider maps
/// `feed_id -> resolution_identity`, so their shapes differ. Full single-source
/// dedup (deriving the live client from the gate provider and removing the
/// duplicate block) is tracked with the #551 gate-provider/seed removal.
///
/// Owned by this provider binding because it deserializes the concrete
/// `ChainlinkDataConfig`/`ChainlinkSecretsConfig` block shapes; core validation
/// reaches it through the neutral `validate_resolution_oracle_client_consistency`
/// seam in `crate::bolt_v3_providers`.
pub(crate) fn validate_client_gate_provider_consistency(root: &BoltV3RootConfig) -> Vec<String> {
    use crate::bolt_v3_validate::{
        CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
        CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
        CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD,
        CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
        CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
    };

    let mut errors = Vec::new();

    let chainlink_gate_providers: Vec<(&String, &toml::Value)> = match &root.gate_providers {
        Some(providers) => providers
            .iter()
            .filter(|(_, provider)| provider.provider_kind.as_deref() == Some(PROVIDER_KIND))
            .filter_map(|(id, provider)| {
                provider
                    .provider_config
                    .get(PROVIDER_KIND)
                    .map(|table| (id, table))
            })
            .collect(),
        None => Vec::new(),
    };

    for (client_id, client) in &root.clients {
        if client.venue.as_str() != KEY {
            continue;
        }
        let validation_client =
            crate::bolt_v3_validate::client_with_root_chainlink_feed_catalog(root, client);
        let client = validation_client.as_ref().unwrap_or(client);
        let Some(data_value) = client.data.as_ref() else {
            continue;
        };
        // Per-client shape errors are reported by the provider validator; only
        // run the cross-check when the client config parses cleanly.
        let Ok(data) = data_value.clone().try_into::<ChainlinkDataConfig>() else {
            continue;
        };

        if chainlink_gate_providers.is_empty() {
            // No gate provider to drift against (e.g. the post-#551 end state).
            continue;
        }
        if chainlink_gate_providers.len() > 1 {
            errors.push(format!(
                "clients.{client_id} (CHAINLINK_DATA_STREAMS) cannot be consistency-checked against the resolution oracle: {} `chainlink_data_streams` gate providers are configured, expected exactly one",
                chainlink_gate_providers.len()
            ));
            continue;
        }

        let (gate_provider_id, gate_table_value) = chainlink_gate_providers[0];
        let Some(gate_table) = gate_table_value.as_table() else {
            // A malformed gate-provider table is reported by validate_gate_providers.
            continue;
        };

        // Section-scoped closures so a divergence error names the TOML subtable
        // the field actually lives in: `data` for the endpoint fields, `secrets`
        // for the SSM credential parameters (which live under
        // [clients.<id>.secrets], not [clients.<id>.data]). The section word
        // stays inside the message template — same shape, two prefixes.
        let check_data_str = |field: &str, client_value: &str| -> Option<String> {
            let gate_value = gate_table.get(field).and_then(toml::Value::as_str);
            if gate_value == Some(client_value) {
                None
            } else {
                Some(format!(
                    "clients.{client_id}.data.{field} (`{client_value}`) must match gate_providers.{gate_provider_id}.chainlink_data_streams.{field} (`{}`); the live strike client and the resolution-oracle gate provider must reference one source",
                    gate_value.unwrap_or("<missing>")
                ))
            }
        };
        let check_secrets_str = |field: &str, client_value: &str| -> Option<String> {
            let gate_value = gate_table.get(field).and_then(toml::Value::as_str);
            if gate_value == Some(client_value) {
                None
            } else {
                Some(format!(
                    "clients.{client_id}.secrets.{field} (`{client_value}`) must match gate_providers.{gate_provider_id}.chainlink_data_streams.{field} (`{}`); the live strike client and the resolution-oracle gate provider must reference one source",
                    gate_value.unwrap_or("<missing>")
                ))
            }
        };

        errors.extend(check_data_str(
            CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
            &data.rest_base_url,
        ));
        errors.extend(check_data_str(
            CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
            &data.report_endpoint_path,
        ));

        let gate_timeout = gate_table
            .get(CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD)
            .and_then(toml::Value::as_integer);
        if gate_timeout != Some(data.http_timeout_secs as i64) {
            errors.push(format!(
                "clients.{client_id}.data.http_timeout_secs ({}) must match gate_providers.{gate_provider_id}.chainlink_data_streams.http_timeout_secs ({}); the live strike client and the resolution-oracle gate provider must reference one source",
                data.http_timeout_secs,
                gate_timeout
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "<missing>".to_string())
            ));
        }

        if let Some(secrets_value) = client.secrets.as_ref()
            && let Ok(secrets) = secrets_value.clone().try_into::<ChainlinkSecretsConfig>()
        {
            errors.extend(check_secrets_str(
                CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
                &secrets.api_key_ssm_parameter,
            ));
            errors.extend(check_secrets_str(
                CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
                &secrets.api_secret_ssm_parameter,
            ));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    //! Feed-binding uniqueness validation.
    //!
    //! The `[clients.<id>.data].feed_bindings` table maps each Chainlink Data
    //! Streams `feed_id` to exactly one NT resolution `instrument_id`. The live
    //! strike lookup in this binding's `strike_source` submodule resolves a binding by
    //! `.find(|b| b.instrument_id == instrument_id)` (first match wins), so a
    //! duplicate `feed_id` or `instrument_id` silently shadows the second entry
    //! — a misconfiguration that must fail closed at config validation rather
    //! than mapping live money onto the wrong feed.

    use super::*;

    // Two distinct valid Chainlink Data Streams feed ids (0x + 64 lowercase hex)
    // and two distinct valid NT instrument ids, so that each fixture varies only
    // the dimension under test (duplicate feed_id XOR duplicate instrument_id).
    const FEED_ID_A: &str = "0x000362205e10b3a147d02792eccee483dca6c7b44ecce7012cb8c6e0b68b3ae9";
    const FEED_ID_B: &str = "0x0003111111111111111111111111111111111111111111111111111111111111";
    const INSTRUMENT_ID_A: &str = "BTC-USD-UP.BOLT";
    const INSTRUMENT_ID_B: &str = "BTC-USD-DOWN.BOLT";

    fn data_config_from_bindings(bindings_toml: &str) -> ChainlinkDataConfig {
        data_config_with_rest_base_url("https://example.invalid", bindings_toml)
    }

    fn data_config_with_rest_base_url(
        rest_base_url: &str,
        bindings_toml: &str,
    ) -> ChainlinkDataConfig {
        let toml_src = format!(
            r#"
rest_base_url = "{rest_base_url}"
report_endpoint_path = "/api/v1/reports"
http_timeout_secs = 5
{bindings_toml}
"#
        );
        toml::from_str::<ChainlinkDataConfig>(&toml_src).expect("fixture data config must parse")
    }

    fn binding_table(feed_id: &str, instrument_id: &str) -> String {
        format!(
            r#"
[[feed_bindings]]
feed_id = "{feed_id}"
instrument_id = "{instrument_id}"
report_schema_version = 3
report_decimal_scale = 18
price_precision = 2
"#
        )
    }

    #[test]
    fn rejects_duplicate_feed_id_in_feed_bindings() {
        // Same feed_id on two bindings (distinct instrument_ids): ambiguous which
        // instrument a single feed's strike resolves onto.
        let bindings = format!(
            "{}{}",
            binding_table(FEED_ID_A, INSTRUMENT_ID_A),
            binding_table(FEED_ID_A, INSTRUMENT_ID_B),
        );
        let data = data_config_from_bindings(&bindings);

        let errors = validate_data_bounds("chainlink_strike", &data);

        assert!(
            !errors.is_empty(),
            "duplicate feed_id across feed_bindings must be rejected at validation; got no errors"
        );
        assert!(
            errors.iter().any(|e| e.contains("feed_id")),
            "expected a duplicate-feed_id error mentioning `feed_id`, got: {errors:?}"
        );
    }

    #[test]
    fn rejects_duplicate_instrument_id_in_feed_bindings() {
        // Same instrument_id on two bindings (distinct feed_ids): the
        // first-match-wins lookup silently ignores the second feed.
        let bindings = format!(
            "{}{}",
            binding_table(FEED_ID_A, INSTRUMENT_ID_A),
            binding_table(FEED_ID_B, INSTRUMENT_ID_A),
        );
        let data = data_config_from_bindings(&bindings);

        let errors = validate_data_bounds("chainlink_strike", &data);

        assert!(
            !errors.is_empty(),
            "duplicate instrument_id across feed_bindings must be rejected at validation; got no errors"
        );
        assert!(
            errors.iter().any(|e| e.contains("instrument_id")),
            "expected a duplicate-instrument_id error mentioning `instrument_id`, got: {errors:?}"
        );
    }

    #[test]
    fn rejects_non_https_rest_base_url() {
        // The strike fetch signs each request with HMAC credentials and sends
        // them as auth headers (strike_source.rs). Over an http:// base URL
        // those credentials would travel in plaintext, so config validation
        // must fail closed on any non-https scheme — not merely on an
        // unparseable URL (an http:// URL parses fine).
        let data = data_config_with_rest_base_url(
            "http://example.invalid",
            &binding_table(FEED_ID_A, INSTRUMENT_ID_A),
        );

        let errors = validate_data_bounds("chainlink_strike", &data);

        assert!(
            errors.iter().any(|e| e.contains("https")),
            "expected an https-scheme rejection for an http:// rest_base_url, got: {errors:?}"
        );
    }

    #[test]
    fn strike_live_probe_derives_btc_cadence_from_loaded_resolution_data() {
        let loaded = load_bolt_v3_config(std::path::Path::new("config/root.toml"))
            .expect("root config should load");
        let cadence_secs = configured_strike_live_probe_cadence_secs(
            &loaded,
            "chainlink_strike",
            InstrumentId::from("BTC-USD.CHAINLINK"),
        )
        .expect("BTC strike cadence should derive from config");

        assert_eq!(cadence_secs, 300);
    }

    #[test]
    fn strike_live_probe_selects_previous_already_open_boundary() {
        let boundary = recent_already_open_boundary_unix_seconds_for_now(1_700_000_650, 300)
            .expect("boundary should be selected");

        assert_eq!(boundary, 1_700_000_100);
    }
}
