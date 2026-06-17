use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
    str::FromStr,
};

use nautilus_model::identifiers::ClientId;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_archetypes::complete_set_arbitrage::CompleteSetSubmitMode,
    bolt_v3_basket_store::BASKET_STORE_SCHEMA_VERSION,
    bolt_v3_config::{BoltV3RootConfig, ClientBlock, GateProviderFreshnessBlock, LoadedStrategy},
    bolt_v3_operator_artifacts::is_lowercase_sha256,
};

pub const COMPLETE_SET_ARBITRAGE_KEY: &str = "complete_set_arbitrage";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupSourceConfig {
    pub source_id: String,
    pub client_id: ClientId,
    pub kind: OutcomeGroupSourceKind,
    pub event_slugs: Option<Vec<String>>,
    pub market_slugs: Option<Vec<String>>,
    pub sports_market_types: Option<Vec<String>>,
    pub gamma_query: Option<GammaQueryBlock>,
    pub question: Option<u32>,
    pub expected_neg_risk_market_id: Option<String>,
    pub terminal_state_labels: Option<Vec<String>>,
    pub max_markets: Option<usize>,
    pub max_groups: Option<usize>,
    pub enabled: bool,
    pub freshness: Option<GateProviderFreshnessBlock>,
    pub order_constraints: Option<OutcomeGroupOrderConstraintsBlock>,
    pub role_bindings: Option<OutcomeGroupRoleBindingsBlock>,
    pub settlement_rules: Option<OutcomeGroupSettlementRulesBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupSourceKind {
    #[serde(rename = "polymarket_gamma_event")]
    GammaEvent,
    #[serde(rename = "polymarket_gamma_market_slug")]
    GammaMarketSlug,
    #[serde(rename = "polymarket_gamma_query")]
    GammaQuery,
    #[serde(rename = "hyperliquid_hip4")]
    Hip4,
}

impl OutcomeGroupSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GammaEvent => "polymarket_gamma_event",
            Self::GammaMarketSlug => "polymarket_gamma_market_slug",
            Self::GammaQuery => "polymarket_gamma_query",
            Self::Hip4 => "hyperliquid_hip4",
        }
    }

    fn is_polymarket(self) -> bool {
        matches!(
            self,
            Self::GammaEvent | Self::GammaMarketSlug | Self::GammaQuery
        )
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GammaQueryBlock {
    pub search: Option<String>,
    pub event_query: Option<String>,
    pub market_query: Option<String>,
    pub tag_id: Option<String>,
    pub sports_market_types: Option<Vec<String>>,
    pub max_events: Option<usize>,
    pub max_markets: usize,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupOrderConstraintsBlock {
    pub default_min_quantity: Option<String>,
    pub default_min_notional: Option<String>,
    pub per_leg: Option<Vec<OutcomeGroupPerLegOrderConstraintsBlock>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupPerLegOrderConstraintsBlock {
    pub native_leg_id: String,
    pub min_quantity: Option<String>,
    pub min_notional: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupRoleBindingsBlock {
    pub kind: OutcomeGroupRoleBindingKind,
    pub attestation_sha256: String,
    pub legs: Vec<OutcomeGroupRoleBindingLegBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupRoleBindingKind {
    OperatorAttestedPositiveSide,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupRoleBindingLegBlock {
    pub terminal_state_label: String,
    pub pays_on_terminal_state_native_leg_id: String,
    pub pays_unless_terminal_state_native_leg_id: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupSettlementRulesBlock {
    pub settlement_contract_id: String,
    pub settlement_source_kind: OutcomeGroupSettlementSourceKind,
    pub terminal_state_convention: OutcomeGroupTerminalStateConvention,
    pub void_policy: OutcomeGroupVoidPolicy,
    pub rounding_policy: OutcomeGroupRoundingPolicy,
    pub timing_policy: OutcomeGroupTimingPolicy,
    pub attestation_sha256: String,
    pub non_standard_terminal_payouts:
        Option<BTreeMap<String, OutcomeGroupNonStandardTerminalPayoutBlock>>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupSettlementSourceKind {
    #[serde(rename = "polymarket_ctf_uma")]
    CtfUma,
    #[serde(rename = "hyperliquid_outcome_question")]
    OutcomeQuestion,
    OperatorAttestedContract,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupTerminalStateConvention {
    ExactlyOneWinner,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupVoidPolicy {
    RefundAllLegs,
    OperatorAttestedFallback,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupRoundingPolicy {
    DecimalExact,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupTimingPolicy {
    VenueFinalResolution,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupNonStandardTerminalPayoutBlock {
    pub convention: OutcomeGroupRefundConvention,
    pub terminal_state_label: String,
    pub legs: Vec<OutcomeGroupNonStandardPayoutLegBlock>,
    pub attestation_sha256: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeGroupRefundConvention {
    OperatorAttestedStaticPayoutPerUnit,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OutcomeGroupNonStandardPayoutLegBlock {
    pub outcome_label: String,
    pub side_label: String,
    pub payout_per_unit: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BasketExecutionRiskBlock {
    pub enabled: bool,
    pub state_path: String,
    pub schema_version: u32,
    pub max_state_file_bytes: u64,
    pub recovery_policy: BasketExecutionRecoveryPolicy,
    pub max_recovery_age_ms: u64,
    pub max_metadata_age_ms: u64,
    pub repair: Option<BasketExecutionBoundedPolicyBlock>,
    pub unwind: Option<BasketExecutionBoundedPolicyBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BasketExecutionRecoveryPolicy {
    FailClosedReconcileBeforeNewBaskets,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BasketExecutionBoundedPolicyBlock {
    pub max_retries: u32,
    pub max_book_age_ms: u64,
    pub max_slippage_bps: u32,
    pub max_depth_levels: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutcomeGroupFreshnessEvidence {
    pub local_receive_unix_ms: u64,
    pub provider_event_unix_ms: Option<u64>,
    pub clock_skew_ms: Option<i64>,
    pub event_time_clock_available: bool,
}

pub fn outcome_group_observation_is_fresh(
    now_unix_ms: u64,
    observed_unix_ms: u64,
    max_age_ms: u64,
    max_clock_skew_ms: Option<u64>,
) -> bool {
    if observed_unix_ms > now_unix_ms {
        return match max_clock_skew_ms {
            Some(max_clock_skew_ms) => observed_unix_ms - now_unix_ms <= max_clock_skew_ms,
            None => false,
        };
    }

    now_unix_ms - observed_unix_ms <= max_age_ms
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetArbitrageParametersBlock {
    pub runtime: CompleteSetArbitrageRuntimeBlock,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetArbitrageRuntimeBlock {
    pub min_edge_bps: i64,
    pub max_basket_notional: String,
    pub max_open_baskets: u32,
    pub submit_mode: String,
    pub vwap_depth_limit_bps: u64,
    pub slippage_buffer_bps: u64,
    pub max_repair_attempts: u32,
    pub max_unwind_attempts: u32,
}

pub fn validate_root_sources(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen_source_ids = BTreeSet::new();
    for source in configured_sources(root) {
        let source_id = source.source_id.as_str();
        if source_id.trim().is_empty() || source_id.trim() != source_id {
            errors.push(
                "outcome_group_sources source_id must be non-empty without surrounding whitespace"
                    .to_string(),
            );
            continue;
        }
        if !seen_source_ids.insert(source_id.to_string()) {
            errors.push(format!(
                "outcome_group_sources source_id `{source_id}` is duplicated"
            ));
        }
        let context = format!("outcome_group_sources.{source_id}");
        errors.extend(validate_source(root, &context, source));
    }
    errors
}

pub fn validate_basket_execution(block: &BasketExecutionRiskBlock) -> Vec<String> {
    if !block.enabled {
        return Vec::new();
    }
    let mut errors = Vec::new();
    let state_path = Path::new(block.state_path.trim());
    if state_path.as_os_str().is_empty()
        || state_path.is_absolute()
        || state_path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || block.state_path.trim() != block.state_path
    {
        errors.push(
            "risk.basket_execution.state_path must be a non-empty relative path under the configured root"
                .to_string(),
        );
    }
    if block.schema_version != BASKET_STORE_SCHEMA_VERSION {
        errors.push(format!(
            "risk.basket_execution.schema_version={} is unsupported by this build",
            block.schema_version
        ));
    }
    if block.max_state_file_bytes == 0 {
        errors.push("risk.basket_execution.max_state_file_bytes must be positive".to_string());
    }
    if block.max_recovery_age_ms == 0 {
        errors.push("risk.basket_execution.max_recovery_age_ms must be positive".to_string());
    }
    if block.max_metadata_age_ms == 0 {
        errors.push("risk.basket_execution.max_metadata_age_ms must be positive".to_string());
    }
    validate_basket_execution_bounded_policy(
        block.repair.as_ref(),
        "risk.basket_execution.repair",
        &mut errors,
    );
    validate_basket_execution_bounded_policy(
        block.unwind.as_ref(),
        "risk.basket_execution.unwind",
        &mut errors,
    );
    errors
}

fn validate_basket_execution_bounded_policy(
    block: Option<&BasketExecutionBoundedPolicyBlock>,
    context: &str,
    errors: &mut Vec<String>,
) {
    let Some(block) = block else {
        errors.push(format!(
            "{context} must be configured before live basket execution"
        ));
        return;
    };
    if block.max_retries == 0 {
        errors.push(format!("{context}.max_retries must be positive"));
    }
    if block.max_book_age_ms == 0 {
        errors.push(format!("{context}.max_book_age_ms must be positive"));
    }
    if block.max_slippage_bps == 0 {
        errors.push(format!("{context}.max_slippage_bps must be positive"));
    }
    if block.max_depth_levels == 0 {
        errors.push(format!("{context}.max_depth_levels must be positive"));
    }
}

pub fn validate_outcome_group_strategy_links(
    root: &BoltV3RootConfig,
    strategies: &[LoadedStrategy],
) -> Vec<String> {
    let mut errors = Vec::new();
    let sources_by_id: BTreeMap<&str, &OutcomeGroupSourceConfig> = configured_sources(root)
        .iter()
        .map(|source| (source.source_id.as_str(), source))
        .collect();

    for loaded in strategies {
        let strategy = &loaded.config;
        let context = format!("strategy `{}`", loaded.relative_path);
        let Ok(target) = crate::bolt_v3_market_families::outcome_group::deserialize_target_block(
            &strategy.target,
        ) else {
            continue;
        };

        if root
            .risk
            .basket_execution
            .as_ref()
            .is_none_or(|basket| !basket.enabled)
        {
            errors.push(format!(
                "risk.basket_execution is required when {context} uses outcome_group basket execution"
            ));
        }

        let runtime = if strategy.strategy_archetype.as_str() == COMPLETE_SET_ARBITRAGE_KEY {
            validate_complete_set_runtime_parameters(&context, &strategy.parameters, &mut errors)
        } else {
            None
        };
        for source_id in &target.group_sources {
            let Some(source) = sources_by_id.get(source_id.as_str()) else {
                errors.push(format!(
                    "{context}: target.group_sources references unknown outcome_group_sources source_id `{source_id}`"
                ));
                continue;
            };
            if !source.enabled {
                errors.push(format!(
                    "{context}: target.group_sources[`{source_id}`] references a disabled outcome_group_sources block"
                ));
            }
            if source.client_id != strategy.execution_client_id {
                errors.push(format!(
                    "{context}: target.group_sources[`{source_id}`] client_id `{}` must match execution_client_id `{}`",
                    source.client_id, strategy.execution_client_id
                ));
            }
            if matches!(runtime, Some(CompleteSetSubmitMode::Ioc)) {
                errors.extend(validate_min_notional_required_for_submit_mode(
                    source,
                    CompleteSetSubmitMode::Ioc.as_config(),
                    &format!("{context}: outcome_group_sources.{source_id}"),
                ));
            }
        }
    }

    errors
}

fn configured_sources(root: &BoltV3RootConfig) -> &[OutcomeGroupSourceConfig] {
    match root.outcome_group_sources.as_deref() {
        Some(sources) => sources,
        None => &[],
    }
}

fn validate_source(
    root: &BoltV3RootConfig,
    context: &str,
    source: &OutcomeGroupSourceConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(validate_source_client(root, context, source));
    errors.extend(validate_source_selectors(context, source));
    errors.extend(validate_terminal_state_labels(context, source));
    errors.extend(validate_freshness(context, source.freshness.as_ref()));
    errors.extend(validate_order_constraints(
        context,
        source.order_constraints.as_ref(),
    ));
    errors.extend(validate_role_bindings(context, source));
    errors.extend(validate_settlement_rules(
        context,
        source.settlement_rules.as_ref(),
    ));
    errors
}

fn validate_source_client(
    root: &BoltV3RootConfig,
    context: &str,
    source: &OutcomeGroupSourceConfig,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(client) = root.clients.get(source.client_id.as_str()) else {
        errors.push(format!(
            "{context}.client_id `{}` does not match any [clients.<id>] block",
            source.client_id
        ));
        return errors;
    };
    match source.kind {
        OutcomeGroupSourceKind::GammaEvent
        | OutcomeGroupSourceKind::GammaMarketSlug
        | OutcomeGroupSourceKind::GammaQuery => {
            errors.extend(validate_client_venue(
                context,
                client,
                crate::bolt_v3_providers::OUTCOME_GROUP_POLYMARKET_VENUE_KEY,
            ));
            errors.extend(validate_metadata_refresh(
                root,
                context,
                "polymarket data",
                client,
            ));
        }
        OutcomeGroupSourceKind::Hip4 => {
            errors.extend(validate_client_venue(
                context,
                client,
                crate::bolt_v3_providers::OUTCOME_GROUP_HIP4_VENUE_KEY,
            ));
            errors.extend(validate_metadata_refresh(
                root,
                context,
                "hyperliquid data",
                client,
            ));
        }
    }
    errors
}

fn validate_client_venue(context: &str, client: &ClientBlock, expected: &str) -> Vec<String> {
    if client.venue.as_str() == expected {
        Vec::new()
    } else {
        vec![format!(
            "{context}.client_id venue `{}` must be `{expected}` for this outcome-group source kind",
            client.venue
        )]
    }
}

fn validate_metadata_refresh(
    root: &BoltV3RootConfig,
    context: &str,
    provider_label: &str,
    client: &ClientBlock,
) -> Vec<String> {
    let mut errors = Vec::new();
    let update_mins = match crate::bolt_v3_providers::metadata_refresh_interval_mins(client) {
        Ok(Some(value)) => value,
        Ok(None) => {
            errors.push(format!(
                "{context}.client_id must reference a data-capable client with [{provider_label}] config"
            ));
            return errors;
        }
        Err(error) => {
            errors.push(format!(
                "{context}.client_id data block is not valid {provider_label} config: {error}"
            ));
            return errors;
        }
    };
    if update_mins == 0 {
        errors.push(format!(
            "{context}.client_id data.update_instruments_interval_mins must be positive for outcome-group metadata refresh"
        ));
        return errors;
    }
    let Some(refresh_ms) = update_mins.checked_mul(60_000) else {
        errors.push(format!(
            "{context}.client_id data.update_instruments_interval_mins overflows metadata refresh milliseconds"
        ));
        return errors;
    };
    if let Some(basket) = &root.risk.basket_execution
        && basket.enabled
        && basket.max_metadata_age_ms < refresh_ms
    {
        errors.push(format!(
            "{context}.client_id metadata refresh interval {refresh_ms}ms exceeds risk.basket_execution.max_metadata_age_ms {}",
            basket.max_metadata_age_ms
        ));
    }
    errors
}

fn validate_source_selectors(context: &str, source: &OutcomeGroupSourceConfig) -> Vec<String> {
    let mut errors = Vec::new();
    match source.kind {
        OutcomeGroupSourceKind::GammaEvent => {
            if source
                .event_slugs
                .as_ref()
                .is_none_or(|values| values.is_empty())
            {
                errors.push(format!(
                    "{context}.event_slugs must contain at least one configured event slug"
                ));
            }
            validate_positive_cap(&mut errors, context, "max_markets", source.max_markets);
        }
        OutcomeGroupSourceKind::GammaMarketSlug => {
            if source
                .market_slugs
                .as_ref()
                .is_none_or(|values| values.is_empty())
            {
                errors.push(format!(
                    "{context}.market_slugs must contain at least one configured market slug"
                ));
            }
        }
        OutcomeGroupSourceKind::GammaQuery => {
            let Some(query) = source.gamma_query.as_ref() else {
                errors.push(format!("{context}.gamma_query is required"));
                return errors;
            };
            let has_scoping_selector = [
                query.search.as_deref(),
                query.event_query.as_deref(),
                query.market_query.as_deref(),
                query.tag_id.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|value| !value.trim().is_empty());
            if !has_scoping_selector {
                errors.push(format!(
                    "{context}.gamma_query must include at least one bounded selector"
                ));
            }
            if (query
                .search
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
                || query
                    .market_query
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty()))
                && query
                    .sports_market_types
                    .as_ref()
                    .is_some_and(|values| !values.is_empty())
            {
                errors.push(format!(
                    "{context}.gamma_query.sports_market_types cannot be combined with search or market_query"
                ));
            }
            if query.max_markets == 0 {
                errors.push(format!(
                    "{context}.gamma_query.max_markets must be positive"
                ));
            }
            if query.max_events.is_some_and(|max_events| max_events == 0) {
                errors.push(format!(
                    "{context}.gamma_query.max_events must be positive when configured"
                ));
            }
        }
        OutcomeGroupSourceKind::Hip4 => {
            if source.question.is_none() {
                errors.push(format!(
                    "{context}.question must configure exactly one NT OutcomeQuestion.question"
                ));
            }
            validate_positive_cap(&mut errors, context, "max_groups", source.max_groups);
        }
    }
    if source.kind.is_polymarket()
        && source
            .expected_neg_risk_market_id
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
    {
        errors.push(format!(
            "{context}.expected_neg_risk_market_id is required for {}",
            source.kind.as_str()
        ));
    }
    errors
}

fn validate_positive_cap(errors: &mut Vec<String>, context: &str, field: &str, cap: Option<usize>) {
    match cap {
        Some(value) if value > 0 => {}
        _ => errors.push(format!("{context}.{field} must be positive")),
    }
}

fn validate_terminal_state_labels(context: &str, source: &OutcomeGroupSourceConfig) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(labels) = source.terminal_state_labels.as_ref() else {
        return vec![format!("{context}.terminal_state_labels is required")];
    };
    if labels.is_empty() {
        errors.push(format!("{context}.terminal_state_labels must not be empty"));
    }
    let mut seen = BTreeSet::new();
    for label in labels {
        if label.trim().is_empty() || label.trim() != label {
            errors.push(format!(
                "{context}.terminal_state_labels entries must be non-empty without surrounding whitespace"
            ));
        }
        if !seen.insert(label.as_str()) {
            errors.push(format!(
                "{context}.terminal_state_labels contains duplicate label `{label}`"
            ));
        }
    }
    errors
}

fn validate_freshness(
    context: &str,
    freshness: Option<&GateProviderFreshnessBlock>,
) -> Vec<String> {
    let Some(freshness) = freshness else {
        return vec![format!("{context}.freshness is required")];
    };
    let mut errors = Vec::new();
    match freshness.max_age_ms {
        Some(0) => errors.push(format!("{context}.freshness.max_age_ms must be positive")),
        Some(_) => {}
        None => errors.push(format!("{context}.freshness.max_age_ms is required")),
    }
    match freshness.max_clock_skew_ms {
        Some(0) => errors.push(format!(
            "{context}.freshness.max_clock_skew_ms must be positive"
        )),
        Some(_) => {}
        None => errors.push(format!("{context}.freshness.max_clock_skew_ms is required")),
    }
    if let (Some(max_age_ms), Some(max_clock_skew_ms)) =
        (freshness.max_age_ms, freshness.max_clock_skew_ms)
        && max_clock_skew_ms > max_age_ms
    {
        errors.push(format!(
            "{context}.freshness.max_clock_skew_ms must be less than or equal to {context}.freshness.max_age_ms"
        ));
    }
    errors
}

fn validate_order_constraints(
    context: &str,
    constraints: Option<&OutcomeGroupOrderConstraintsBlock>,
) -> Vec<String> {
    let Some(constraints) = constraints else {
        return vec![format!("{context}.order_constraints is required")];
    };
    let mut errors = Vec::new();
    let has_default_min_quantity = match constraints.default_min_quantity.as_deref() {
        Some(value) => validate_positive_decimal_string(
            &mut errors,
            &format!("{context}.order_constraints.default_min_quantity"),
            value,
        ),
        None => false,
    };
    if let Some(value) = constraints.default_min_notional.as_deref() {
        validate_positive_decimal_string(
            &mut errors,
            &format!("{context}.order_constraints.default_min_notional"),
            value,
        );
    }
    let per_leg = constraints.per_leg.as_deref().unwrap_or(&[]);
    if !has_default_min_quantity && per_leg.is_empty() {
        errors.push(format!(
            "{context}.order_constraints.default_min_quantity is required unless per-leg min_quantity is configured"
        ));
    }
    let mut seen_native_legs = BTreeSet::new();
    for leg in per_leg {
        if leg.native_leg_id.trim().is_empty() || leg.native_leg_id.trim() != leg.native_leg_id {
            errors.push(format!(
                "{context}.order_constraints.per_leg.native_leg_id must be non-empty without surrounding whitespace"
            ));
        }
        if !seen_native_legs.insert(leg.native_leg_id.as_str()) {
            errors.push(format!(
                "{context}.order_constraints.per_leg native_leg_id `{}` is duplicated",
                leg.native_leg_id
            ));
        }
        if let Some(value) = leg.min_quantity.as_deref() {
            validate_positive_decimal_string(
                &mut errors,
                &format!(
                    "{context}.order_constraints.per_leg[`{}`].min_quantity",
                    leg.native_leg_id
                ),
                value,
            );
        }
        if let Some(value) = leg.min_notional.as_deref() {
            validate_positive_decimal_string(
                &mut errors,
                &format!(
                    "{context}.order_constraints.per_leg[`{}`].min_notional",
                    leg.native_leg_id
                ),
                value,
            );
        }
    }
    errors
}

fn validate_role_bindings(context: &str, source: &OutcomeGroupSourceConfig) -> Vec<String> {
    if !source.kind.is_polymarket() {
        return Vec::new();
    }
    let Some(role_bindings) = source.role_bindings.as_ref() else {
        return vec![format!(
            "{context}.role_bindings is required for {}",
            source.kind.as_str()
        )];
    };
    let mut errors = Vec::new();
    if !is_lowercase_sha256(&role_bindings.attestation_sha256) {
        errors.push(format!(
            "{context}.role_bindings.attestation_sha256 must be a lowercase 64-character SHA-256 hex digest"
        ));
    }
    if role_bindings.legs.is_empty() {
        errors.push(format!("{context}.role_bindings.legs must not be empty"));
    }
    let labels = match source.terminal_state_labels.as_ref() {
        Some(values) => values.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        None => BTreeSet::new(),
    };
    let mut seen_labels = BTreeSet::new();
    for leg in &role_bindings.legs {
        if !labels.is_empty() && !labels.contains(leg.terminal_state_label.as_str()) {
            errors.push(format!(
                "{context}.role_bindings.legs terminal_state_label `{}` is not declared in terminal_state_labels",
                leg.terminal_state_label
            ));
        }
        if !seen_labels.insert(leg.terminal_state_label.as_str()) {
            errors.push(format!(
                "{context}.role_bindings.legs terminal_state_label `{}` is duplicated",
                leg.terminal_state_label
            ));
        }
        for (field, value) in [
            (
                "pays_on_terminal_state_native_leg_id",
                &leg.pays_on_terminal_state_native_leg_id,
            ),
            (
                "pays_unless_terminal_state_native_leg_id",
                &leg.pays_unless_terminal_state_native_leg_id,
            ),
        ] {
            if value.trim().is_empty() || value.trim() != value {
                errors.push(format!(
                    "{context}.role_bindings.legs.{field} must be non-empty without surrounding whitespace"
                ));
            }
        }
    }
    if !labels.is_empty() && seen_labels != labels {
        errors.push(format!(
            "{context}.role_bindings.legs must bind exactly one leg pair for every terminal_state_labels entry"
        ));
    }
    errors
}

fn validate_settlement_rules(
    context: &str,
    settlement_rules: Option<&OutcomeGroupSettlementRulesBlock>,
) -> Vec<String> {
    let Some(settlement_rules) = settlement_rules else {
        return vec![format!("{context}.settlement_rules is required")];
    };
    let mut errors = Vec::new();
    if settlement_rules.settlement_contract_id.trim().is_empty()
        || settlement_rules.settlement_contract_id.trim() != settlement_rules.settlement_contract_id
    {
        errors.push(format!(
            "{context}.settlement_rules.settlement_contract_id must be non-empty without surrounding whitespace"
        ));
    }
    if !is_lowercase_sha256(&settlement_rules.attestation_sha256) {
        errors.push(format!(
            "{context}.settlement_rules.attestation_sha256 must be a lowercase 64-character SHA-256 hex digest"
        ));
    }
    let Some(non_standard) = settlement_rules.non_standard_terminal_payouts.as_ref() else {
        errors.push(format!(
            "{context}.settlement_rules.non_standard_terminal_payouts must not be empty"
        ));
        return errors;
    };
    if non_standard.is_empty() {
        errors.push(format!(
            "{context}.settlement_rules.non_standard_terminal_payouts must not be empty"
        ));
    }
    for (payout_id, payout) in non_standard {
        if payout.terminal_state_label.trim().is_empty() {
            errors.push(format!(
                "{context}.settlement_rules.non_standard_terminal_payouts.{payout_id}.terminal_state_label must be non-empty"
            ));
        }
        if !is_lowercase_sha256(&payout.attestation_sha256) {
            errors.push(format!(
                "{context}.settlement_rules.non_standard_terminal_payouts.{payout_id}.attestation_sha256 must be a lowercase 64-character SHA-256 hex digest"
            ));
        }
        if payout.legs.is_empty() {
            errors.push(format!(
                "{context}.settlement_rules.non_standard_terminal_payouts.{payout_id}.legs must not be empty"
            ));
        }
        for leg in &payout.legs {
            validate_decimal_string(
                &mut errors,
                &format!(
                    "{context}.settlement_rules.non_standard_terminal_payouts.{payout_id}.legs.payout_per_unit"
                ),
                &leg.payout_per_unit,
            );
        }
    }
    errors
}

fn validate_complete_set_runtime_parameters(
    context: &str,
    parameters: &toml::Value,
    errors: &mut Vec<String>,
) -> Option<CompleteSetSubmitMode> {
    let Some(runtime) = parameters.get("runtime").and_then(toml::Value::as_table) else {
        errors.push(format!("{context}: parameters.runtime is required"));
        return None;
    };
    for field in [
        "min_edge_bps",
        "max_basket_notional",
        "max_open_baskets",
        "submit_mode",
        "vwap_depth_limit_bps",
        "slippage_buffer_bps",
        "max_repair_attempts",
        "max_unwind_attempts",
    ] {
        if !runtime.contains_key(field) {
            errors.push(format!("{context}: parameters.runtime.{field} is required"));
        }
    }
    let parsed = match parameters
        .clone()
        .try_into::<CompleteSetArbitrageParametersBlock>()
    {
        Ok(value) => value,
        Err(error) => {
            if let Some(submit_mode) = runtime.get("submit_mode").and_then(toml::Value::as_str)
                && CompleteSetSubmitMode::from_config(submit_mode).is_none()
            {
                errors.push(format!(
                    "{context}: parameters.runtime.submit_mode `{submit_mode}` is not supported"
                ));
                return None;
            }
            errors.push(format!(
                "{context}: parameters block is not a valid `{COMPLETE_SET_ARBITRAGE_KEY}` [parameters] block: {error}"
            ));
            return None;
        }
    };
    let runtime = parsed.runtime;
    if runtime.min_edge_bps <= 0 {
        errors.push(format!(
            "{context}: parameters.runtime.min_edge_bps must be positive"
        ));
    }
    validate_positive_decimal_string(
        errors,
        &format!("{context}: parameters.runtime.max_basket_notional"),
        &runtime.max_basket_notional,
    );
    if runtime.max_open_baskets == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.max_open_baskets must be positive"
        ));
    }
    if runtime.vwap_depth_limit_bps == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.vwap_depth_limit_bps must be positive"
        ));
    }
    if runtime.slippage_buffer_bps == 0 {
        errors.push(format!(
            "{context}: parameters.runtime.slippage_buffer_bps must be positive"
        ));
    }
    match CompleteSetSubmitMode::from_config(runtime.submit_mode.as_str()) {
        Some(mode) => Some(mode),
        None => {
            errors.push(format!(
                "{context}: parameters.runtime.submit_mode `{}` is not supported",
                runtime.submit_mode
            ));
            None
        }
    }
}

fn validate_min_notional_required_for_submit_mode(
    source: &OutcomeGroupSourceConfig,
    submit_mode: &str,
    context: &str,
) -> Vec<String> {
    let mut errors = Vec::new();
    let Some(constraints) = source.order_constraints.as_ref() else {
        return errors;
    };
    if match constraints
        .default_min_notional
        .as_deref()
        .map(parse_decimal)
    {
        Some(Ok(decimal)) => decimal > Decimal::ZERO,
        Some(Err(_)) | None => false,
    } {
        return errors;
    }
    let per_leg = constraints.per_leg.as_deref().unwrap_or(&[]);
    if per_leg.is_empty()
        || per_leg
            .iter()
            .any(per_leg_min_notional_missing_or_non_positive)
    {
        errors.push(format!(
            "{context}.order_constraints.default_min_notional is required for submit_mode `{submit_mode}` unless every per-leg min_notional is positive"
        ));
    }
    errors
}

fn per_leg_min_notional_missing_or_non_positive(
    leg: &OutcomeGroupPerLegOrderConstraintsBlock,
) -> bool {
    match leg.min_notional.as_deref().map(parse_decimal) {
        Some(Ok(decimal)) => decimal <= Decimal::ZERO,
        Some(Err(_)) | None => true,
    }
}

fn validate_positive_decimal_string(errors: &mut Vec<String>, field: &str, value: &str) -> bool {
    match parse_decimal(value) {
        Ok(decimal) if decimal > Decimal::ZERO => true,
        Ok(_) => {
            errors.push(format!("{field} must be positive"));
            false
        }
        Err(reason) => {
            errors.push(format!("{field} is not a valid decimal string ({reason})"));
            false
        }
    }
}

fn validate_decimal_string(errors: &mut Vec<String>, field: &str, value: &str) {
    if let Err(reason) = parse_decimal(value) {
        errors.push(format!("{field} is not a valid decimal string ({reason})"));
    }
}

fn parse_decimal(value: &str) -> Result<Decimal, String> {
    Decimal::from_str(value).map_err(|error| error.to_string())
}
