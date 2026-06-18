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
//! `crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings`,
//! passing the production binding list from
//! `crate::strategy_bindings::production_validation_bindings`.
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
    CapitalPoolBlock, ClientBlock, DataClientReadinessProbeQuoteTargetSource,
    GATE_PROVIDER_CAPABILITIES, GATE_PROVIDER_KINDS, GateProviderBlock, GateProviderFreshnessBlock,
    KillSwitchConfigBlock, LoadedStrategy, NautilusBlock, PRICE_GATE_VALUE_KIND, PersistenceBlock,
    RealizedVolatilityAggregationBlock, RealizedVolatilityJumpPolicyBlock,
    RealizedVolatilityNoiseMethodBlock, RealizedVolatilityPricingComponentBlock,
    RealizedVolatilitySampleKindBlock, RealizedVolatilitySourceClassBlock, RiskBlock,
    SSM_CREDENTIAL_PARAMETER_FIELD, TEST_DOUBLE_PROVIDER_KIND,
};
use crate::bolt_v3_decision_evidence::validate_decision_evidence_relative_path;
use crate::bolt_v3_loss_halt_actions::LossGovernorTradingStateAction;
use crate::bolt_v3_numeric::{HALF_F64, UNIT_F64, ZERO_F64, is_positive_finite};
use crate::bolt_v3_order_execution::BoltV3OrderExecutionMode;
use crate::bolt_v3_providers::{
    ReferencePriceIdentifierKind, reference_price_provider_identifier_is_configured,
    reference_price_provider_metadata,
};
use crate::bolt_v3_reference_price::reference_price_source_is_unsupported;

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
// Shared with the provider-owned F3 client/gate-provider consistency check
// (`crate::bolt_v3_providers::chainlink::validate_client_gate_provider_consistency`),
// which reaches them through this single core definition rather than re-declaring
// the gate-provider field names.
pub(crate) const CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD: &str = "rest_base_url";
pub(crate) const CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD: &str = "report_endpoint_path";
pub(crate) const CHAINLINK_DATA_STREAMS_HTTP_TIMEOUT_SECS_FIELD: &str = "http_timeout_secs";
pub(crate) const CHAINLINK_DATA_STREAMS_API_KEY_SSM_PARAMETER_FIELD: &str = "api_key_ssm_parameter";
pub(crate) const CHAINLINK_DATA_STREAMS_API_SECRET_SSM_PARAMETER_FIELD: &str =
    "api_secret_ssm_parameter";
const CHAINLINK_DATA_STREAMS_OLD_SSM_CREDENTIAL_PARAMETER_FIELD: &str = "ssm_credential_parameter";
const CHAINLINK_DATA_STREAMS_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const CHAINLINK_DATA_STREAMS_VALUE_KIND_FIELD: &str = "value_kind";
const CHAINLINK_DATA_STREAMS_FEED_ID_FIELD: &str = "feed_id";
const CHAINLINK_DATA_STREAMS_REPORT_SCHEMA_VERSION_FIELD: &str = "report_schema_version";
const CHAINLINK_DATA_STREAMS_REPORT_DECIMAL_SCALE_FIELD: &str = "report_decimal_scale";
const MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT: usize = usize::MIN;
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

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct ResolutionFeedBindingKey {
    provider_id: String,
    resolution_identity: String,
    value_kind: String,
}

#[derive(Debug, Clone)]
struct ResolutionFeedMappingReference {
    key: ResolutionFeedBindingKey,
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
    errors.extend(validate_order_rate_within_venue_egress(root));
    errors.extend(validate_persistence_block(&root.persistence));
    errors.extend(validate_position_sizer_recovery_evidence(root));
    errors.extend(validate_aws_block(&root.aws));
    errors.extend(validate_clients_block(root));
    errors.extend(validate_realized_volatility_surfaces(root));
    if let Some(gate_providers) = &root.gate_providers {
        errors.extend(validate_gate_providers(gate_providers, &root.clients));
    }
    errors.extend(crate::bolt_v3_outcome_group_sources::validate_root_sources(
        root,
    ));
    if let Some(iv) = &root.iv {
        errors.extend(crate::bolt_v3_iv::config::validate_iv_root_config(iv));
    }
    errors.extend(validate_realized_volatility_source_clients(root));
    errors.extend(validate_iv_source_clients(root));
    errors.extend(crate::bolt_v3_providers::validate_resolution_oracle_client_consistency(root));

    errors
}

pub(crate) fn validate_iv_source_clients(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(iv) = root.iv.as_ref() else {
        return errors;
    };

    for profile in &iv.profiles {
        for source in &profile.sources {
            let context = format!(
                "iv.profiles.{}.sources.{}",
                profile.profile_id, source.source_id
            );
            match root.clients.get(source.client_id.as_str()) {
                None => errors.push(format!(
                    "{context}.client_id `{}` does not match any [clients.<id>] block",
                    source.client_id
                )),
                Some(client) if client.data.is_none() => errors.push(format!(
                    "{context}.client_id `{}` must reference a data-capable client (the referenced client has no [data] block)",
                    source.client_id
                )),
                Some(_) => {}
            }
        }
    }

    errors
}

pub(crate) fn validate_realized_volatility_source_clients(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(realized_volatility_surfaces) = root.realized_volatility_surfaces.as_ref() else {
        return errors;
    };

    for (surface_id, surface) in realized_volatility_surfaces {
        for source in surface.sources.iter().filter(|source| source.enabled) {
            let context = format!(
                "realized_volatility_surfaces.{surface_id}.sources.{}",
                source.source_id
            );
            match root.clients.get(source.data_client_id.as_str()) {
                None => errors.push(format!(
                    "{context}.data_client_id `{}` does not match any [clients.<id>] block",
                    source.data_client_id
                )),
                Some(client) if client.data.is_none() => errors.push(format!(
                    "{context}.data_client_id `{}` must reference a data-capable client (no [data] block)",
                    source.data_client_id
                )),
                Some(_) => {}
            }
        }
    }

    errors
}

fn validate_realized_volatility_surfaces(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(realized_volatility_surfaces) = root.realized_volatility_surfaces.as_ref() else {
        return errors;
    };

    for (surface_id, surface) in realized_volatility_surfaces {
        let context = format!("realized_volatility_surfaces.{surface_id}");
        if surface_id.trim().is_empty() {
            errors.push("realized_volatility_surfaces contains an empty surface id".to_string());
        }
        if surface.canonical_base_asset.trim().is_empty() {
            errors.push(format!("{context}.canonical_base_asset must be non-empty"));
        }
        if surface.canonical_quote_asset.trim().is_empty() {
            errors.push(format!("{context}.canonical_quote_asset must be non-empty"));
        }
        if surface.sources.is_empty() {
            errors.push(format!(
                "{context}.sources must contain at least one source"
            ));
        }

        let policy = &surface.policy;
        for (field, value) in [
            (stringify!(window_ms), policy.window_ms),
            (
                stringify!(sampling_interval_ms),
                policy.sampling_interval_ms,
            ),
            (
                stringify!(min_ready_sources),
                policy.min_ready_sources as u64,
            ),
            (stringify!(max_source_age_ms), policy.max_source_age_ms),
            (
                stringify!(max_event_receive_lag_ms),
                policy.max_event_receive_lag_ms,
            ),
            (
                stringify!(max_inter_sample_gap_ms),
                policy.max_inter_sample_gap_ms,
            ),
        ] {
            if value == u64::MIN {
                errors.push(format!(
                    "{context}.policy.{field} must be a positive integer"
                ));
            }
        }
        if policy.window_ms < policy.sampling_interval_ms {
            errors.push(format!(
                "{context}.policy.{} {} must be greater than or equal to policy.{} {}",
                stringify!(window_ms),
                policy.window_ms,
                stringify!(sampling_interval_ms),
                policy.sampling_interval_ms,
            ));
        }
        if !is_positive_finite(policy.min_coverage_ratio) || policy.min_coverage_ratio > UNIT_F64 {
            errors.push(format!(
                "{context}.policy.min_coverage_ratio must be finite and in (0, 1]"
            ));
        }
        if !policy.max_cross_source_dispersion.is_finite()
            || policy.max_cross_source_dispersion < ZERO_F64
        {
            errors.push(format!(
                "{context}.policy.max_cross_source_dispersion must be finite and non-negative"
            ));
        }
        if !is_positive_finite(policy.seconds_per_annum) {
            errors.push(format!(
                "{context}.policy.seconds_per_annum must be positive finite"
            ));
        }
        if !policy.upper_quantile.is_finite()
            || !(HALF_F64..=UNIT_F64).contains(&policy.upper_quantile)
        {
            errors.push(format!(
                "{context}.policy.upper_quantile must be finite and in [0.5, 1.0]"
            ));
        }
        if matches!(
            policy.aggregation,
            RealizedVolatilityAggregationBlock::TrimmedMean
        ) {
            match policy.trim_fraction {
                Some(trim_fraction)
                    if trim_fraction.is_finite()
                        && (ZERO_F64..HALF_F64).contains(&trim_fraction) => {}
                _ => errors.push(format!(
                    "{context}.policy.trim_fraction must be finite and in [0, 0.5) for trimmed_mean aggregation"
                )),
            }
        }
        if matches!(
            policy.aggregation,
            RealizedVolatilityAggregationBlock::MedianWithUpperQuantileGuard
        ) {
            match policy.guard_weight {
                Some(guard_weight)
                    if guard_weight.is_finite()
                        && (ZERO_F64..=UNIT_F64).contains(&guard_weight) => {}
                _ => errors.push(format!(
                    "{context}.policy.guard_weight must be finite and in [0, 1] for median_with_upper_quantile_guard aggregation"
                )),
            }
        }
        if let Some(estimator) = surface.estimator.as_ref() {
            if estimator.noise_robust_method.is_none() {
                errors.push(format!(
                    "{context}.estimator.noise_robust_method must be set when estimator is configured"
                ));
            }
            if estimator.jump_policy.is_none() {
                errors.push(format!(
                    "{context}.estimator.jump_policy must be set when estimator is configured"
                ));
            }
            if estimator.pricing_component.is_none() {
                errors.push(format!(
                    "{context}.estimator.pricing_component must be set when estimator is configured"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::Subsampled)
            ) {
                let subsamples = estimator
                    .subsamples
                    .unwrap_or(MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT);
                let min_ready_subsamples = estimator
                    .min_ready_subsamples
                    .unwrap_or(MISSING_REALIZED_VOLATILITY_SUBSAMPLE_COUNT);
                if subsamples == 0 || min_ready_subsamples == 0 {
                    errors.push(format!(
                        "{context}.estimator.subsamples and min_ready_subsamples must be positive for subsampled RV"
                    ));
                }
                if min_ready_subsamples > subsamples {
                    errors.push(format!(
                        "{context}.estimator.min_ready_subsamples must be less than or equal to subsamples"
                    ));
                }
                if subsamples as u64 > policy.sampling_interval_ms {
                    errors.push(format!(
                        "{context}.estimator.subsamples must not exceed policy.sampling_interval_ms unless collision semantics are explicitly supported"
                    ));
                }
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator.coarse_sampling_interval_ms.is_none()
            {
                errors.push(format!(
                    "{context}.estimator.coarse_sampling_interval_ms must be set for coarser_grid RV"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator.coarser_grid_policy.is_none()
            {
                errors.push(format!(
                    "{context}.estimator.coarser_grid_policy must be set for coarser_grid RV"
                ));
            }
            if matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
            ) && estimator
                .coarse_sampling_interval_ms
                .is_some_and(|interval| interval <= policy.sampling_interval_ms)
            {
                errors.push(format!(
                    "{context}.estimator.coarse_sampling_interval_ms must be greater than policy.sampling_interval_ms"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::NoiseRobust)
            ) && !matches!(
                estimator.noise_robust_method,
                Some(RealizedVolatilityNoiseMethodBlock::CoarserGrid)
                    | Some(RealizedVolatilityNoiseMethodBlock::Subsampled)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component noise_robust requires noise_robust_method other than none"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::Forecast)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component forecast is not enabled in this implementation slice"
                ));
            }
            if matches!(
                estimator.pricing_component,
                Some(RealizedVolatilityPricingComponentBlock::Continuous)
            ) && !matches!(
                estimator.jump_policy,
                Some(RealizedVolatilityJumpPolicyBlock::Separate)
            ) {
                errors.push(format!(
                    "{context}.estimator.pricing_component continuous requires jump_policy separate"
                ));
            }
        }

        let mut seen_source_ids = BTreeSet::new();
        let mut seen_source_instrument_clients: BTreeMap<String, (String, String)> =
            BTreeMap::new();
        let mut enabled_quorum_sources = 0usize;
        let mut quorum_source_contract: Option<(
            RealizedVolatilitySourceClassBlock,
            RealizedVolatilitySampleKindBlock,
            String,
        )> = None;
        for (index, source) in surface.sources.iter().enumerate() {
            let source_context = format!("{context}.sources[{index}]");
            if source.source_id.trim().is_empty() {
                errors.push(format!("{source_context}.source_id must be non-empty"));
            } else if !seen_source_ids.insert(source.source_id.as_str()) {
                errors.push(format!(
                    "{source_context}.source_id duplicate source_id `{}`",
                    source.source_id
                ));
            }

            if source.canonical_base_asset != surface.canonical_base_asset {
                errors.push(format!(
                    "{source_context}.{} `{}` must match {context}.{} `{}`",
                    stringify!(canonical_base_asset),
                    source.canonical_base_asset,
                    stringify!(canonical_base_asset),
                    surface.canonical_base_asset,
                ));
            }
            let instrument_key = source.instrument_id.to_string();
            let data_client_id = source.data_client_id.to_string();
            match seen_source_instrument_clients.get(&instrument_key) {
                Some((existing_data_client_id, existing_context))
                    if existing_data_client_id != &data_client_id =>
                {
                    errors.push(format!(
                        "{source_context}.instrument_id `{}` with data_client_id `{data_client_id}` is also used by {existing_context} with distinct data_client_id `{existing_data_client_id}`; realized_volatility_surfaces source events do not carry data_client_id, so same-instrument RV sources must share one data client",
                        source.instrument_id,
                    ));
                }
                Some(_) => {}
                None => {
                    seen_source_instrument_clients
                        .insert(instrument_key, (data_client_id, source_context.clone()));
                }
            }

            if source.canonical_quote_asset != surface.canonical_quote_asset {
                errors.push(format!(
                    "{source_context}.{} `{}` must match {context}.{} `{}`",
                    stringify!(canonical_quote_asset),
                    source.canonical_quote_asset,
                    stringify!(canonical_quote_asset),
                    surface.canonical_quote_asset,
                ));
            }
            if !realized_volatility_source_pair_supported(source.source_class, source.sample_kind) {
                errors.push(format!(
                    "{source_context}.{} {:?} with {} {:?} is not supported by the taker realized-volatility router",
                    stringify!(source_class),
                    source.source_class,
                    stringify!(sample_kind),
                    source.sample_kind,
                ));
            }
            if source.enabled && source.counts_toward_quorum {
                enabled_quorum_sources += 1;
                match quorum_source_contract.as_ref() {
                    Some((source_class, sample_kind, existing_context))
                        if source.source_class != *source_class
                            || source.sample_kind != *sample_kind =>
                    {
                        errors.push(format!(
                            "{source_context}.source_class/sample_kind {:?}/{:?} must match enabled quorum source contract {:?}/{:?} established by {existing_context}",
                            source.source_class,
                            source.sample_kind,
                            source_class,
                            sample_kind,
                        ));
                    }
                    Some(_) => {}
                    None => {
                        quorum_source_contract = Some((
                            source.source_class,
                            source.sample_kind,
                            source_context.clone(),
                        ));
                    }
                }
            }
        }

        if policy.min_ready_sources > enabled_quorum_sources {
            errors.push(format!(
                "{context}.policy.min_ready_sources {} exceeds enabled quorum source count {}",
                policy.min_ready_sources, enabled_quorum_sources
            ));
        }
    }

    errors
}

fn realized_volatility_source_pair_supported(
    source_class: RealizedVolatilitySourceClassBlock,
    sample_kind: RealizedVolatilitySampleKindBlock,
) -> bool {
    matches!(
        (source_class, sample_kind),
        (
            RealizedVolatilitySourceClassBlock::SpotQuote,
            RealizedVolatilitySampleKindBlock::Midpoint,
        ) | (
            RealizedVolatilitySourceClassBlock::Trade,
            RealizedVolatilitySampleKindBlock::Trade,
        ) | (
            RealizedVolatilitySourceClassBlock::Index,
            RealizedVolatilitySampleKindBlock::Index,
        )
    )
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

/// Validates that a Chainlink Data Streams `rest_base_url` parses as a URL and
/// uses the `https` scheme. The signed Data Streams credentials travel in the
/// request `Authorization` header, so any non-https (or unparseable) endpoint
/// fails closed at config load — credentials must never traverse plaintext.
/// Shared by the live-strike client validator and the resolution-oracle
/// gate-provider validator so both config blocks that name the same endpoint are
/// held to one transport standard.
pub(crate) fn validate_https_rest_base_url(
    field_path: &str,
    rest_base_url: &str,
    errors: &mut Vec<String>,
) {
    match url::Url::parse(rest_base_url) {
        Ok(parsed) if parsed.scheme() != "https" => errors.push(format!(
            "{field_path} must use the https scheme (got `{scheme}`); \
             signed credentials must never be sent over an insecure transport",
            scheme = parsed.scheme()
        )),
        Ok(_) => {}
        Err(_) => errors.push(format!("{field_path} must be a valid absolute URL")),
    }
}

/// Resolves a Chainlink Data Streams `report_endpoint_path` against its
/// `rest_base_url`, failing closed against any value that would redirect the
/// credential-bearing report request off the configured endpoint. The HMAC-signed
/// Data Streams credentials travel with this request, so the path must be a rooted
/// absolute path and the joined URL must keep the base scheme, host, port, and
/// userinfo (username/password) and introduce no query or fragment of its own.
/// `url::Url::join` otherwise accepts absolute URLs (`https://other/...`) and
/// scheme-relative/authority paths (`//other/...`, `//user:pass@host/...`) that
/// silently swap the host or inject userinfo while still receiving the signed
/// credentials.
/// Shared by the live-strike client validator, the resolution-oracle gate-provider
/// validator, and the request-URL builder so the endpoint can only ever resolve to
/// the configured host. Returns the safe joined URL so the builder reuses one
/// resolution rather than re-joining.
pub(crate) fn resolve_chainlink_report_endpoint_url(
    rest_base_url: &str,
    report_endpoint_path: &str,
) -> Result<url::Url, String> {
    let base = url::Url::parse(rest_base_url)
        .map_err(|_| "must resolve against a valid absolute base URL".to_string())?;
    // Require a single rooted path. `strip_prefix` enforces the leading slash; a
    // second leading slash or backslash makes the value an authority/scheme-relative
    // reference (`//host`, `/\host`) that `url::Url::join` resolves into
    // host/userinfo/port — including same-host `//user:pass@host` forms that would
    // smuggle credentials into the signed request URL — rather than a path.
    let after_root = match report_endpoint_path.strip_prefix('/') {
        Some(after_root) => after_root,
        None => {
            return Err("must be a rooted absolute path beginning with a single slash".to_string());
        }
    };
    if after_root.starts_with('/') || after_root.starts_with('\\') {
        return Err(
            "must be a single rooted path, not a scheme-relative or authority reference"
                .to_string(),
        );
    }
    let joined = base
        .join(report_endpoint_path)
        .map_err(|_| "must be a path that resolves against the base URL".to_string())?;
    // Authoritative backstop: resolving the path must change only the path. The
    // scheme, host, port, and userinfo must match the base, and the path must
    // introduce no query or fragment of its own.
    if joined.scheme() != base.scheme()
        || joined.host_str() != base.host_str()
        || joined.port_or_known_default() != base.port_or_known_default()
        || joined.username() != base.username()
        || joined.password() != base.password()
        || joined.query().is_some()
        || joined.fragment().is_some()
    {
        return Err(
            "must not redirect off the base URL host, scheme, port, or credentials, or carry a query or fragment"
                .to_string(),
        );
    }
    Ok(joined)
}

/// Validates a Chainlink Data Streams `report_endpoint_path` config field via
/// [`resolve_chainlink_report_endpoint_url`], pushing a field-scoped error on any
/// value that would redirect the signed request off the configured host. A
/// malformed base URL is reported by the `rest_base_url` validator, so this skips
/// that case to avoid double-reporting.
pub(crate) fn validate_chainlink_report_endpoint_path(
    field_path: &str,
    rest_base_url: &str,
    report_endpoint_path: &str,
    errors: &mut Vec<String>,
) {
    if url::Url::parse(rest_base_url).is_err() {
        return;
    }
    if let Err(reason) = resolve_chainlink_report_endpoint_url(rest_base_url, report_endpoint_path)
    {
        errors.push(format!("{field_path} {reason}"));
    }
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
                "{context}.chainlink_data_streams.{field} is not a supported chainlink_data_streams provider field"
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
    let rest_base_url = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD,
        &mut errors,
    );
    if let Some(rest_base_url) = rest_base_url {
        validate_https_rest_base_url(
            &format!(
                "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REST_BASE_URL_FIELD}"
            ),
            rest_base_url,
            &mut errors,
        );
    }
    if let Some(report_endpoint_path) = required_string_field(
        table,
        &format!("{context}.chainlink_data_streams"),
        CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD,
        &mut errors,
    ) && let Some(rest_base_url) = rest_base_url
    {
        validate_chainlink_report_endpoint_path(
            &format!(
                "{context}.chainlink_data_streams.{CHAINLINK_DATA_STREAMS_REPORT_ENDPOINT_PATH_FIELD}"
            ),
            rest_base_url,
            report_endpoint_path,
            &mut errors,
        );
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
                "{binding_context}.value_kind `{value_kind}` is not supported for chainlink_data_streams price reports"
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
                "{binding_context}.feed_id must be a lowercase chainlink_data_streams feed id"
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
    match parse_decimal_string(&block.default_max_notional_per_order) {
        Ok(value) if value <= Decimal::ZERO => {
            errors.push(format!(
                "risk.default_max_notional_per_order must be a positive decimal string: `{value}`",
                value = block.default_max_notional_per_order
            ));
        }
        Ok(_) => {}
        Err(reason) => {
            errors.push(format!(
                "risk.default_max_notional_per_order is not a valid decimal string ({reason}): `{value}`",
                value = block.default_max_notional_per_order
            ));
        }
    }
    if let Some(loss_governor) = block.loss_governor.as_ref() {
        if loss_governor.enabled && loss_governor.max_snapshot_age_ns == 0 {
            errors.push(
                "risk.loss_governor.max_snapshot_age_ns must be a positive integer".to_string(),
            );
        }
        if loss_governor.enabled && loss_governor.rolling_window_ns == 0 {
            errors.push(
                "risk.loss_governor.rolling_window_ns must be a positive integer".to_string(),
            );
        }
        if loss_governor.enabled
            && loss_governor
                .active_position_pnl_max_entries
                .is_none_or(|value| value == 0)
        {
            errors.push(
                "risk.loss_governor.active_position_pnl_max_entries must be a positive integer"
                    .to_string(),
            );
        }
        if loss_governor.enabled {
            for (label, threshold) in [
                (
                    "risk.loss_governor.max_per_trade_loss",
                    loss_governor.max_per_trade_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_daily_loss",
                    loss_governor.max_daily_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_rolling_loss",
                    loss_governor.max_rolling_loss.as_deref(),
                ),
                (
                    "risk.loss_governor.max_drawdown",
                    loss_governor.max_drawdown.as_deref(),
                ),
            ] {
                if threshold.is_none() {
                    errors.push(format!("{label} must be configured when enabled"));
                }
            }
            for (label, configured) in [
                (
                    "risk.loss_governor.on_loss_breach_trading_state",
                    loss_governor.on_loss_breach_trading_state.is_some(),
                ),
                (
                    "risk.loss_governor.on_untrusted_snapshot_trading_state",
                    loss_governor.on_untrusted_snapshot_trading_state.is_some(),
                ),
                (
                    "risk.loss_governor.recovery_mode",
                    loss_governor.recovery_mode.is_some(),
                ),
                (
                    "risk.loss_governor.manual_recovery_evidence_max_path_bytes",
                    loss_governor
                        .manual_recovery_evidence_max_path_bytes
                        .is_some(),
                ),
            ] {
                if !configured {
                    errors.push(format!("{label} must be configured when enabled"));
                }
            }
            if matches!(
                loss_governor.on_untrusted_snapshot_trading_state,
                Some(LossGovernorTradingStateAction::None)
            ) {
                errors.push(
                    "risk.loss_governor.on_untrusted_snapshot_trading_state must be reducing or halted when enabled"
                        .to_string(),
                );
            }
            if loss_governor
                .manual_recovery_evidence_max_path_bytes
                .is_some_and(|limit| limit == usize::MIN)
            {
                errors.push(
                    "risk.loss_governor.manual_recovery_evidence_max_path_bytes must be a positive integer"
                        .to_string(),
                );
            }
        }
        for (label, threshold) in [
            (
                "risk.loss_governor.max_per_trade_loss",
                loss_governor.max_per_trade_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_daily_loss",
                loss_governor.max_daily_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_rolling_loss",
                loss_governor.max_rolling_loss.as_deref(),
            ),
            (
                "risk.loss_governor.max_drawdown",
                loss_governor.max_drawdown.as_deref(),
            ),
        ] {
            let Some(value) = threshold else {
                continue;
            };
            match parse_decimal_string(value) {
                Ok(decimal) if decimal <= Decimal::ZERO => {
                    errors.push(format!(
                        "{label} must be a positive decimal string: `{value}`"
                    ));
                }
                Ok(_) => {}
                Err(reason) => {
                    errors.push(format!(
                        "{label} is not a valid decimal string ({reason}): `{value}`"
                    ));
                }
            }
        }
    }
    if let Some(capital_pools) = block.capital_pools.as_ref() {
        errors.extend(validate_capital_pools(capital_pools));
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
    if let Some(kill_switch) = &block.kill_switch {
        errors.extend(validate_kill_switch_block(kill_switch));
    }
    if let Some(basket_execution) = &block.basket_execution {
        errors.extend(
            crate::bolt_v3_outcome_group_sources::validate_basket_execution(basket_execution),
        );
    }
    errors
}

fn validate_capital_pools(pools: &[CapitalPoolBlock]) -> Vec<String> {
    let mut errors = Vec::new();
    let mut pool_ids = HashSet::new();
    let mut enforced_pool_count = 0usize;

    for pool in pools {
        let label = format!("risk.capital_pools[{}]", pool.pool_id);
        if pool.enforce_submit_admission {
            enforced_pool_count += 1;
        }
        if pool.pool_id.trim().is_empty() {
            errors.push("risk.capital_pools pool_id must be a non-empty string".to_string());
        } else if !pool_ids.insert(pool.pool_id.as_str()) {
            errors.push(format!("{label}.pool_id must be unique"));
        }
        if pool.venue_id.trim().is_empty() {
            errors.push(format!("{label}.venue_id must be a non-empty string"));
        } else if pool.enforce_submit_admission
            && pool.venue_id != pool.venue_id.to_ascii_uppercase()
        {
            errors.push(format!(
                "{label}.venue_id must be canonical uppercase when submit admission enforcement is enabled"
            ));
        }
        if pool.collateral_currency.trim().is_empty() {
            errors.push(format!(
                "{label}.collateral_currency must be a non-empty string"
            ));
        }
        if pool.product_kind != "prediction_market_binary" {
            errors.push(format!(
                "{label}.product_kind must be `prediction_market_binary`"
            ));
        }
        validate_prediction_market_binary_product_metadata(pool, &label, &mut errors);
        validate_positive_decimal(
            &format!("{label}.max_pool_liability"),
            &pool.max_pool_liability,
            &mut errors,
        );
        if pool.max_snapshot_age_ns == 0 {
            errors.push(format!(
                "{label}.max_snapshot_age_ns must be a positive integer"
            ));
        }
        if pool.dedupe_retention_ns == 0 {
            errors.push(format!(
                "{label}.dedupe_retention_ns must be a positive integer"
            ));
        }
        validate_venue_spendability_source_binding(pool, &label, &mut errors);
        if let Some(min_remaining_pool_balance) =
            pool.sizing_policy.min_remaining_pool_balance.as_ref()
        {
            validate_positive_decimal(
                &format!("{label}.sizing_policy.min_remaining_pool_balance"),
                min_remaining_pool_balance,
                &mut errors,
            );
        }
        validate_positive_decimal(
            &format!("{label}.sizing_policy.fee_slippage.max_fee_liability"),
            &pool.sizing_policy.fee_slippage.max_fee_liability,
            &mut errors,
        );
        validate_positive_decimal(
            &format!("{label}.sizing_policy.fee_slippage.max_slippage_liability"),
            &pool.sizing_policy.fee_slippage.max_slippage_liability,
            &mut errors,
        );
    }

    if enforced_pool_count > 1 {
        errors.push(
            "risk.capital_pools may enable submit admission enforcement for at most one pool"
                .to_string(),
        );
    }

    errors
}

fn validate_venue_spendability_source_binding(
    pool: &CapitalPoolBlock,
    label: &str,
    errors: &mut Vec<String>,
) {
    let has_binding = pool.venue_spendability_source_path.is_some()
        || pool.venue_spendability_source_sha256.is_some()
        || pool.venue_spendability_source_max_bytes.is_some();
    if !has_binding {
        return;
    }
    if !pool.enforce_submit_admission {
        errors.push(format!(
            "{label}.venue_spendability_source_path requires enforce_submit_admission = true"
        ));
    }
    match pool.venue_spendability_source_path.as_deref() {
        Some(path) if !path.trim().is_empty() => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_path must be a non-empty string"
        )),
    }
    match pool.venue_spendability_source_sha256.as_deref() {
        Some(sha256) if is_lowercase_sha256_hex(sha256) => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_sha256 must be a lowercase sha256 hex string"
        )),
    }
    match pool.venue_spendability_source_max_bytes {
        Some(max_bytes) if max_bytes > 0 => {}
        _ => errors.push(format!(
            "{label}.venue_spendability_source_max_bytes must be positive"
        )),
    }
}

fn validate_prediction_market_binary_product_metadata(
    pool: &CapitalPoolBlock,
    label: &str,
    errors: &mut Vec<String>,
) {
    let Some(product) = pool.prediction_market_binary.as_ref() else {
        if pool.enforce_submit_admission && pool.product_kind == "prediction_market_binary" {
            errors.push(format!(
                "{label}.prediction_market_binary is required when prediction-market submit admission is enforced"
            ));
        }
        return;
    };

    if pool.product_kind != "prediction_market_binary" {
        errors.push(format!(
            "{label}.prediction_market_binary is only supported for prediction_market_binary pools"
        ));
    }
    if product.yes_instrument_id == product.no_instrument_id {
        errors.push(format!(
            "{label}.prediction_market_binary.yes_instrument_id and no_instrument_id must differ"
        ));
    }
    if product.collateral_coupled_group_id.trim().is_empty() {
        errors.push(format!(
            "{label}.prediction_market_binary.collateral_coupled_group_id must be a non-empty string"
        ));
    }
}

fn validate_positive_decimal(label: &str, value: &str, errors: &mut Vec<String>) {
    match parse_decimal_string(value) {
        Ok(decimal) if decimal <= Decimal::ZERO => {
            errors.push(format!(
                "{label} must be a positive decimal string: `{value}`"
            ));
        }
        Ok(_) => {}
        Err(reason) => {
            errors.push(format!(
                "{label} is not a valid decimal string ({reason}): `{value}`"
            ));
        }
    }
}

fn is_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_kill_switch_block(block: &KillSwitchConfigBlock) -> Vec<String> {
    if !block.enabled {
        return Vec::new();
    }

    let mut errors = Vec::new();
    let state_path = Path::new(block.state_path.trim());
    if state_path.as_os_str().is_empty()
        || state_path.is_absolute()
        || state_path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        errors.push(
            "risk.kill_switch.state_path must be a non-empty relative path under the configured root"
                .to_string(),
        );
    }
    if block.max_state_file_bytes == 0 {
        errors.push("risk.kill_switch.max_state_file_bytes must be positive".to_string());
    }
    if block.action_retry_interval_ms == 0 {
        errors.push("risk.kill_switch.action_retry_interval_ms must be positive".to_string());
    }
    if block.action_retry_timeout_ms == 0 {
        errors.push("risk.kill_switch.action_retry_timeout_ms must be positive".to_string());
    }
    if block.action_retry_interval_ms > block.action_retry_timeout_ms
        && block.action_retry_timeout_ms > 0
    {
        errors.push(
            "risk.kill_switch.action_retry_interval_ms must be <= action_retry_timeout_ms"
                .to_string(),
        );
    }
    if block.mandatory_proof_max_age_ms == 0 {
        errors.push("risk.kill_switch.mandatory_proof_max_age_ms must be positive".to_string());
    }
    if block.manual_reset_evidence_max_age_ms == 0 {
        errors
            .push("risk.kill_switch.manual_reset_evidence_max_age_ms must be positive".to_string());
    }
    if block.forced_reduction_policy_sha256.len() != 64
        || !block
            .forced_reduction_policy_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        errors.push(
            "risk.kill_switch.forced_reduction_policy_sha256 must be a 64-character SHA-256 hex digest"
                .to_string(),
        );
    }
    if block.forced_reduction_max_live_order_count == 0 {
        errors.push(
            "risk.kill_switch.forced_reduction_max_live_order_count must be positive".to_string(),
        );
    }
    match parse_decimal_string(&block.forced_reduction_max_notional_per_order) {
        Ok(notional) if notional > Decimal::ZERO => {}
        Ok(_) => errors.push(
            "risk.kill_switch.forced_reduction_max_notional_per_order must be positive"
                .to_string(),
        ),
        Err(reason) => errors.push(format!(
            "risk.kill_switch.forced_reduction_max_notional_per_order is not a valid decimal string ({reason}): `{}`",
            block.forced_reduction_max_notional_per_order
        )),
    }
    if block.authorized_operator_ids.is_empty() {
        errors.push(
            "risk.kill_switch.authorized_operator_ids must not be empty when enabled".to_string(),
        );
    }
    if block
        .authorized_operator_ids
        .iter()
        .any(|operator_id| operator_id.trim().is_empty())
    {
        errors.push(
            "risk.kill_switch.authorized_operator_ids must not contain empty values".to_string(),
        );
    }
    if block.account_ids.is_empty() {
        errors.push("risk.kill_switch.account_ids must not be empty when enabled".to_string());
    }
    if block
        .account_ids
        .iter()
        .any(|account_id| account_id.trim().is_empty())
    {
        errors.push("risk.kill_switch.account_ids must not contain empty values".to_string());
    }
    if block.instrument_ids.is_empty() {
        errors.push("risk.kill_switch.instrument_ids must not be empty when enabled".to_string());
    }
    for instrument_id in &block.instrument_ids {
        if let Err(error) = InstrumentId::from_str(instrument_id) {
            errors.push(format!(
                "risk.kill_switch.instrument_ids[`{instrument_id}`] is not a valid Nautilus instrument ID ({error})"
            ));
        }
    }
    errors
}

/// Seconds in one hour / one minute, named so the `HH:MM:SS` interval
/// computation reads as a time conversion rather than bare magic numbers.
const SECONDS_PER_HOUR: u64 = 3600;
const SECONDS_PER_MINUTE: u64 = 60;

/// Validates an NT `limit/HH:MM:SS` rate-limit string and returns the parsed
/// `(limit, interval_seconds)` so callers can reconcile the rate against a
/// venue REST egress ceiling without re-parsing.
///
/// `pub(crate)` so the maker requote-budget bridge
/// ([`crate::bolt_v3_maker_rate_budget`]) sources its submit-governor cap and
/// window from the same single parser the config validator uses, rather than
/// introducing a second rate-string interpretation.
pub(crate) fn validate_rate_limit_string(value: &str) -> Result<(u64, u64), String> {
    let (limit, interval) = value
        .split_once('/')
        .ok_or_else(|| "expected `limit/HH:MM:SS`".to_string())?;
    let limit = limit.parse::<u64>().map_err(|error| error.to_string())?;
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

    // Checked so a large `hours` value returns an Err instead of panicking
    // (debug) or wrapping to a bogus/zero interval (release). `minutes` is
    // bounded < 60 above so `minutes * SECONDS_PER_MINUTE` cannot overflow, but
    // it is kept inside the checked chain for a single readable expression.
    let interval_seconds = hours
        .checked_mul(SECONDS_PER_HOUR)
        .and_then(|h| h.checked_add(minutes * SECONDS_PER_MINUTE))
        .and_then(|s| s.checked_add(seconds))
        .ok_or_else(|| "interval seconds overflow u64".to_string())?;
    Ok((limit, interval_seconds))
}

/// Reconciles the global NT RiskEngine order submit/modify throttle against the
/// tightest configured trading-venue REST egress ceiling, derated by the venue's
/// worst-case per-order-command REST request fanout.
///
/// The RiskEngine throttle counts order *commands* while the venue HTTP quota
/// counts REST *requests*, and a single command can issue more than one request
/// (a Polymarket market submit = `get_book` + `post_order` = 2). A submit rate at
/// the raw per-minute cap therefore over-drives the venue's request quota by the
/// fanout factor; the excess does not reject early with a loud `OrderDenied` — it
/// blocks at egress (added latency, stale reference quotes), a silent failure on
/// a live-money path. Reconciling `limit * fanout` against the cap at config load
/// keeps the policy fail-loud regardless of the rendered deploy-time value, which
/// is not otherwise knowable from the repo.
///
/// NOTE (tier-1): this derates submit/modify against the per-bucket ceiling using
/// the deterministic worst-case per-command fanout only. The full shared REST
/// budget — transient retries, cancels, status queries, readiness/account probes,
/// and the fact that CLOB and Gamma are *separate* per-client buckets — is the
/// venue egress-capability contract tracked in #501.
fn validate_order_rate_within_venue_egress(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    // Fail closed on any execution venue whose REST egress model bolt-v3 does not
    // model: skipping it silently would let an unbounded submit rate through on a
    // venue we cannot reconcile against. Iterate the keyed client map so the
    // error can name the offending `clients.<id>`.
    let mut tightest: Option<(&str, crate::bolt_v3_providers::VenueEgressModel)> = None;
    for (key, client) in &root.clients {
        if client.execution.is_none() {
            continue;
        }
        let venue = client.venue.as_str();
        match crate::bolt_v3_providers::venue_egress_model(venue) {
            Some(model) => {
                // Tightest = smallest effective ceiling cap/fanout. Compare via
                // cross-multiplication (cap_a/fanout_a < cap_b/fanout_b iff
                // cap_a * fanout_b < cap_b * fanout_a) in u128 to avoid integer
                // division and any saturation.
                let tighter = tightest.is_none_or(|(_, current)| {
                    (model.cap_per_minute as u128)
                        * (current.max_rest_requests_per_order_command as u128)
                        < (current.cap_per_minute as u128)
                            * (model.max_rest_requests_per_order_command as u128)
                });
                if tighter {
                    tightest = Some((venue, model));
                }
            }
            None => errors.push(format!(
                "clients.{key} (provider={venue}) declares an [execution] block but bolt-v3 \
                 models no REST egress cap for it; cannot reconcile order rates — fail closed"
            )),
        }
    }
    let Some((venue, model)) = tightest else {
        // No modeled execution venue to reconcile against; `errors` may already
        // carry fail-closed messages for unmodeled execution venues above.
        return errors;
    };
    let cap_per_minute = model.cap_per_minute;
    let fanout = model.max_rest_requests_per_order_command;
    // Largest order-command rate per minute that keeps `limit * fanout <= cap`.
    let derated_ceiling = cap_per_minute / fanout;
    for (label, value) in [
        (
            "risk.nautilus.max_order_submit_rate",
            root.risk.nautilus.max_order_submit_rate.as_str(),
        ),
        (
            "risk.nautilus.max_order_modify_rate",
            root.risk.nautilus.max_order_modify_rate.as_str(),
        ),
    ] {
        // Only well-formed rate strings reach the ceiling check; malformed
        // strings are already reported by validate_rate_limit_string above.
        let Ok((limit, interval_seconds)) = validate_rate_limit_string(value) else {
            continue;
        };
        // Over-drives the cap iff limit/interval > (cap/fanout)/60, i.e.
        // limit * fanout * SECONDS_PER_MINUTE > cap * interval_seconds. Compared
        // in u128 so no product can saturate to u64::MAX and let an over-cap rate
        // slip through (MAX > MAX is false). validate_rate_limit_string guarantees
        // interval_seconds >= 1, so no zero-interval guard is needed.
        if (limit as u128) * (fanout as u128) * (SECONDS_PER_MINUTE as u128)
            > (cap_per_minute as u128) * (interval_seconds as u128)
        {
            errors.push(format!(
                "{label} = `{value}` over-drives the {venue} REST egress cap of \
                 {cap_per_minute}/min (nautilus HTTP_RATE_LIMIT): a single order command issues up \
                 to {fanout} REST requests (market submit = book + post), so the order rate must \
                 not exceed {derated_ceiling}/min or submits block at egress with stale reference \
                 quotes instead of failing loud — lower it to at most {derated_ceiling}/00:01:00"
            ));
        }
    }
    errors
}

fn validate_persistence_block(block: &PersistenceBlock) -> Vec<String> {
    let mut errors = Vec::new();
    if !Path::new(&block.catalog_directory).is_absolute() {
        errors.push(format!(
            "persistence.catalog_directory must be an absolute path: `{}`",
            block.catalog_directory
        ));
    }
    if let Some(required_catalog_prefix) = block.required_catalog_prefix.as_deref()
        && !Path::new(required_catalog_prefix).is_absolute()
    {
        errors.push(format!(
            "{}.{} must be an absolute path: `{}`",
            stringify!(persistence),
            stringify!(required_catalog_prefix),
            required_catalog_prefix
        ));
    }
    if block.runtime_capture_start_poll_interval_ms == 0 {
        errors.push(
            "persistence.runtime_capture_start_poll_interval_ms must be a positive integer"
                .to_string(),
        );
    }
    if block
        .min_free_bytes
        .is_some_and(|min_free_bytes| min_free_bytes == 0)
    {
        errors.push(format!(
            "{}.{} must be a positive integer",
            stringify!(persistence),
            stringify!(min_free_bytes)
        ));
    }
    if block.streaming.flush_interval_ms == 0 {
        errors
            .push("persistence.streaming.flush_interval_ms must be a positive integer".to_string());
    }
    if let Err(message) = validate_decision_evidence_relative_path(
        &block.decision_evidence.order_intents_relative_path,
    ) {
        errors.push(message);
    }
    if block
        .decision_evidence
        .recovery_evidence_max_bytes
        .is_some_and(|max_bytes| max_bytes == 0)
    {
        errors.push(
            "persistence.decision_evidence.recovery_evidence_max_bytes must be a positive integer"
                .to_string(),
        );
    }
    errors
}

fn validate_position_sizer_recovery_evidence(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let enforced_submit_admission = root
        .risk
        .capital_pools
        .as_ref()
        .is_some_and(|pools| pools.iter().any(|pool| pool.enforce_submit_admission));
    if enforced_submit_admission
        && root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes
            .is_none()
    {
        errors.push(
            "persistence.decision_evidence.recovery_evidence_max_bytes must be configured when risk.capital_pools enables submit admission enforcement"
                .to_string(),
        );
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

fn validate_clients_block(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let clients = &root.clients;
    if clients.is_empty() {
        errors.push("clients must define at least one client block".to_string());
        return errors;
    }
    for (key, client) in clients {
        errors.extend(validate_root_owned_chainlink_feed_catalog(
            root, key, client,
        ));
        let validation_client = client_with_root_chainlink_feed_catalog(root, client);
        let client = validation_client.as_ref().unwrap_or(client);
        errors.extend(crate::bolt_v3_providers::validate_client_block(key, client));
        errors.extend(validate_client_readiness_probe(key, client));
    }
    errors.extend(validate_unique_client_readiness_probe_instruments(clients));
    errors
}

fn validate_root_owned_chainlink_feed_catalog(
    root: &BoltV3RootConfig,
    key: &str,
    client: &ClientBlock,
) -> Vec<String> {
    if !uses_root_owned_chainlink_feed_catalog(client) || client.data.is_none() {
        return Vec::new();
    }
    let has_client_feed_bindings = client
        .data
        .as_ref()
        .and_then(toml::Value::as_table)
        .is_some_and(|data| data.contains_key(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD));
    let mut errors = Vec::new();
    if has_client_feed_bindings {
        errors.push(format!(
            "chainlink_data_streams.feed_bindings is root-owned; clients.{key}.data.feed_bindings must be removed so feed bindings have one configured path"
        ));
    }
    if root.chainlink_data_streams.is_none() {
        errors.push(format!(
            "chainlink_data_streams.feed_bindings must be configured for clients.{key}; clients.{key}.data.feed_bindings is not supported"
        ));
    }
    errors
}

fn uses_root_owned_chainlink_feed_catalog(client: &ClientBlock) -> bool {
    client.venue.as_str() == crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY
}

pub(crate) fn client_with_root_chainlink_feed_catalog(
    root: &BoltV3RootConfig,
    client: &ClientBlock,
) -> Option<ClientBlock> {
    let catalog = root.chainlink_data_streams.as_ref()?;
    if client.venue.as_str() != crate::bolt_v3_providers::RESOLUTION_ORACLE_VENUE_KEY {
        return None;
    }
    let data = client.data.as_ref()?.as_table()?;
    if data.contains_key(CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD) {
        return None;
    }

    let mut client = client.clone();
    let data = client.data.as_mut()?.as_table_mut()?;
    data.insert(
        CHAINLINK_DATA_STREAMS_FEED_BINDINGS_FIELD.to_string(),
        toml::Value::Array(catalog.feed_bindings.clone()),
    );
    Some(client)
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
    // A trade chunk-count probe walks the venue's full instrument universe in
    // chunks until `m` distinct markets trade; it has no fixed sample, so it
    // owns a distinct config surface (chunk_size + window, no sampling knobs).
    let is_trade_volume_probe = matches!(
        readiness_probe.market_data_kind,
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Trade
    ) && matches!(
        readiness_probe.quote_target_source,
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse
    );
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
        crate::bolt_v3_config::DataClientReadinessProbeMarketDataKind::Trade => {
            if readiness_probe.book_type.is_some() {
                errors.push(format!(
                    "clients.{key}.readiness_probe.book_type is only valid when market_data_kind = \"book\""
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
            if is_trade_volume_probe {
                // Chunk-count mode subscribes the whole universe in chunks of
                // chunk_size until `m` (min_observed_targets) distinct markets
                // trade. There is no fixed sample, so the sampling knobs are
                // rejected and the chunk knobs are required instead.
                if readiness_probe.max_metadata_quote_targets.is_some() {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.max_metadata_quote_targets is not valid for a trade chunk-count probe; configure chunk_size instead"
                    ));
                }
                if readiness_probe.allow_metadata_target_sampling.is_some() {
                    errors.push(format!(
                        "clients.{key}.readiness_probe.allow_metadata_target_sampling is not valid for a trade chunk-count probe"
                    ));
                }
                match readiness_probe.chunk_size {
                    Some(chunk_size) if chunk_size > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.chunk_size must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
                match readiness_probe.chunk_observation_window_seconds {
                    Some(window) if window > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.chunk_observation_window_seconds must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
                match readiness_probe.min_observed_targets {
                    Some(min_observed_targets) if min_observed_targets > 0 => {}
                    _ => {
                        errors.push(format!(
                            "clients.{key}.readiness_probe.min_observed_targets must be a positive integer when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
                        ));
                    }
                };
            } else {
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
    }
    if !is_trade_volume_probe {
        if readiness_probe.chunk_size.is_some() {
            errors.push(format!(
                "clients.{key}.readiness_probe.chunk_size is only valid when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
            ));
        }
        if readiness_probe.chunk_observation_window_seconds.is_some() {
            errors.push(format!(
                "clients.{key}.readiness_probe.chunk_observation_window_seconds is only valid when market_data_kind = \"trade\" and quote_target_source = \"metadata_response\""
            ));
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
                            "clients.{client_key}.readiness_probe.quote_targets.{target_id}.instrument_id `{instrument_id}` is also used by clients.{existing_client_key}.readiness_probe.quote_targets.{existing_target_id}.instrument_id; QuoteTick does not carry data_client_id, so strategy-free data-client quote probe evidence cannot distinguish data clients for the same instrument"
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

        errors.extend(
            validate_shadow_order_execution_mode_forbids_managed_venue_actions(
                &context, root, strategy,
            ),
        );
        errors.extend(validate_complete_set_activation_is_shadow_only(
            &context, root, strategy,
        ));

        if let Some(surface_id) = &strategy.realized_volatility_surface_id
            && !root
                .realized_volatility_surfaces
                .as_ref()
                .is_some_and(|surfaces| surfaces.contains_key(surface_id))
        {
            errors.push(format!(
                "{context}: {} `{surface_id}` references missing {}.{surface_id}",
                stringify!(realized_volatility_surface_id),
                stringify!(realized_volatility_surfaces),
            ));
        }

        if let Some(surface_id) = &strategy.realized_volatility_surface_id
            && let Some(surface) = root
                .realized_volatility_surfaces
                .as_ref()
                .and_then(|surfaces| surfaces.get(surface_id))
            && let Ok(target) =
                crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
            && target.underlying_asset != surface.canonical_base_asset
        {
            errors.push(format!(
                "{context}: realized_volatility_surface_id `{surface_id}` references realized_volatility_surfaces.{surface_id}.canonical_base_asset `{}`, but target.underlying_asset is `{}`",
                surface.canonical_base_asset, target.underlying_asset,
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

        errors.extend(validate_reference_current_price(&context, root, strategy));
        errors.extend(
            crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings(
                &context,
                root,
                strategy,
                default_max_notional_decimal.as_ref(),
                crate::strategy_bindings::production_validation_bindings(),
            ),
        );
    }
    errors.extend(validate_target_gate_provider_references(root, strategies));
    errors.extend(validate_chainlink_feed_binding_coverage(root, strategies));
    errors.extend(
        crate::bolt_v3_outcome_group_sources::validate_outcome_group_strategy_links(
            root, strategies,
        ),
    );

    errors
}

fn validate_complete_set_activation_is_shadow_only(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    if strategy.strategy_archetype.as_str()
        != crate::bolt_v3_outcome_group_sources::COMPLETE_SET_ARBITRAGE_KEY
        || root.runtime.order_execution_mode == BoltV3OrderExecutionMode::Shadow
    {
        return Vec::new();
    }

    vec![format!(
        "{context}: complete_set_arbitrage runtime activation is registration-only until NautilusTrader event forwarding is wired; runtime.order_execution_mode must be shadow for this substrate slice"
    )]
}

fn validate_shadow_order_execution_mode_forbids_managed_venue_actions(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    if root.runtime.order_execution_mode != BoltV3OrderExecutionMode::Shadow {
        return Vec::new();
    }

    let mut errors = Vec::new();
    for (field, enabled) in [
        (stringify!(manage_stop), strategy.manage_stop),
        (stringify!(manage_gtd_expiry), strategy.manage_gtd_expiry),
        (
            stringify!(manage_contingent_orders),
            strategy.manage_contingent_orders,
        ),
    ] {
        if enabled {
            errors.push(format!(
                "{context}: runtime.order_execution_mode=shadow requires {field}=false because it drives NautilusTrader-managed venue actions outside the shared order-execution policy"
            ));
        }
    }
    if !strategy.external_order_claims.is_empty() {
        errors.push(format!(
            "{context}: runtime.order_execution_mode=shadow requires external_order_claims=[] because claimed foreign orders are managed by NautilusTrader outside the shared order-execution policy"
        ));
    }
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
                "{}: chainlink_data_streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has no matching gate_providers.{}.chainlink_data_streams.feed_bindings entry",
                reference.context,
                reference.key.provider_id,
                reference.key.resolution_identity,
                reference.key.value_kind,
                reference.key.provider_id
            )),
            count => errors.push(format!(
                "{}: chainlink_data_streams mapping provider_id `{}` resolution_identity `{}` value_kind `{}` has {count} matching gate_providers.{}.chainlink_data_streams.feed_bindings entries; expected exactly one",
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
                    "{context} resolution_identity `{}` value_kind `{}` is not referenced by any loaded strategy chainlink_data_streams mapping",
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
) -> Vec<ResolutionFeedMappingReference> {
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
                        "{strategy_context}: target.{TARGET_GATE_SUBSCRIPTIONS_FIELD}.{role}.{TARGET_MARKET_MAPPINGS_FIELD}[{index}]: chainlink_data_streams mapping resolution_identity `{resolution_identity}` value_kind `{value_kind}` cannot resolve provider_id from mapping provider_id, provider_preference, or a single allowed_provider_ids entry"
                    ));
                    continue;
                };
                references.push(ResolutionFeedMappingReference {
                    key: ResolutionFeedBindingKey {
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
) -> BTreeMap<ResolutionFeedBindingKey, Vec<String>> {
    let mut bindings: BTreeMap<ResolutionFeedBindingKey, Vec<String>> = BTreeMap::new();
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
            let key = ResolutionFeedBindingKey {
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

fn validate_reference_current_price(
    context: &str,
    root: &BoltV3RootConfig,
    strategy: &BoltV3StrategyConfig,
) -> Vec<String> {
    let Some(reference_current_price) = &strategy.reference_current_price else {
        return Vec::new();
    };

    let mut errors = Vec::new();
    let configured: BTreeSet<&str> = reference_current_price
        .source_order
        .iter()
        .map(String::as_str)
        .collect();
    let declared: BTreeSet<&str> = reference_current_price
        .sources
        .keys()
        .map(String::as_str)
        .collect();

    if reference_current_price.asset.is_empty()
        || !reference_current_price
            .asset
            .chars()
            .all(|char| char.is_ascii_uppercase() || char.is_ascii_digit() || char == '_')
    {
        errors.push(format!(
            "{context}: reference_current_price.asset must be a normalized non-empty uppercase ASCII asset symbol containing only letters, digits, and underscores"
        ));
    }
    if let Ok(target) =
        crate::bolt_v3_market_families::target_runtime_fields_from_target(&strategy.target)
        && reference_current_price.asset != target.underlying_asset
    {
        errors.push(format!(
            "{context}: reference_current_price.asset `{}` must match target.underlying_asset `{}`",
            reference_current_price.asset, target.underlying_asset,
        ));
    }

    if reference_current_price.source_order.is_empty() {
        errors.push(format!(
            "{context}: reference_current_price.sources must be non-empty"
        ));
    }

    let mut seen_sources = HashSet::new();
    for source_id in &reference_current_price.source_order {
        if !seen_sources.insert(source_id.as_str()) {
            errors.push(format!(
                "{context}: reference_current_price.sources contains duplicate source key `{source_id}`"
            ));
        }
    }

    if reference_current_price.min_valid_sources == 0 {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources must be at least 1"
        ));
    }

    let enabled_source_count = reference_current_price
        .source_order
        .iter()
        .filter(|source_id| {
            reference_current_price
                .sources
                .get(source_id.as_str())
                .is_some_and(|source| source.enabled)
        })
        .count();
    if reference_current_price.min_valid_sources > enabled_source_count {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources {} exceeds enabled source count {}",
            reference_current_price.min_valid_sources, enabled_source_count
        ));
    }

    if reference_current_price.max_source_age_ms == 0 {
        errors.push(format!(
            "{context}: reference_current_price.max_source_age_ms must be positive"
        ));
    }

    if reference_current_price.max_source_drift_bps == 0 {
        errors.push(format!(
            "{context}: reference_current_price.max_source_drift_bps must be positive"
        ));
    }

    for source_id in configured.difference(&declared) {
        errors.push(format!(
            "{context}: reference_current_price.sources contains `{source_id}` but missing [reference_current_price.source.{source_id}]"
        ));
    }

    for source_id in declared.difference(&configured) {
        errors.push(format!(
            "{context}: [reference_current_price.source.{source_id}] is declared but not listed in reference_current_price.sources"
        ));
    }

    let mut valid_enabled_sources = enabled_source_count;
    let mut physical_source_keys: BTreeMap<(String, String, String), &str> = BTreeMap::new();
    for source_id in &reference_current_price.source_order {
        let Some(source) = reference_current_price.sources.get(source_id.as_str()) else {
            continue;
        };
        if !source.enabled {
            continue;
        }
        let Some(provider_metadata) = reference_price_provider_metadata(source.provider.as_str())
        else {
            continue;
        };
        let identifier = match provider_metadata.identifier_kind {
            ReferencePriceIdentifierKind::InstrumentId => source.instrument_id.as_deref(),
            ReferencePriceIdentifierKind::Symbol => source.symbol.as_deref(),
        };
        let Some(identifier) = identifier.filter(|value| !reference_price_field_is_blank(value))
        else {
            continue;
        };
        let key = (
            source.provider.as_str().to_string(),
            source.client_id.to_string(),
            identifier.to_string(),
        );
        if let Some(existing_source_id) = physical_source_keys.insert(key, source_id.as_str()) {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} uses the same physical reference feed as reference_current_price.source.{existing_source_id}: provider `{}`, client_id `{}`, identifier `{identifier}`",
                source.provider.as_str(),
                source.client_id,
            ));
        }
    }

    for (source_id, source) in &reference_current_price.sources {
        let provider_metadata = reference_price_provider_metadata(source.provider.as_str());
        match root.clients.get(source.client_id.as_str()) {
            None => errors.push(format!(
                "{context}: reference_current_price.source.{source_id}.client_id `{}` does not match any [clients.<id>] block",
                source.client_id
            )),
            Some(client) => {
                if let Some(provider_metadata) = provider_metadata
                    && client.venue.as_str() != provider_metadata.client_venue_key
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.client_id `{}` must reference a {} client for provider `{}`; got `{}`",
                        source.client_id,
                        provider_metadata.client_venue_key,
                        provider_metadata.provider_key,
                        client.venue.as_str()
                    ));
                }
                if client.data.is_none() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.client_id `{}` must reference a data-capable client",
                        source.client_id
                    ));
                }
            }
        }
        if source.required && !source.enabled {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} is required but disabled"
            ));
        }

        let Some(provider_metadata) = provider_metadata else {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id}.provider `{}` is unsupported",
                source.provider.as_str()
            ));
            continue;
        };

        match provider_metadata.identifier_kind {
            ReferencePriceIdentifierKind::InstrumentId => {
                let provider_key = source.provider.as_str();
                if source
                    .instrument_id
                    .as_deref()
                    .is_none_or(reference_price_field_is_blank)
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id is required for provider `{provider_key}`"
                    ));
                }
                if source.symbol.is_some() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol is unsupported for provider `{provider_key}`"
                    ));
                }
                if let Some(instrument_id) = source.instrument_id.as_deref()
                    && !reference_price_identifier_matches_asset(
                        instrument_id,
                        &reference_current_price.asset,
                    )
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` must map to reference_current_price.asset `{}`",
                        reference_current_price.asset
                    ));
                }
                if let Some(instrument_id) = source.instrument_id.as_deref() {
                    match reference_price_provider_identifier_is_configured(
                        root,
                        source.provider.as_str(),
                        instrument_id,
                    ) {
                        Ok(true) => {}
                        Ok(false) => errors.push(format!(
                            "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` is not present in provider catalog for provider `{provider_key}`"
                        )),
                        Err(message) => errors.push(format!(
                            "{context}: reference_current_price.source.{source_id}.instrument_id `{instrument_id}` could not be checked against provider catalog: {message}"
                        )),
                    }
                }
            }
            ReferencePriceIdentifierKind::Symbol => {
                let provider_key = source.provider.as_str();
                if source
                    .symbol
                    .as_deref()
                    .is_none_or(reference_price_field_is_blank)
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol is required for provider `{provider_key}`"
                    ));
                }
                if source.instrument_id.is_some() {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.instrument_id is unsupported for provider `{provider_key}`"
                    ));
                }
                if let Some(symbol) = source.symbol.as_deref()
                    && !reference_price_identifier_matches_asset(
                        symbol,
                        &reference_current_price.asset,
                    )
                {
                    errors.push(format!(
                        "{context}: reference_current_price.source.{source_id}.symbol `{symbol}` must map to reference_current_price.asset `{}`",
                        reference_current_price.asset
                    ));
                }
            }
        }

        let unsupported_asset = source.enabled
            && reference_price_source_is_unsupported(reference_current_price, source);
        if unsupported_asset && configured.contains(source_id.as_str()) {
            valid_enabled_sources = valid_enabled_sources.saturating_sub(1);
        }
        if unsupported_asset && (source.required || !configured.contains(source_id.as_str())) {
            errors.push(format!(
                "{context}: reference_current_price.source.{source_id} {} asset `{}` is unsupported",
                source.provider.as_str(),
                reference_current_price.asset
            ));
        }
    }

    if reference_current_price.min_valid_sources > valid_enabled_sources {
        errors.push(format!(
            "{context}: reference_current_price.min_valid_sources {} cannot be met by {} enabled supported source(s)",
            reference_current_price.min_valid_sources, valid_enabled_sources
        ));
    }

    errors
}

fn reference_price_field_is_blank(value: &str) -> bool {
    value.trim().is_empty() || value.trim() != value
}

fn reference_price_identifier_matches_asset(identifier: &str, asset: &str) -> bool {
    identifier
        .split(['-', '.', '/'])
        .next()
        .is_some_and(|prefix| prefix == asset)
}

pub(crate) fn parse_decimal_string(value: &str) -> Result<Decimal, String> {
    use std::str::FromStr;
    Decimal::from_str(value).map_err(|error| error.to_string())
}
