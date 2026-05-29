//! Startup-shaping validation for bolt-v3 root and strategy configs.
//!
//! Schema rules: docs/bolt-v3/2026-04-25-bolt-v3-schema.md Section 8.
//!
//! This module owns common strategy-envelope validation (schema
//! version, uniqueness of instance / order-id-tag, client / execution
//! lookup, per-role reference-data structural validation, reference quote
//! probe source disambiguation), root-block validation, and root risk
//! decimal syntax only. Market-family-shaped
//! target rules
//! (rotating-market kind, family discriminator, cadence policy,
//! underlying-asset shape, retry / blocked timers, market-selection
//! rule) are owned by the per-family binding modules under
//! `crate::bolt_v3_market_families`; `validate_strategies` dispatches
//! the strategy envelope's raw `[target]` value through
//! `crate::bolt_v3_market_families::validate_strategy_target`. Strategy-
//! archetype-specific rules (required reference-data roles, allowed
//! `[parameters.entry_order]` / `[parameters.exit_order]` combinations,
//! archetype-specific error wording) are owned by the per-archetype
//! binding modules under `crate::bolt_v3_archetypes`; those modules also
//! own archetype parameter bounds such as parameter decimal syntax and
//! root-cap comparison. `validate_strategies` dispatches into the
//! matching archetype validator via
//! `crate::bolt_v3_archetypes::validate_strategy_archetype`.
//! Per-provider venue-block validation (provider-shaped
//! `[clients.<id>.{data,execution,secrets}]` rules: typed
//! deserialization, cross-block presence rules, provider data /
//! execution bounds, EVM funder-address syntax, provider secret-path
//! ownership) is owned by the per-provider binding modules under
//! `crate::bolt_v3_providers`; `validate_clients_block` dispatches each
//! client block through `crate::bolt_v3_providers::validate_client_block`.
//! Only the genuinely provider-neutral SSM parameter-path utility
//! (`validate_ssm_parameter_path`) stays in this module and is exposed
//! `pub(crate)` so the per-provider secret validators can call it the
//! same way the archetype binding calls `parse_decimal_string`.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::Path,
    str::FromStr,
};

use nautilus_model::{
    enums::{BarAggregation, BarIntervalType},
    identifiers::{ClientOrderId, InstrumentId},
};
use rust_decimal::Decimal;

use crate::bolt_v3_config::{
    AwsBlock, BoltV3RootConfig, BoltV3StrategyConfig, CHAINLINK_DATA_STREAMS_PROVIDER_KIND,
    ClientBlock, DataClientReadinessProbeQuoteTargetSource, GATE_PROVIDER_CAPABILITIES,
    GATE_PROVIDER_KINDS, GateProviderBlock, GateProviderFreshnessBlock, LiveCanaryProofPolicyBlock,
    LoadedStrategy, NautilusBlock, PRICE_GATE_VALUE_KIND, PersistenceBlock, RiskBlock,
    SSM_CREDENTIAL_PARAMETER_FIELD, TEST_DOUBLE_PROVIDER_KIND,
};

#[derive(Debug)]
pub struct BoltV3ValidationError {
    messages: Vec<String>,
}

impl BoltV3ValidationError {
    pub fn new(messages: Vec<String>) -> Self {
        Self { messages }
    }

    pub fn messages(&self) -> &[String] {
        &self.messages
    }
}

impl std::fmt::Display for BoltV3ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "bolt-v3 config validation failed ({} error{}):",
            self.messages.len(),
            if self.messages.len() == 1 { "" } else { "s" }
        )?;
        for message in &self.messages {
            writeln!(f, "  - {message}")?;
        }
        Ok(())
    }
}

impl std::error::Error for BoltV3ValidationError {}

pub const SUPPORTED_ROOT_SCHEMA_VERSION: u32 = 1;
pub const SUPPORTED_STRATEGY_SCHEMA_VERSION: u32 = 2;
const CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD: &str = "feed_bindings";
const CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD: &str = "endpoint_id";
const CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD: &str = "rest_base_url";
const CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD: &str = "report_endpoint_path";
const CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD: &str = "http_timeout_secs";
const CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD: &str = "api_key_ssm_parameter";
const CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD: &str = "api_secret_ssm_parameter";
const CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD: &str = "ssm_credential_parameter";
const CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD: &str = "value_kind";
const CHAINLINK_DATA_STREAMS_FEED_ID_FIELD: &str = "feed_id";
const CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD: &str = "report_schema_version";
const CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD: &str = "report_decimal_scale";
const CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS: &[&str] = &[
    CHAINLINK_DATA_STREAMS_FEED_ID_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD,
];
const CHAINLINK_DATA_STREAMS_PROVIDER_FIELDS: &[&str] = &[
    CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD,
    CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
    CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
    CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD,
    CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
    CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
    CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD,
    CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD,
];
const TARGET_GATE_SUBSCRIPTIONS_FIELD: &str = "gate_subscriptions";
const TARGET_MARKET_MAPPINGS_FIELD: &str = "market_mappings";
const TARGET_RESOLUTION_KIND_FIELD: &str = "resolution_kind";
const TARGET_PROVIDER_ID_FIELD: &str = "provider_id";
const TARGET_PROVIDER_PREFERENCE_FIELD: &str = "provider_preference";
const TARGET_ALLOWED_PROVIDER_IDS_FIELD: &str = "allowed_provider_ids";
const LIVE_CANARY_PROOF_POLICY_KIND: &str = "least_bad_strategy_candidate";
const LIVE_CANARY_PROOF_POLICY_CANDIDATE_SCORE_SOURCE: &str = "strategy_evidence";
const LIVE_CANARY_PROOF_POLICY_NOTIONAL_MODE: &str = "fixed";
const LIVE_CANARY_PROOF_POLICY_REQUIRED_PROOF_CLAIM: &str = "proof_only";

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ChainlinkFeedBindingKey {
    provider_id: String,
    resolution_identity: String,
    value_kind: String,
}

#[derive(Debug, Clone)]
struct ChainlinkTargetMappingReference {
    key: ChainlinkFeedBindingKey,
    context: String,
}

pub fn validate_root_only(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if root.schema_version != SUPPORTED_ROOT_SCHEMA_VERSION {
        errors.push(format!(
            "root schema_version={} is unsupported by this build (only {} is currently supported)",
            root.schema_version, SUPPORTED_ROOT_SCHEMA_VERSION
        ));
    }
    if root.strategy_files.is_empty() {
        errors.push("strategy_files must list at least one strategy file".to_string());
    }
    // FINDING-1: NT's `Environment` has Backtest/Sandbox/Live; bolt-v3 is a
    // live-trading LiveNode wrapper and must reject the other variants
    // explicitly rather than booting NT's kernel in an unsupported mode.
    if root.runtime.mode != nautilus_common::enums::Environment::Live {
        errors.push(format!(
            "runtime.mode `{:?}` is not supported by bolt-v3 (only Live is implemented)",
            root.runtime.mode
        ));
    }
    errors.extend(validate_nautilus_block(&root.nautilus));
    errors.extend(validate_risk_block(&root.risk));
    errors.extend(validate_persistence_block(&root.persistence));
    errors.extend(validate_aws_block(&root.aws));
    errors.extend(validate_clients_block(&root.clients));
    if let Some(gate_providers) = &root.gate_providers {
        errors.extend(validate_gate_providers(gate_providers, &root.clients));
    }
    if let Some(live_canary) = root.live_canary.as_ref() {
        if let Some(proof_policy) = live_canary.proof_policy.as_ref() {
            errors.extend(validate_live_canary_proof_policy(
                proof_policy,
                &live_canary.max_notional_per_order,
            ));
        }
    }

    errors
}

fn validate_live_canary_proof_policy(
    policy: &LiveCanaryProofPolicyBlock,
    max_notional_per_order: &str,
) -> Vec<String> {
    let mut errors = Vec::new();

    if policy.enabled && policy.proof_claim.trim() != LIVE_CANARY_PROOF_POLICY_REQUIRED_PROOF_CLAIM
    {
        errors.push(format!(
            "live_canary.proof_policy.proof_claim must be `{}` when enabled",
            LIVE_CANARY_PROOF_POLICY_REQUIRED_PROOF_CLAIM
        ));
    }
    if policy.enabled && policy.policy_kind.trim() != LIVE_CANARY_PROOF_POLICY_KIND {
        errors.push(format!(
            "live_canary.proof_policy.policy_kind must be `{}` when enabled",
            LIVE_CANARY_PROOF_POLICY_KIND
        ));
    }
    if policy.enabled && policy.notional_mode.trim() != LIVE_CANARY_PROOF_POLICY_NOTIONAL_MODE {
        errors.push(format!(
            "live_canary.proof_policy.notional_mode must be `{}` when enabled",
            LIVE_CANARY_PROOF_POLICY_NOTIONAL_MODE
        ));
    }
    if policy.enabled
        && policy.candidate_score_source.trim() != LIVE_CANARY_PROOF_POLICY_CANDIDATE_SCORE_SOURCE
    {
        errors.push(format!(
            "live_canary.proof_policy.candidate_score_source must be `{}` when enabled",
            LIVE_CANARY_PROOF_POLICY_CANDIDATE_SCORE_SOURCE
        ));
    }
    if policy.enabled && policy.strategy_instance_id.trim().is_empty() {
        errors.push(
            "live_canary.proof_policy.strategy_instance_id must not be blank when enabled"
                .to_string(),
        );
    }
    if policy.enabled && policy.execution_client_id.trim().is_empty() {
        errors.push(
            "live_canary.proof_policy.execution_client_id must not be blank when enabled"
                .to_string(),
        );
    }

    match (
        parse_decimal_string(&policy.proof_notional),
        parse_decimal_string(max_notional_per_order),
    ) {
        (Ok(proof_notional), Ok(max_notional)) => {
            if proof_notional <= Decimal::ZERO {
                errors.push("live_canary.proof_policy.proof_notional must be positive".to_string());
            }
            if proof_notional > max_notional {
                errors.push(
                    "live_canary.proof_policy.proof_notional must be <= live_canary.max_notional_per_order"
                        .to_string(),
                );
            }
        }
        (Err(reason), _) => errors.push(format!(
            "live_canary.proof_policy.proof_notional is invalid: {reason}"
        )),
        (_, Err(reason)) => errors.push(format!(
            "live_canary.max_notional_per_order is invalid: {reason}"
        )),
    }

    if policy.enabled && policy.rotation_min_distinct_markets == 0 {
        errors.push(
            "live_canary.proof_policy.rotation_min_distinct_markets must be positive".to_string(),
        );
    }
    if policy.enabled && policy.rotation_max_attempts == 0 {
        errors.push("live_canary.proof_policy.rotation_max_attempts must be positive".to_string());
    }
    if policy.enabled && policy.rotation_min_distinct_markets > policy.rotation_max_attempts {
        errors.push(
            "live_canary.proof_policy.rotation_min_distinct_markets must be <= rotation_max_attempts"
                .to_string(),
        );
    }

    errors
}

fn validate_gate_providers(
    providers: &BTreeMap<String, GateProviderBlock>,
    clients: &BTreeMap<String, ClientBlock>,
) -> Vec<String> {
    let mut errors = Vec::new();

    for (provider_id, provider) in providers {
        let context = format!("gate_providers.{provider_id}");
        let provider_kind = match provider.provider_kind.as_deref() {
            Some(value) if GATE_PROVIDER_KINDS.contains(&value) => Some(value),
            Some(value) => {
                errors.push(format!(
                    "{context}.provider_kind `{value}` is unregistered; supported gate provider kinds are {GATE_PROVIDER_KINDS:?}"
                ));
                None
            }
            None => {
                errors.push(format!("{context}.provider_kind is required"));
                None
            }
        };

        if matches!(provider_kind, Some(kind) if kind == TEST_DOUBLE_PROVIDER_KIND) {
            errors.push(format!(
                "{context}.provider_kind `test_double` is test-only and is not allowed in live/local operator TOML"
            ));
        }

        match &provider.capabilities {
            Some(capabilities) if capabilities.is_empty() => {
                errors.push(format!(
                    "{context}.capabilities must contain one or more semantic capabilities"
                ));
            }
            Some(capabilities) => {
                for capability in capabilities {
                    if !GATE_PROVIDER_CAPABILITIES.contains(&capability.as_str()) {
                        errors.push(format!(
                            "{context}.capabilities contains unregistered capability `{capability}`; supported capabilities are {GATE_PROVIDER_CAPABILITIES:?}"
                        ));
                    }
                }
            }
            None => errors.push(format!(
                "{context}.capabilities must contain one or more semantic capabilities"
            )),
        }

        match &provider.freshness {
            Some(freshness) => errors.extend(validate_gate_provider_freshness(
                &format!("{context}.freshness"),
                freshness,
            )),
            None => errors.push(format!("{context}.freshness is required")),
        }

        if let Some(client_id) = &provider.client_id
            && !clients.contains_key(client_id.as_str())
        {
            errors.push(format!(
                "{context}.client_id `{client_id}` does not match any [clients.<id>] block"
            ));
        }

        if let Some(kind) = provider_kind {
            let expected_table = format!("[{context}.{kind}]");
            match provider.provider_config.get(kind) {
                Some(value) if value.as_table().is_some() => {}
                _ => errors.push(format!(
                    "{context} with provider_kind `{kind}` must define exactly one matching provider-specific subtable {expected_table}"
                )),
            }
            if provider.provider_config.len() != 1 {
                errors.push(format!(
                    "{context} with provider_kind `{kind}` must define exactly one provider-specific subtable; expected {expected_table}"
                ));
            }
            for table_name in provider.provider_config.keys() {
                if table_name != kind {
                    errors.push(format!(
                        "{context} has provider-specific subtable [gate_providers.{provider_id}.{table_name}] but provider_kind `{kind}` requires {expected_table}"
                    ));
                }
            }
            if kind == CHAINLINK_DATA_STREAMS_PROVIDER_KIND
                && let Some(table) = provider
                    .provider_config
                    .get(kind)
                    .and_then(toml::Value::as_table)
            {
                errors.extend(validate_chainlink_data_streams_gate_provider(
                    &context, table,
                ));
            }
        }

        for (table_name, value) in &provider.provider_config {
            if let Some(table) = value.as_table()
                && let Some(parameter) = table.get(SSM_CREDENTIAL_PARAMETER_FIELD)
            {
                match parameter.as_str() {
                    Some(path) => errors.extend(validate_gate_provider_ssm_parameter_path(
                        provider_id,
                        table_name,
                        SSM_CREDENTIAL_PARAMETER_FIELD,
                        path,
                    )),
                    None => errors.push(format!(
                        "gate_providers.{provider_id}.{table_name}.ssm_credential_parameter must be a string SSM path"
                    )),
                }
            }
        }
    }

    errors
}

fn validate_chainlink_data_streams_gate_provider(
    context: &str,
    table: &toml::map::Map<String, toml::Value>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let feed_bindings_context =
        format!("{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD}");

    for field in table.keys() {
        let is_old_provider_level_feed_field =
            CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS.contains(&field.as_str());
        if !CHAINLINK_DATA_STREAMS_PROVIDER_FIELDS.contains(&field.as_str())
            && !is_old_provider_level_feed_field
        {
            errors.push(format!(
                "{context}.chainlink_data_streams.{field} is not a supported Chainlink Data Streams provider field"
            ));
        }
    }

    if table.contains_key(CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD) {
        errors.push(format!(
            "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD} must be replaced by {CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD} and {CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD}"
        ));
    }
    required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_ENDPOINT_ID_FIELD,
        &mut errors,
    );
    if let Some(rest_base_url) = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
        &mut errors,
    ) && url::Url::parse(rest_base_url).is_err()
    {
        errors.push(format!(
            "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD} must be an absolute URL"
        ));
    }
    if let Some(report_endpoint_path) = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
        &mut errors,
    ) && (!report_endpoint_path.starts_with('/') || report_endpoint_path.contains('?'))
    {
        errors.push(format!(
            "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD} must be an absolute path without query parameters"
        ));
    }
    required_positive_integer_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD,
        &mut errors,
    );
    errors.extend(validate_chainlink_data_streams_ssm_parameter_field(
        context,
        table,
        CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD,
    ));
    errors.extend(validate_chainlink_data_streams_ssm_parameter_field(
        context,
        table,
        CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD,
    ));

    for field in CHAINLINK_DATA_STREAMS_OLD_PROVIDER_LEVEL_FEED_FIELDS {
        if table.contains_key(*field) {
            errors.push(format!(
                "{context}.chainlink_data_streams.{field} must move under [[{feed_bindings_context}]]"
            ));
        }
    }

    let Some(feed_bindings) = table
        .get(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD)
        .and_then(toml::Value::as_array)
        .filter(|bindings| !bindings.is_empty())
    else {
        errors.push(format!(
            "{feed_bindings_context} must contain one or more resolution feed bindings"
        ));
        return errors;
    };

    let mut seen = HashSet::new();
    for (index, binding_value) in feed_bindings.iter().enumerate() {
        let binding_context = format!("{feed_bindings_context}[{index}]");
        let Some(binding) = binding_value.as_table() else {
            errors.push(format!("{binding_context} must be a TOML table"));
            continue;
        };
        let resolution_identity = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD,
            &mut errors,
        );
        let value_kind = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD,
            &mut errors,
        );
        if let Some(value_kind) = value_kind
            && value_kind != PRICE_GATE_VALUE_KIND
        {
            errors.push(format!(
                "{binding_context}.value_kind `{value_kind}` is not supported for Chainlink Data Streams price reports"
            ));
        }
        if let (Some(resolution_identity), Some(value_kind)) = (resolution_identity, value_kind)
            && !seen.insert((resolution_identity.to_string(), value_kind.to_string()))
        {
            errors.push(format!(
                "{binding_context} duplicates resolution_identity `{resolution_identity}` and value_kind `{value_kind}`"
            ));
        }
        if let Some(feed_id) = required_string_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_FEED_ID_FIELD,
            &mut errors,
        ) && !is_lowercase_chainlink_feed_id(feed_id)
        {
            errors.push(format!(
                "{binding_context}.feed_id must be a lowercase Chainlink feed id"
            ));
        }
        required_positive_integer_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD,
            &mut errors,
        );
        required_positive_integer_field(
            binding,
            &binding_context,
            CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD,
            &mut errors,
        );
    }

    errors
}

fn validate_chainlink_data_streams_ssm_parameter_field(
    context: &str,
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let field_context = format!("{context}.chainlink_data_streams.{field}");
    let Some(value) = table.get(field) else {
        errors.push(format!("{field_context} must be a string SSM path"));
        return errors;
    };
    let Some(path) = value.as_str() else {
        errors.push(format!("{field_context} must be a string SSM path"));
        return errors;
    };
    let trimmed = path.trim();
    if trimmed.is_empty() {
        errors.push(format!("{field_context} must be a non-empty SSM path"));
    } else {
        if trimmed != path {
            errors.push(format!(
                "{field_context} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            errors.push(format!(
                "{field_context} must be an absolute-style SSM parameter path starting with `/`: `{path}`"
            ));
        }
    }
    errors
}

fn required_string_field<'a>(
    table: &'a toml::map::Map<String, toml::Value>,
    context: &str,
    field: &str,
    errors: &mut Vec<String>,
) -> Option<&'a str> {
    match table.get(field).and_then(toml::Value::as_str) {
        Some(value) if !value.trim().is_empty() => Some(value.trim()),
        _ => {
            errors.push(format!("{context}.{field} must be a non-empty string"));
            None
        }
    }
}

fn required_positive_integer_field(
    table: &toml::map::Map<String, toml::Value>,
    context: &str,
    field: &str,
    errors: &mut Vec<String>,
) {
    if table
        .get(field)
        .and_then(toml::Value::as_integer)
        .is_none_or(|value| value <= 0)
    {
        errors.push(format!("{context}.{field} must be a positive integer"));
    }
}

fn is_lowercase_chainlink_feed_id(value: &str) -> bool {
    value.len() == 66
        && value.starts_with("0x")
        && value[2..]
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn validate_gate_provider_freshness(
    context: &str,
    freshness: &GateProviderFreshnessBlock,
) -> Vec<String> {
    let mut errors = Vec::new();

    match freshness.max_age_ms {
        Some(0) => errors.push(format!("{context}.max_age_ms must be a positive integer")),
        Some(_) => {}
        None => errors.push(format!("{context}.max_age_ms is required")),
    }
    match freshness.max_clock_skew_ms {
        Some(0) => errors.push(format!(
            "{context}.max_clock_skew_ms must be a positive integer"
        )),
        Some(_) => {}
        None => errors.push(format!("{context}.max_clock_skew_ms is required")),
    }
    if let (Some(max_age_ms), Some(max_clock_skew_ms)) =
        (freshness.max_age_ms, freshness.max_clock_skew_ms)
        && max_clock_skew_ms > max_age_ms
    {
        errors.push(format!(
            "{context}.max_clock_skew_ms must be less than or equal to {context}.max_age_ms"
        ));
    }

    errors
}

fn validate_gate_provider_ssm_parameter_path(
    provider_id: &str,
    table_name: &str,
    field: &str,
    value: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let context = format!("gate_providers.{provider_id}.{table_name}.{field}");
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!("{context} must be a non-empty SSM path"));
    } else {
        if trimmed != value {
            errors.push(format!(
                "{context} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            errors.push(format!(
                "{context} must be an absolute-style SSM parameter path starting with `/`: `{value}`"
            ));
        }
    }
    errors
}

fn validate_nautilus_block(block: &NautilusBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.timeout_connection_secs",
            block.timeout_connection_secs,
        ),
        (
            "nautilus.timeout_reconciliation_secs",
            block.timeout_reconciliation_secs,
        ),
        (
            "nautilus.timeout_portfolio_secs",
            block.timeout_portfolio_secs,
        ),
        (
            "nautilus.timeout_disconnection_secs",
            block.timeout_disconnection_secs,
        ),
        (
            "nautilus.timeout_shutdown_secs",
            block.timeout_shutdown_secs,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }
    errors.extend(validate_data_engine_block(&block.data_engine));
    errors.extend(validate_exec_engine_block(&block.exec_engine));
    errors
}

fn validate_data_engine_block(
    block: &crate::bolt_v3_config::NautilusDataEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(error) = BarIntervalType::from_str(&block.time_bars_interval_type) {
        errors.push(format!(
            "nautilus.data_engine.time_bars_interval_type is not valid ({error}): `{}`",
            block.time_bars_interval_type
        ));
    }
    for aggregation in block.time_bars_origins.keys() {
        if let Err(error) = BarAggregation::from_str(aggregation) {
            errors.push(format!(
                "nautilus.data_engine.time_bars_origins key `{aggregation}` is not a valid Nautilus bar aggregation ({error})"
            ));
        }
    }
    if block.graceful_shutdown_on_error {
        errors.push(
            "nautilus.data_engine.graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime"
                .to_string(),
        );
    }
    let nt_data_default = nautilus_live::config::LiveDataEngineConfig::default();
    if block.qsize != nt_data_default.qsize {
        errors.push(format!(
            "nautilus.data_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_data_default.qsize
        ));
    }
    errors
}

fn validate_exec_engine_block(
    block: &crate::bolt_v3_config::NautilusExecEngineBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    let positive_fields: &[(&str, u64)] = &[
        (
            "nautilus.exec_engine.inflight_check_threshold_ms",
            block.inflight_check_threshold_ms as u64,
        ),
        (
            "nautilus.exec_engine.open_check_threshold_ms",
            block.open_check_threshold_ms as u64,
        ),
        (
            "nautilus.exec_engine.max_single_order_queries_per_cycle",
            block.max_single_order_queries_per_cycle as u64,
        ),
        (
            "nautilus.exec_engine.position_check_threshold_ms",
            block.position_check_threshold_ms as u64,
        ),
    ];
    for (label, value) in positive_fields {
        if *value == 0 {
            errors.push(format!("{label} must be a positive integer"));
        }
    }

    if block.snapshot_orders {
        errors.push(
            "nautilus.exec_engine.snapshot_orders must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.snapshot_positions {
        errors.push(
            "nautilus.exec_engine.snapshot_positions must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.purge_from_database {
        errors.push(
            "nautilus.exec_engine.purge_from_database must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    if block.graceful_shutdown_on_error {
        errors.push(
            "nautilus.exec_engine.graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime".to_string(),
        );
    }
    let nt_exec_default = nautilus_live::config::LiveExecEngineConfig::default();
    if block.qsize != nt_exec_default.qsize {
        errors.push(format!(
            "nautilus.exec_engine.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_exec_default.qsize
        ));
    }

    for instrument_id in &block.reconciliation_instrument_ids {
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "nautilus.exec_engine.reconciliation_instrument_ids contains invalid instrument ID `{instrument_id}` ({error})"
            ));
        }
    }
    for client_order_id in &block.filtered_client_order_ids {
        if let Err(error) = ClientOrderId::new_checked(client_order_id) {
            errors.push(format!(
                "nautilus.exec_engine.filtered_client_order_ids contains invalid client order ID `{client_order_id}` ({error})"
            ));
        }
    }
    errors
}

fn validate_risk_block(block: &RiskBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if let Err(reason) = parse_decimal_string(&block.default_max_notional_per_order) {
        errors.push(format!(
            "risk.default_max_notional_per_order is not a valid decimal string ({reason}): `{value}`",
            value = block.default_max_notional_per_order
        ));
    }
    if block.nautilus.bypass {
        errors.push("risk.nautilus.bypass must be false".to_string());
    }
    if block.nautilus.graceful_shutdown_on_error {
        errors.push(
            "risk.nautilus.graceful_shutdown_on_error must be false; NT rejects true on the Rust live runtime"
                .to_string(),
        );
    }
    let nt_risk_default = nautilus_live::config::LiveRiskEngineConfig::default();
    if block.nautilus.qsize != nt_risk_default.qsize {
        errors.push(format!(
            "risk.nautilus.qsize must match NT default {}; NT rejects non-default qsize on the Rust live runtime",
            nt_risk_default.qsize
        ));
    }
    for (label, value) in [
        (
            "risk.nautilus.max_order_submit_rate",
            block.nautilus.max_order_submit_rate.as_str(),
        ),
        (
            "risk.nautilus.max_order_modify_rate",
            block.nautilus.max_order_modify_rate.as_str(),
        ),
    ] {
        if let Err(reason) = validate_rate_limit_string(value) {
            errors.push(format!(
                "{label} is not a valid Nautilus rate limit ({reason}): `{value}`"
            ));
        }
    }
    for (instrument_id, notional) in &block.nautilus.max_notional_per_order {
        // Mirrors NT's `LiveRiskEngineConfig::validate_runtime_support`;
        // keep this early-bound config validation aligned on pin bumps.
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "risk.nautilus.max_notional_per_order key `{instrument_id}` is not a valid Nautilus instrument ID ({error})"
            ));
        }
        match parse_decimal_string(notional) {
            Ok(value) if value <= Decimal::ZERO => {
                errors.push(format!(
                    "risk.nautilus.max_notional_per_order[`{instrument_id}`] must be a positive decimal string: `{notional}`"
                ));
            }
            Ok(_) => {}
            Err(reason) => {
                errors.push(format!(
                    "risk.nautilus.max_notional_per_order[`{instrument_id}`] is not a valid decimal string ({reason}): `{notional}`"
                ));
            }
        }
    }
    errors
}

fn validate_rate_limit_string(value: &str) -> Result<(), String> {
    let (limit, interval) = value
        .split_once('/')
        .ok_or_else(|| "expected `limit/HH:MM:SS`".to_string())?;
    let limit = limit.parse::<usize>().map_err(|error| error.to_string())?;
    if limit == 0 {
        return Err("limit must be greater than zero".to_string());
    }

    let mut parts = interval.split(':');
    let mut next_part = |label: &str| -> Result<u64, String> {
        parts
            .next()
            .ok_or_else(|| format!("missing {label} component"))?
            .parse::<u64>()
            .map_err(|error| format!("{label}: {error}"))
    };
    let hours = next_part("hours")?;
    let minutes = next_part("minutes")?;
    let seconds = next_part("seconds")?;
    if parts.next().is_some() {
        return Err("expected `limit/HH:MM:SS`".to_string());
    }
    if minutes >= 60 {
        return Err("minutes must be less than 60".to_string());
    }
    if seconds >= 60 {
        return Err("seconds must be less than 60".to_string());
    }
    if hours == 0 && minutes == 0 && seconds == 0 {
        return Err("interval must be greater than zero".to_string());
    }

    Ok(())
}

fn validate_persistence_block(block: &PersistenceBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.catalog_directory).is_absolute() {
        errors.push(format!(
            "persistence.catalog_directory must be an absolute path: `{}`",
            block.catalog_directory
        ));
    }
    if block.runtime_capture_start_poll_interval_ms == 0 {
        errors.push(
            "persistence.runtime_capture_start_poll_interval_ms must be a positive integer"
                .to_string(),
        );
    }
    if block.streaming.flush_interval_ms == 0 {
        errors
            .push("persistence.streaming.flush_interval_ms must be a positive integer".to_string());
    }
    errors
}

fn validate_aws_block(block: &AwsBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if block.region.trim().is_empty() {
        errors.push("aws.region must be a non-empty string".to_string());
    }
    errors
}

fn validate_clients_block(clients: &BTreeMap<String, ClientBlock>) -> Vec<String> {
    let mut errors = Vec::new();
    if clients.is_empty() {
        errors.push("clients must define at least one client block".to_string());
        return errors;
    }
    for (key, client) in clients {
        errors.extend(crate::bolt_v3_providers::validate_client_block(key, client));
        errors.extend(validate_client_readiness_probe(key, client));
    }
    errors.extend(validate_unique_client_readiness_probe_instruments(clients));
    errors
}

fn validate_client_readiness_probe(key: &str, client: &ClientBlock) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(readiness_probe) = &client.readiness_probe else {
        return errors;
    };
    if client.data.is_none() {
        errors.push(format!(
            "clients.{key}.readiness_probe requires the same client to declare a [data] block"
        ));
    }
    match readiness_probe.market_data_kind {
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Quote => {
            if readiness_probe.book_type.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type is only valid when market_data_kind = \"book\""
                ));
            }
        }
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Book => {
            if readiness_probe.book_type.is_none() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type must be configured when market_data_kind = \"book\""
                ));
            }
        }
    }
    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            if readiness_probe
                .quote_targets
                .as_ref()
                .is_none_or(|quote_targets| quote_targets.is_empty())
            {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets must define at least one configured quote target when quote_target_source = \"configured\""
                ));
            }
            if readiness_probe.max_metadata_quote_targets.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.max_metadata_quote_targets is only valid when quote_target_source = \"metadata_response\""
                ));
            }
            if readiness_probe.allow_metadata_target_sampling.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.allow_metadata_target_sampling is only valid when quote_target_source = \"metadata_response\""
                ));
            }
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            if readiness_probe.quote_targets.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe cannot combine quote_target_source = \"metadata_response\" with readiness_probe.quote_targets"
                ));
            }
            match readiness_probe.max_metadata_quote_targets {
                Some(max_metadata_quote_targets) if max_metadata_quote_targets > 0 => {}
                _ => {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.max_metadata_quote_targets must be a positive integer when quote_target_source = \"metadata_response\""
                    ));
                }
            };
            if readiness_probe.allow_metadata_target_sampling.is_none() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.allow_metadata_target_sampling must be explicitly configured when quote_target_source = \"metadata_response\""
                ));
            }
        }
    }
    if let Some(quote_targets) = &readiness_probe.quote_targets {
        for (target_id, target) in quote_targets {
            if target_id.trim().is_empty() || target_id.trim() != target_id {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets target id must be non-empty without surrounding whitespace"
                ));
            }
            if target.instrument_id.venue.as_str() != client.venue.as_str() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.quote_targets.{target_id}.instrument_id venue `{}` must match clients.{key}.venue `{}`",
                    target.instrument_id.venue,
                    client.venue
                ));
            }
        }
    }
    errors
}

fn validate_unique_client_readiness_probe_instruments(
    clients: &BTreeMap<String, ClientBlock>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut by_instrument: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (client_key, client) in clients {
        let Some(readiness_probe) = &client.readiness_probe else {
            continue;
        };
        if let Some(quote_targets) = &readiness_probe.quote_targets {
            for (target_id, target) in quote_targets {
                let instrument_id = target.instrument_id.to_string();
                match by_instrument.get(instrument_id.as_str()) {
                    Some((existing_client_key, existing_target_id))
                        if existing_client_key != client_key =>
                    {
                        errors.push(format!(
                            "clients.{client_key}.readiness_probe.quote_targets.{target_id}.instrument_id `{instrument_id}` is also used by clients.{existing_client_key}.readiness_probe.quote_targets.{existing_target_id}.instrument_id; QuoteTick does not carry data_client_id, so no-submit data-client quote probe evidence cannot distinguish data clients for the same instrument"
                        ));
                    }
                    None => {
                        by_instrument
                            .insert(instrument_id, (client_key.as_str(), target_id.as_str()));
                    }
                    _ => {}
                }
            }
        }
    }
    errors
}

/// Provider-neutral SSM parameter-path utility shared by the per-
/// provider secret validators in `crate::bolt_v3_providers`. Stays in
/// core because the path-shape rule itself is provider-neutral and is
/// also the gate behind the SSM-only invariant; mirrors the cross-
/// layer call that the archetype binding makes into
/// `parse_decimal_string`.
pub(crate) fn validate_ssm_parameter_path(key: &str, field: &str, value: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        errors.push(format!(
            "clients.{key}.secrets.{field} must be a non-empty SSM path"
        ));
    } else {
        if trimmed != value {
            errors.push(format!(
                "clients.{key}.secrets.{field} must not have leading or trailing whitespace"
            ));
        }
        if !trimmed.starts_with('/') {
            // The Rust AWS SDK accepts both `name`-style and `/name`-style
            // parameter references, but bolt-v3 standardizes on
            // absolute-style hierarchical paths so an SSM resource layout
            // like `/bolt/<venue>/<field>` is the only supported shape and
            // typos that drop the leading slash fail closed at startup.
            errors.push(format!(
            "clients.{key}.secrets.{field} must be an absolute-style SSM parameter path starting with `/`: `{value}`"
        ));
        }
    }
    errors
}

pub fn validate_strategies(root: &BoltV3RootConfig, strategies: &[LoadedStrategy]) -> Vec<String> {
    let mut errors = Vec::new();

    let mut seen_instance_ids: HashSet<&str> = HashSet::new();
    let mut seen_order_id_tags: HashSet<&str> = HashSet::new();
    let mut seen_target_ids: HashSet<String> = HashSet::new();

    let default_max_notional_decimal =
        parse_decimal_string(&root.risk.default_max_notional_per_order).ok();

    for loaded in strategies {
        let context = format!("strategy `{}`", loaded.relative_path);
        let strategy = &loaded.config;

        if strategy.schema_version != SUPPORTED_STRATEGY_SCHEMA_VERSION {
            errors.push(format!(
                "{context}: schema_version={} is unsupported by this build (only {} is currently supported)",
                strategy.schema_version, SUPPORTED_STRATEGY_SCHEMA_VERSION
            ));
        }

        if !seen_instance_ids.insert(strategy.strategy_instance_id.as_str()) {
            errors.push(format!(
                "{context}: strategy_instance_id `{}` is already used by another listed strategy",
                strategy.strategy_instance_id
            ));
        }
        if !seen_order_id_tags.insert(strategy.order_id_tag.as_str()) {
            errors.push(format!(
                "{context}: order_id_tag `{}` is already used by another listed strategy",
                strategy.order_id_tag
            ));
        }

        match root.clients.get(strategy.execution_client_id.as_str()) {
            None => errors.push(format!(
                "{context}: execution_client_id `{}` does not match any [clients.<id>] block",
                strategy.execution_client_id
            )),
            Some(client) => {
                if client.execution.is_none() {
                    errors.push(format!(
                        "{context}: strategy execution_client_id `{}` must reference an execution-capable client \
                         (the referenced client has no [execution] block)",
                        strategy.execution_client_id
                    ));
                }
            }
        }

        let (target_metadata, target_errors) =
            crate::bolt_v3_market_families::validate_strategy_target(&context, &strategy.target);
        if let Some(metadata) = target_metadata {
            let configured_target_id = metadata.configured_target_id;
            if !seen_target_ids.insert(configured_target_id.clone()) {
                errors.push(format!(
                    "{context}: configured_target_id `{configured_target_id}` is already used by another configured target"
                ));
            }
        }
        errors.extend(target_errors.into_iter().map(|error| error.to_string()));

        errors.extend(validate_reference_data(&context, root, strategy));
        errors.extend(crate::bolt_v3_archetypes::validate_strategy_archetype(
            &context,
            strategy,
            default_max_notional_decimal.as_ref(),
        ));
    }
    errors.extend(validate_reference_quote_probe_sources(strategies));
    errors.extend(validate_target_gate_provider_references(root, strategies));
    errors.extend(validate_chainlink_feed_binding_coverage(root, strategies));

    errors
}

fn validate_target_gate_provider_references(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    let mut errors = Vec::new();
    let known_provider_ids = match &root.gate_providers {
        Some(providers) => providers.keys().cloned().collect::<BTreeSet<_>>(),
        None => BTreeSet::new(),
    };

    for loaded in strategies {
        let strategy_context = format!("strategy `{}`", loaded.relative_path);
        let Some(target) = loaded.config.target.as_table() else {
            continue;
        };
        let Some(gate_subscriptions) = target
            .get(TARGET_GATE_SUBSCRIPTIONS_FIELD)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        for (role, subscription_value) in gate_subscriptions {
            let Some(subscription) = subscription_value.as_table() else {
                continue;
            };
            let subscription_context =
                format!("{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}");
            let allowed_provider_ids =
                string_array_values(subscription, TARGET_ALLOWED_PROVIDER_IDS_FIELD);
            for provider_id in &allowed_provider_ids {
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{subscription_context}.{TARGET_ALLOWED_PROVIDER_IDS_FIELD}"),
                    provider_id,
                );
            }

            for provider_id in string_array_values(subscription, TARGET_PROVIDER_PREFERENCE_FIELD) {
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{subscription_context}.{TARGET_PROVIDER_PREFERENCE_FIELD}"),
                    &provider_id,
                );
                if !allowed_provider_ids.is_empty()
                    && !allowed_provider_ids
                        .iter()
                        .any(|allowed| allowed == &provider_id)
                {
                    errors.push(format!(
                        "{subscription_context}.{TARGET_PROVIDER_PREFERENCE_FIELD} provider_id `{provider_id}` must also be listed in {TARGET_ALLOWED_PROVIDER_IDS_FIELD}"
                    ));
                }
            }

            let Some(market_mappings) = subscription
                .get(TARGET_MARKET_MAPPINGS_FIELD)
                .and_then(toml::Value::as_array)
            else {
                continue;
            };
            for (index, mapping_value) in market_mappings.iter().enumerate() {
                let Some(mapping) = mapping_value.as_table() else {
                    continue;
                };
                let Some(provider_id) = string_field(mapping, TARGET_PROVIDER_ID_FIELD) else {
                    continue;
                };
                let mapping_context =
                    format!("{subscription_context}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]");
                validate_known_target_gate_provider_id(
                    &mut errors,
                    &known_provider_ids,
                    &format!("{mapping_context}.{TARGET_PROVIDER_ID_FIELD}"),
                    &provider_id,
                );
                if !allowed_provider_ids.is_empty()
                    && !allowed_provider_ids
                        .iter()
                        .any(|allowed| allowed == &provider_id)
                {
                    errors.push(format!(
                        "{mapping_context}.{TARGET_PROVIDER_ID_FIELD} `{provider_id}` must also be listed in {TARGET_ALLOWED_PROVIDER_IDS_FIELD}"
                    ));
                }
            }
        }
    }

    errors
}

fn validate_known_target_gate_provider_id(
    errors: &mut Vec<String>,
    known_provider_ids: &BTreeSet<String>,
    context: &str,
    provider_id: &str,
) {
    if !known_provider_ids.contains(provider_id) {
        errors.push(format!(
            "{context} references provider_id `{provider_id}` but [gate_providers.{provider_id}] is not configured"
        ));
    }
}

fn validate_chainlink_feed_binding_coverage(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    let mut errors = Vec::new();
    let target_references = collect_chainlink_target_mapping_references(strategies, &mut errors);
    let target_keys = target_references
        .iter()
        .map(|reference| reference.key.clone())
        .collect::<BTreeSet<_>>();
    let feed_bindings = collect_chainlink_feed_bindings(root);

    for reference in &target_references {
        let binding_count = match feed_bindings.get(&reference.key) {
            Some(contexts) => contexts.len(),
            None => 0,
        };
        match binding_count {
            1 => {}
            0 => errors.push(format!(
                "{}: Chainlink Data Streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has no matching gate_providers.{}.chainlink_data_streams.feed_bindings entry",
                reference.context,
                reference.key.provider_id,
                reference.key.resolution_identity,
                reference.key.value_kind,
                reference.key.provider_id
            )),
            count => errors.push(format!(
                "{}: Chainlink Data Streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has {count} matching gate_providers.{}.chainlink_data_streams.feed_bindings entries; expected exactly one",
                reference.context,
                reference.key.provider_id,
                reference.key.resolution_identity,
                reference.key.value_kind,
                reference.key.provider_id
            )),
        }
    }

    for (key, contexts) in &feed_bindings {
        if !target_keys.contains(key) {
            for context in contexts {
                errors.push(format!(
                    "{context} resolution_identity `{}` value_kind `{}` is not referenced by any loaded strategy Chainlink mapping",
                    key.resolution_identity, key.value_kind
                ));
            }
        }
    }

    errors
}

fn collect_chainlink_target_mapping_references(
    strategies: &[LoadedStrategy],
    errors: &mut Vec<String>,
) -> Vec<ChainlinkTargetMappingReference> {
    let mut references = Vec::new();

    for loaded in strategies {
        let strategy_context = format!("strategy `{}`", loaded.relative_path);
        let Some(target) = loaded.config.target.as_table() else {
            continue;
        };
        let Some(gate_subscriptions) = target
            .get(TARGET_GATE_SUBSCRIPTIONS_FIELD)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        for (role, subscription_value) in gate_subscriptions {
            let Some(subscription) = subscription_value.as_table() else {
                continue;
            };
            let Some(market_mappings) = subscription
                .get(TARGET_MARKET_MAPPINGS_FIELD)
                .and_then(toml::Value::as_array)
            else {
                continue;
            };
            for (index, mapping_value) in market_mappings.iter().enumerate() {
                let Some(mapping) = mapping_value.as_table() else {
                    continue;
                };
                if string_field(mapping, TARGET_RESOLUTION_KIND_FIELD).as_deref()
                    != Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
                {
                    continue;
                }
                let (Some(resolution_identity), Some(value_kind)) = (
                    string_field(mapping, CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD),
                    string_field(mapping, CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD),
                ) else {
                    continue;
                };
                let Some(provider_id) = selected_chainlink_provider_id(subscription, mapping)
                else {
                    errors.push(format!(
                        "{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]: Chainlink Data Streams mapping resolution_identity `{resolution_identity}` value_kind `{value_kind}` cannot resolve provider_id from mapping provider_id, provider_preference, or a single allowed_provider_ids entry"
                    ));
                    continue;
                };
                references.push(ChainlinkTargetMappingReference {
                    key: ChainlinkFeedBindingKey {
                        provider_id,
                        resolution_identity,
                        value_kind,
                    },
                    context: format!(
                        "{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]"
                    ),
                });
            }
        }
    }

    references
}

fn selected_chainlink_provider_id(
    subscription: &toml::map::Map<String, toml::Value>,
    mapping: &toml::map::Map<String, toml::Value>,
) -> Option<String> {
    string_field(mapping, TARGET_PROVIDER_ID_FIELD)
        .or_else(|| first_string_array_value(subscription, TARGET_PROVIDER_PREFERENCE_FIELD))
        .or_else(|| single_string_array_value(subscription, TARGET_ALLOWED_PROVIDER_IDS_FIELD))
}

fn collect_chainlink_feed_bindings(
    root: &BoltV3RootConfig,
) -> BTreeMap<ChainlinkFeedBindingKey, Vec<String>> {
    let mut bindings: BTreeMap<ChainlinkFeedBindingKey, Vec<String>> = BTreeMap::new();
    let Some(gate_providers) = &root.gate_providers else {
        return bindings;
    };

    for (provider_id, provider) in gate_providers {
        if provider.provider_kind.as_deref().map(str::trim)
            != Some(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
        {
            continue;
        }
        let Some(provider_config) = provider
            .provider_config
            .get(CHAINLINK_DATA_STREAMS_PROVIDER_KIND)
            .and_then(toml::Value::as_table)
        else {
            continue;
        };
        let Some(feed_bindings) = provider_config
            .get(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD)
            .and_then(toml::Value::as_array)
        else {
            continue;
        };
        for (index, binding_value) in feed_bindings.iter().enumerate() {
            let Some(binding) = binding_value.as_table() else {
                continue;
            };
            let (Some(resolution_identity), Some(value_kind)) = (
                string_field(binding, CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD),
                string_field(binding, CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD),
            ) else {
                continue;
            };
            let key = ChainlinkFeedBindingKey {
                provider_id: provider_id.clone(),
                resolution_identity,
                value_kind,
            };
            let context = format!(
                "gate_providers.{provider_id}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD}[{index}]"
            );
            match bindings.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().push(context);
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(vec![context]);
                }
            }
        }
    }

    bindings
}

fn string_field(table: &toml::map::Map<String, toml::Value>, field: &str) -> Option<String> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn string_array_values(table: &toml::map::Map<String, toml::Value>, field: &str) -> Vec<String> {
    table
        .get(field)
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn first_string_array_value(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Option<String> {
    table
        .get(field)
        .and_then(toml::Value::as_array)
        .and_then(|values| values.first())
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn single_string_array_value(
    table: &toml::map::Map<String, toml::Value>,
    field: &str,
) -> Option<String> {
    let values = table.get(field).and_then(toml::Value::as_array)?;
    (values.len() == 1)
        .then(|| values[0].as_str())
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn validate_reference_quote_probe_sources(strategies: &[LoadedStrategy]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut by_instrument: BTreeMap<String, (&str, String, String)> = BTreeMap::new();

    for loaded in strategies {
        let context = format!("strategy `{}`", loaded.relative_path);
        for (role, block) in &loaded.config.reference_data {
            let instrument_id = block.instrument_id.to_string();
            let data_client_id = block.data_client_id.as_str();
            match by_instrument.get(&instrument_id) {
                Some((existing_data_client_id, existing_context, existing_role))
                    if *existing_data_client_id != data_client_id =>
                {
                    errors.push(format!(
                        "{context}: reference_data.{role}.instrument_id `{instrument_id}` with data_client_id `{data_client_id}` is also used by {existing_context}: reference_data.{existing_role}.instrument_id with data_client_id `{existing_data_client_id}`; QuoteTick does not carry data_client_id, so no-submit reference quote evidence cannot distinguish data clients for the same instrument"
                    ));
                }
                None => {
                    by_instrument.insert(
                        instrument_id,
                        (data_client_id, context.clone(), role.clone()),
                    );
                }
                _ => {}
            }
        }
    }

    errors
}

fn validate_reference_data(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let mut errors = Vec::new();

    for (role, block) in &strategy.reference_data {
        match root.clients.get(block.data_client_id.as_str()) {
            None => errors.push(format!(
                "{context}: reference_data.{role}.data_client_id `{}` does not match any [clients.<id>] block",
                block.data_client_id
            )),
            Some(client) => {
                if client.data.is_none() {
                    errors.push(format!(
                        "{context}: reference_data.{role}.data_client_id `{}` must reference a data-capable client \
                         (the referenced client has no [data] block)",
                        block.data_client_id
                    ));
                }
            }
        }
    }

    errors
}

pub(crate) fn parse_decimal_string(value: &str) -> Result<Decimal, String> {
    use std::str::FromStr;
    Decimal::from_str(value).map_err(|error| error.to_string())
}
