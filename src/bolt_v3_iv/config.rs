use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    audit::IvAuditPolicy,
    authz::{IvAuthorizationMode, IvSelectorAuthorization},
    bounds::{IvBoundUnit, IvNumericBounds},
    derive::{IvDerivedInputPolicy, IvDerivedInputSet, IvHelperOutput, IvHelperPolicy},
    policy::{IvFallbackPolicy, IvInterpolationPolicy, IvProjectionPolicy, IvQuorumPolicy},
    selector::IvSelector,
    subscription::{IvProfileSubscriptionConfig, IvSourceSubscriptionConfig, plan_profile_start},
    types::{IvProductKind, IvSourceKind},
};

pub const SUPPORTED_IV_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvRootConfig {
    pub schema_version: u32,
    pub profiles: Vec<IvProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvProfile {
    pub profile_id: String,
    pub strategy_authorizations: Vec<IvSelectorAuthorization>,
    pub enabled_products: BTreeSet<IvProductKind>,
    pub max_raw_events: usize,
    pub max_indexed_points: usize,
    pub max_smiles: usize,
    pub max_surfaces: usize,
    pub max_derived_points: usize,
    pub max_source_health_events: usize,
    pub max_source_event_age_ns: Option<u64>,
    pub max_source_event_future_skew_ns: u64,
    pub input_bounds: IvNumericBounds,
    pub audit_policy: IvAuditPolicy,
    pub projection_policies: Vec<IvProjectionPolicy>,
    pub interpolation_policies: Vec<IvInterpolationPolicy>,
    pub fallback_policies: Vec<IvFallbackPolicy>,
    pub quorum_policies: Vec<IvQuorumPolicy>,
    pub helper_policies: Vec<IvHelperPolicy>,
    pub derived_input_policies: Vec<IvDerivedInputPolicy>,
    pub derived_inputs: Vec<IvDerivedInputSet>,
    pub sources: Vec<IvSourceConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvSourceConfig {
    pub source_id: String,
    pub selector_fingerprint: String,
    pub source_kind: IvSourceKind,
    pub client_id: String,
    pub subscription_generation: u64,
    pub accepted_conventions: BTreeSet<String>,
    pub nt_provenance: IvSourceNtProvenance,
    pub selector: IvSelector,
    pub params: toml::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IvSourceNtProvenance {
    pub nt_revision: String,
    pub nt_evidence_path: String,
    pub nt_symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvConfigError {
    Parse(String),
    Validation(Vec<String>),
}

pub fn load_iv_config_from_toml(text: &str) -> Result<IvRootConfig, IvConfigError> {
    let config = toml::from_str::<IvRootConfig>(text)
        .map_err(|error| IvConfigError::Parse(error.to_string()))?;
    let errors = validate_iv_root_config(&config);

    if errors.is_empty() {
        Ok(config)
    } else {
        Err(IvConfigError::Validation(errors))
    }
}

pub fn validate_iv_root_config(config: &IvRootConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if config.schema_version != SUPPORTED_IV_SCHEMA_VERSION {
        errors.push(format!(
            "iv.schema_version={} is unsupported by this build (only {} is currently supported)",
            config.schema_version, SUPPORTED_IV_SCHEMA_VERSION
        ));
    }
    if config.profiles.is_empty() {
        errors.push("iv.profiles must contain at least one profile".to_string());
    }

    let mut seen_profile_ids = BTreeSet::new();
    for profile in &config.profiles {
        if !seen_profile_ids.insert(profile.profile_id.as_str()) {
            errors.push(format!(
                "iv.profiles.{} profile_id is duplicated",
                profile.profile_id
            ));
        }
        errors.extend(validate_profile(profile));
    }

    errors
}

impl IvProfile {
    pub fn subscription_config(&self) -> IvProfileSubscriptionConfig {
        IvProfileSubscriptionConfig {
            profile_id: self.profile_id.clone(),
            sources: self
                .sources
                .iter()
                .map(|source| IvSourceSubscriptionConfig {
                    source_id: source.source_id.clone(),
                    source_kind: source.source_kind,
                    client_id: source.client_id.clone(),
                    selector: source.selector.clone(),
                    params: source.params.clone(),
                    subscription_generation: source.subscription_generation,
                })
                .collect(),
        }
    }

    pub fn strategy_authorizations(&self) -> Vec<IvSelectorAuthorization> {
        self.strategy_authorizations.clone()
    }
}

fn validate_profile(profile: &IvProfile) -> Vec<String> {
    let mut errors = Vec::new();
    let profile_context = format!("iv.profiles.{}", profile.profile_id);

    if profile.profile_id.trim().is_empty() {
        errors.push("iv.profiles.profile_id must be non-empty".to_string());
    }
    if profile.profile_id != profile.profile_id.trim() {
        errors.push(
            "iv.profiles.profile_id must not contain leading or trailing whitespace".to_string(),
        );
    }
    if profile.strategy_authorizations.is_empty() {
        errors.push(format!(
            "{profile_context}.strategy_authorizations must be non-empty"
        ));
    }
    let mut seen_strategy_authorizations = BTreeSet::new();
    for authorization in &profile.strategy_authorizations {
        if authorization.strategy_id.trim().is_empty() {
            errors.push(format!(
                "{profile_context}.strategy_authorizations.strategy_id must be non-empty"
            ));
        }
        if !authorization.strategy_id.trim().is_empty()
            && !seen_strategy_authorizations.insert(authorization.strategy_id.clone())
        {
            errors.push(format!(
                "{profile_context}.strategy_authorizations.{} is duplicated",
                authorization.strategy_id
            ));
        }
    }
    if profile.enabled_products.is_empty() {
        errors.push(format!(
            "{profile_context}.enabled_products must be non-empty"
        ));
    }
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_raw_events",
        profile.max_raw_events,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_indexed_points",
        profile.max_indexed_points,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_smiles",
        profile.max_smiles,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_surfaces",
        profile.max_surfaces,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_derived_points",
        profile.max_derived_points,
    );
    validate_positive_bound(
        &mut errors,
        &profile_context,
        "max_source_health_events",
        profile.max_source_health_events,
    );
    if let Some(max_source_event_age_ns) = profile.max_source_event_age_ns {
        validate_positive_u64_bound(
            &mut errors,
            &profile_context,
            "max_source_event_age_ns",
            max_source_event_age_ns,
        );
    }
    validate_iv_numeric_bounds(
        &mut errors,
        &format!("{profile_context}.input_bounds"),
        &profile.input_bounds,
        true,
    );
    if profile.sources.is_empty() {
        errors.push(format!(
            "{profile_context}.sources must contain at least one source"
        ));
    }

    let mut seen_sources = BTreeSet::new();
    let mut seen_selector_fingerprints = BTreeSet::new();
    for source in &profile.sources {
        let source_context = format!("{profile_context}.sources.{}", source.source_id);
        if !seen_sources.insert(source.source_id.clone()) {
            errors.push(format!("{source_context}.source_id is duplicated"));
        }
        if !source.selector_fingerprint.trim().is_empty()
            && !seen_selector_fingerprints.insert(source.selector_fingerprint.clone())
        {
            errors.push(format!(
                "{source_context}.selector_fingerprint is duplicated"
            ));
        }
        errors.extend(validate_source(&source_context, source));
    }
    errors.extend(validate_effective_nt_topic_uniqueness(
        &profile_context,
        profile,
    ));
    errors.extend(validate_audit_policy(
        &profile_context,
        profile,
        &seen_sources,
    ));
    errors.extend(validate_projection_policies(&profile_context, profile));
    errors.extend(validate_policy_surface(&profile_context, profile));
    errors.extend(validate_strategy_authorizations(
        &profile_context,
        profile,
        &seen_sources,
        &seen_selector_fingerprints,
    ));

    if let Err(error) = plan_profile_start(&profile.subscription_config()) {
        errors.push(format!(
            "{profile_context}.subscription planning rejected: {error:?}"
        ));
    }

    errors
}

fn validate_effective_nt_topic_uniqueness(context: &str, profile: &IvProfile) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen_option_greeks_topics = BTreeMap::new();
    let mut seen_option_chain_topics = BTreeMap::new();
    let mut seen_aggregate_greeks_topics = BTreeMap::new();
    let mut seen_custom_iv_topics = BTreeMap::new();

    for source in &profile.sources {
        match &source.selector {
            IvSelector::SourceOptionGreeks { instrument_ids, .. } => {
                for instrument_id in instrument_ids {
                    if let Some(first_source_id) =
                        seen_option_greeks_topics.insert(instrument_id.clone(), &source.source_id)
                    {
                        errors.push(format!(
                            "{context}.sources.{} duplicates option_greeks NT topic {} already owned by {}",
                            source.source_id, instrument_id, first_source_id
                        ));
                    }
                }
            }
            IvSelector::SourceOptionChain { series_ids, .. } => {
                for series_id in series_ids {
                    if let Some(first_source_id) =
                        seen_option_chain_topics.insert(series_id.clone(), &source.source_id)
                    {
                        errors.push(format!(
                            "{context}.sources.{} duplicates option_chain NT topic {} already owned by {}",
                            source.source_id, series_id, first_source_id
                        ));
                    }
                }
            }
            IvSelector::SourceAggregateGreeks { aggregate_key, .. } => {
                if let Some(first_source_id) =
                    seen_aggregate_greeks_topics.insert(aggregate_key.clone(), &source.source_id)
                {
                    errors.push(format!(
                        "{context}.sources.{} duplicates aggregate_greeks NT topic {} already owned by {}",
                        source.source_id, aggregate_key, first_source_id
                    ));
                }
            }
            IvSelector::SourceCustomImpliedVolatility {
                custom_iv_data_type,
                ..
            } => {
                if let Some(first_source_id) =
                    seen_custom_iv_topics.insert(custom_iv_data_type.clone(), &source.source_id)
                {
                    errors.push(format!(
                        "{context}.sources.{} duplicates custom_implied_volatility NT topic {} already owned by {}",
                        source.source_id, custom_iv_data_type, first_source_id
                    ));
                }
            }
            _ => {}
        }
    }

    errors
}

fn validate_audit_policy(
    context: &str,
    profile: &IvProfile,
    source_ids: &BTreeSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let audit_context = format!("{context}.audit_policy");

    if profile.audit_policy.profile_id != profile.profile_id {
        errors.push(format!(
            "{audit_context}.profile_id must match owning profile_id"
        ));
    }
    if profile.audit_policy.enabled_raw_products.is_empty() {
        errors.push(format!(
            "{audit_context}.enabled_raw_products must be non-empty"
        ));
    }
    if profile.audit_policy.authorized_audit_handles.is_empty() {
        errors.push(format!(
            "{audit_context}.authorized_audit_handles must be non-empty"
        ));
    }
    if profile.audit_policy.access_purposes.is_empty() {
        errors.push(format!("{audit_context}.access_purposes must be non-empty"));
    }
    if profile.audit_policy.eligible_sources.is_empty() {
        errors.push(format!(
            "{audit_context}.eligible_sources must be non-empty"
        ));
    }
    for source_id in &profile.audit_policy.eligible_sources {
        if !source_ids.contains(source_id) {
            errors.push(format!(
                "{audit_context}.eligible_sources contains unknown source {source_id}"
            ));
        }
    }
    if profile.audit_policy.audit_retention.max_events.is_none()
        && profile.audit_policy.audit_retention.max_age_ns.is_none()
    {
        errors.push(format!(
            "{audit_context}.audit_retention must set max_events or max_age_ns"
        ));
    }
    if let Some(max_events) = profile.audit_policy.audit_retention.max_events {
        if max_events == 0 {
            errors.push(format!(
                "{audit_context}.audit_retention.max_events must be positive when set"
            ));
        }
        if max_events > profile.max_raw_events {
            errors.push(format!(
                "{audit_context}.audit_retention.max_events cannot exceed max_raw_events"
            ));
        }
    }
    if profile.audit_policy.audit_retention.max_age_ns == Some(0) {
        errors.push(format!(
            "{audit_context}.audit_retention.max_age_ns must be positive when set"
        ));
    }

    errors
}

fn validate_policy_surface(context: &str, profile: &IvProfile) -> Vec<String> {
    let mut errors = Vec::new();
    errors.extend(validate_unique_policy_ids(
        context,
        "interpolation_policies",
        profile
            .interpolation_policies
            .iter()
            .map(|policy| policy.policy_id.as_str()),
    ));
    errors.extend(validate_unique_policy_ids(
        context,
        "fallback_policies",
        profile
            .fallback_policies
            .iter()
            .map(|policy| policy.policy_id.as_str()),
    ));
    errors.extend(validate_unique_policy_ids(
        context,
        "quorum_policies",
        profile
            .quorum_policies
            .iter()
            .map(|policy| policy.policy_id.as_str()),
    ));
    errors.extend(validate_unique_policy_ids(
        context,
        "helper_policies",
        profile
            .helper_policies
            .iter()
            .map(|policy| policy.helper_policy_id.as_str()),
    ));
    errors.extend(validate_unique_policy_ids(
        context,
        "derived_input_policies",
        profile
            .derived_input_policies
            .iter()
            .map(|policy| policy.input_policy_id.as_str()),
    ));
    let helper_policy_ids = profile
        .helper_policies
        .iter()
        .map(|policy| policy.helper_policy_id.as_str())
        .collect::<BTreeSet<_>>();
    let derived_input_policy_ids = profile
        .derived_input_policies
        .iter()
        .map(|policy| policy.input_policy_id.as_str())
        .collect::<BTreeSet<_>>();
    let derived_input_policy_by_id = profile
        .derived_input_policies
        .iter()
        .map(|policy| (policy.input_policy_id.as_str(), policy))
        .collect::<BTreeMap<_, _>>();
    let helper_policy_by_id = profile
        .helper_policies
        .iter()
        .map(|policy| (policy.helper_policy_id.as_str(), policy))
        .collect::<BTreeMap<_, _>>();
    let source_ids = profile
        .sources
        .iter()
        .map(|source| source.source_id.as_str())
        .collect::<BTreeSet<_>>();
    let source_selector_pairs = profile
        .sources
        .iter()
        .map(|source| {
            (
                source.source_id.as_str(),
                source.selector_fingerprint.as_str(),
            )
        })
        .collect::<BTreeSet<_>>();

    for policy in &profile.interpolation_policies {
        if policy.minimum_points == 0 {
            errors.push(format!(
                "{context}.interpolation_policies.{}.minimum_points must be positive",
                policy.policy_id
            ));
        }
        if policy.strike_axis.trim().is_empty() || policy.tenor_axis.trim().is_empty() {
            errors.push(format!(
                "{context}.interpolation_policies.{} axes must be non-empty",
                policy.policy_id
            ));
        }
        validate_known_source_refs(
            context,
            "interpolation_policies",
            &policy.policy_id,
            "eligible_sources",
            &policy.eligible_sources,
            &source_ids,
            &mut errors,
        );
    }
    for policy in &profile.fallback_policies {
        if policy.candidate_order.is_empty() {
            errors.push(format!(
                "{context}.fallback_policies.{}.candidate_order must be non-empty",
                policy.policy_id
            ));
        }
        if policy.maximum_timestamp_skew_ns == 0 {
            errors.push(format!(
                "{context}.fallback_policies.{}.maximum_timestamp_skew_ns must be positive",
                policy.policy_id
            ));
        }
        validate_known_source_refs(
            context,
            "fallback_policies",
            &policy.policy_id,
            "eligible_sources",
            &policy.eligible_sources,
            &source_ids,
            &mut errors,
        );
    }
    for policy in &profile.quorum_policies {
        if policy.minimum_sources == 0 {
            errors.push(format!(
                "{context}.quorum_policies.{}.minimum_sources must be positive",
                policy.policy_id
            ));
        }
        if !policy.agreement_band.is_finite() || policy.agreement_band <= 0.0 {
            errors.push(format!(
                "{context}.quorum_policies.{}.agreement_band must be finite and positive",
                policy.policy_id
            ));
        }
        if !policy.eligible_sources.is_empty()
            && policy.minimum_sources > policy.eligible_sources.len()
        {
            errors.push(format!(
                "{context}.quorum_policies.{}.minimum_sources cannot exceed eligible_sources",
                policy.policy_id
            ));
        }
        validate_known_source_refs(
            context,
            "quorum_policies",
            &policy.policy_id,
            "eligible_sources",
            &policy.eligible_sources,
            &source_ids,
            &mut errors,
        );
    }
    for policy in &profile.helper_policies {
        if policy.parameter_signature.trim().is_empty() {
            errors.push(format!(
                "{context}.helper_policies.{}.parameter_signature must be non-empty",
                policy.helper_policy_id
            ));
        } else if policy.parameter_signature.trim() != policy.nt_helper_symbol.parameter_signature()
        {
            errors.push(format!(
                "{context}.helper_policies.{}.parameter_signature must match {} ({})",
                policy.helper_policy_id,
                policy.nt_helper_symbol.nt_symbol(),
                policy.nt_helper_symbol.parameter_signature()
            ));
        }
        if policy.allowed_outputs.is_empty() {
            errors.push(format!(
                "{context}.helper_policies.{}.allowed_outputs must be non-empty",
                policy.helper_policy_id
            ));
        } else if !policy
            .allowed_outputs
            .contains(&IvHelperOutput::IvAndGreeks)
        {
            errors.push(format!(
                "{context}.helper_policies.{}.allowed_outputs must include iv_and_greeks",
                policy.helper_policy_id
            ));
        }
        if !derived_input_policy_ids.contains(policy.input_policy_ref.as_str()) {
            errors.push(format!(
                "{context}.helper_policies.{}.input_policy_ref must reference a configured derived input policy",
                policy.helper_policy_id
            ));
        } else if let Some(input_policy) =
            derived_input_policy_by_id.get(policy.input_policy_ref.as_str())
            && input_policy.helper_policy_ref != policy.helper_policy_id
        {
            errors.push(format!(
                "{context}.helper_policies.{}.input_policy_ref references {}, but its helper_policy_ref is {}",
                policy.helper_policy_id, policy.input_policy_ref, input_policy.helper_policy_ref
            ));
        }
        if policy.max_input_timestamp_skew_ns == 0 {
            errors.push(format!(
                "{context}.helper_policies.{}.max_input_timestamp_skew_ns must be positive",
                policy.helper_policy_id
            ));
        }
        if policy.max_operator_input_age_ns == 0 {
            errors.push(format!(
                "{context}.helper_policies.{}.max_operator_input_age_ns must be positive",
                policy.helper_policy_id
            ));
        }
        if policy.minimum_valid_iv_output <= policy.nt_helper_symbol.minimum_valid_output_floor()
            || !policy.minimum_valid_iv_output.is_finite()
        {
            errors.push(format!(
                "{context}.helper_policies.{}.minimum_valid_iv_output must be finite and above the selected NT helper failure floor",
                policy.helper_policy_id
            ));
        }
        validate_iv_numeric_bounds(
            &mut errors,
            &format!(
                "{context}.helper_policies.{}.output_bounds",
                policy.helper_policy_id
            ),
            &policy.output_bounds,
            true,
        );
        if policy.convention_policy.allowed_conventions.is_empty() {
            errors.push(format!(
                "{context}.helper_policies.{}.convention_policy.allowed_conventions must be non-empty",
                policy.helper_policy_id
            ));
        }
    }
    for policy in &profile.derived_input_policies {
        if !helper_policy_ids.contains(policy.helper_policy_ref.as_str()) {
            errors.push(format!(
                "{context}.derived_input_policies.{}.helper_policy_ref must reference a configured helper policy",
                policy.input_policy_id
            ));
        } else if let Some(helper_policy) =
            helper_policy_by_id.get(policy.helper_policy_ref.as_str())
            && helper_policy.input_policy_ref != policy.input_policy_id
        {
            errors.push(format!(
                "{context}.derived_input_policies.{}.helper_policy_ref references {}, but its input_policy_ref is {}",
                policy.input_policy_id, policy.helper_policy_ref, helper_policy.input_policy_ref
            ));
        }
        if policy.required_fields.is_empty() {
            errors.push(format!(
                "{context}.derived_input_policies.{}.required_fields must be non-empty",
                policy.input_policy_id
            ));
        }
        if let Some(helper_policy) = helper_policy_by_id.get(policy.helper_policy_ref.as_str()) {
            for required_field in helper_policy.nt_helper_symbol.required_fields() {
                if !policy.required_fields.contains(required_field) {
                    errors.push(format!(
                        "{context}.derived_input_policies.{}.required_fields must include {}",
                        policy.input_policy_id,
                        required_field.as_str()
                    ));
                }
            }
        }
        if policy.field_sources.is_empty() {
            errors.push(format!(
                "{context}.derived_input_policies.{}.field_sources must be non-empty",
                policy.input_policy_id
            ));
        }
        let mut seen_field_sources = BTreeSet::new();
        for field_policy in &policy.field_sources {
            if !seen_field_sources.insert(field_policy.field) {
                errors.push(format!(
                    "{context}.derived_input_policies.{}.field_sources contains duplicate {}",
                    policy.input_policy_id,
                    field_policy.field.as_str()
                ));
            }
            if field_policy.allowed_source_kinds.is_empty() {
                errors.push(format!(
                    "{context}.derived_input_policies.{}.field_sources.{}.allowed_source_kinds must be non-empty",
                    policy.input_policy_id,
                    field_policy.field.as_str()
                ));
            }
            validate_derived_input_field_source_consistency(
                context,
                &policy.input_policy_id,
                field_policy,
                &mut errors,
            );
            if let Some(source_ref) = &field_policy.profile_source_ref
                && !source_selector_pairs.contains(&(
                    source_ref.source_id.as_str(),
                    source_ref.selector_fingerprint.as_str(),
                ))
            {
                errors.push(format!(
                    "{context}.derived_input_policies.{}.field_sources.{}.profile_source_ref must reference a configured source and selector pair: {} / {}",
                    policy.input_policy_id,
                    field_policy.field.as_str(),
                    source_ref.source_id,
                    source_ref.selector_fingerprint
                ));
            }
        }
        for required_field in &policy.required_fields {
            if !policy
                .field_sources
                .iter()
                .any(|field_policy| field_policy.field == *required_field)
            {
                errors.push(format!(
                    "{context}.derived_input_policies.{}.field_sources must include {}",
                    policy.input_policy_id,
                    required_field.as_str()
                ));
            }
        }
        if policy.max_input_skew_ns == 0 {
            errors.push(format!(
                "{context}.derived_input_policies.{}.max_input_skew_ns must be positive",
                policy.input_policy_id
            ));
        }
        if policy.freshness_ns == 0 {
            errors.push(format!(
                "{context}.derived_input_policies.{}.freshness_ns must be positive",
                policy.input_policy_id
            ));
        }
        if policy.convention_policy.allowed_conventions.is_empty() {
            errors.push(format!(
                "{context}.derived_input_policies.{}.convention_policy.allowed_conventions must be non-empty",
                policy.input_policy_id
            ));
        }
        validate_derived_input_policy_bounds(context, policy, &mut errors);
    }
    for input in &profile.derived_inputs {
        if input.profile_id != profile.profile_id {
            errors.push(format!(
                "{context}.derived_inputs must reference profile_id {}",
                profile.profile_id
            ));
        }
    }

    errors
}

fn validate_derived_input_field_source_consistency(
    context: &str,
    input_policy_id: &str,
    field_policy: &super::derive::IvDerivedInputFieldPolicy,
    errors: &mut Vec<String>,
) {
    use super::derive::{IvDerivedInputField, IvDerivedInputSourceKind};

    let field_context = format!(
        "{context}.derived_input_policies.{}.field_sources.{}",
        input_policy_id,
        field_policy.field.as_str()
    );

    if field_policy.profile_source_ref.is_some()
        && !field_policy
            .allowed_source_kinds
            .contains(&IvDerivedInputSourceKind::ProfileSourceRef)
    {
        errors.push(format!(
            "{field_context}.profile_source_ref requires allowed_source_kinds to include profile_source_ref"
        ));
    }

    if let Some(operator_number) = field_policy.operator_number {
        if field_policy.field == IvDerivedInputField::OptionSide {
            errors.push(format!(
                "{field_context}.operator_number is only valid for numeric derived input fields"
            ));
        }
        if operator_number.source_kind != IvDerivedInputSourceKind::OperatorConfigured {
            errors.push(format!(
                "{field_context}.operator_number source_kind must be operator_configured"
            ));
        }
        if !field_policy
            .allowed_source_kinds
            .contains(&IvDerivedInputSourceKind::OperatorConfigured)
        {
            errors.push(format!(
                "{field_context}.operator_number requires allowed_source_kinds to include operator_configured"
            ));
        }
    }

    if let Some(operator_side) = field_policy.operator_side {
        if field_policy.field != IvDerivedInputField::OptionSide {
            errors.push(format!(
                "{field_context}.operator_side is only valid for option_side"
            ));
        }
        if operator_side.source_kind != IvDerivedInputSourceKind::OperatorConfigured {
            errors.push(format!(
                "{field_context}.operator_side source_kind must be operator_configured"
            ));
        }
        if !field_policy
            .allowed_source_kinds
            .contains(&IvDerivedInputSourceKind::OperatorConfigured)
        {
            errors.push(format!(
                "{field_context}.operator_side requires allowed_source_kinds to include operator_configured"
            ));
        }
    }
}

fn validate_known_source_refs(
    context: &str,
    policy_family: &str,
    policy_id: &str,
    field: &str,
    configured_refs: &[String],
    source_ids: &BTreeSet<&str>,
    errors: &mut Vec<String>,
) {
    for source_id in configured_refs {
        if !source_ids.contains(source_id.as_str()) {
            errors.push(format!(
                "{context}.{policy_family}.{policy_id}.{field} contains unknown source {source_id}"
            ));
        }
    }
}

fn validate_derived_input_policy_bounds(
    context: &str,
    policy: &super::derive::IvDerivedInputPolicy,
    errors: &mut Vec<String>,
) {
    for field in policy.required_fields.iter().copied() {
        if field == super::derive::IvDerivedInputField::OptionSide {
            continue;
        }
        if policy.bounds.numeric_bound(field).is_none() {
            errors.push(format!(
                "{context}.derived_input_policies.{} bounds must define {}",
                policy.input_policy_id,
                field.as_str()
            ));
        }
    }
    for (field, bounds) in policy.bounds.numeric_bounds() {
        validate_numeric_bound_shape(
            errors,
            &format!(
                "{context}.derived_input_policies.{}.bounds.{}",
                policy.input_policy_id,
                field.as_str()
            ),
            bounds,
        );
    }
}

fn validate_unique_policy_ids<'a>(
    context: &str,
    policy_family: &str,
    policy_ids: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for policy_id in policy_ids {
        if policy_id.trim().is_empty() {
            errors.push(format!(
                "{context}.{policy_family}.policy_id must be non-empty"
            ));
        } else if !seen.insert(policy_id.to_string()) {
            errors.push(format!(
                "{context}.{policy_family} contains duplicate policy_id {policy_id}"
            ));
        }
    }
    errors
}

fn validate_projection_policies(context: &str, profile: &IvProfile) -> Vec<String> {
    let mut errors = Vec::new();
    let policy_context = format!("{context}.projection_policies");
    let interpolation_policy_ids = profile
        .interpolation_policies
        .iter()
        .map(|policy| policy.policy_id.as_str())
        .collect::<BTreeSet<_>>();
    let fallback_policy_ids = profile
        .fallback_policies
        .iter()
        .map(|policy| policy.policy_id.as_str())
        .collect::<BTreeSet<_>>();
    let quorum_policy_ids = profile
        .quorum_policies
        .iter()
        .map(|policy| policy.policy_id.as_str())
        .collect::<BTreeSet<_>>();
    let source_ids = profile
        .sources
        .iter()
        .map(|source| source.source_id.as_str())
        .collect::<BTreeSet<_>>();

    if profile
        .enabled_products
        .contains(&IvProductKind::ProjectedScalarIv)
        && profile.projection_policies.is_empty()
    {
        errors.push(format!(
            "{policy_context} must be non-empty when projected_scalar_iv is enabled"
        ));
    }

    let mut seen_policy_ids = BTreeSet::new();
    for policy in &profile.projection_policies {
        if policy.policy_id.trim().is_empty() {
            errors.push(format!("{policy_context}.policy_id must be non-empty"));
        }
        if !seen_policy_ids.insert(policy.policy_id.clone()) {
            errors.push(format!(
                "{policy_context}.{} is duplicated",
                policy.policy_id
            ));
        }
        if policy.minimum_points == 0 {
            errors.push(format!(
                "{policy_context}.{}.minimum_points must be positive",
                policy.policy_id
            ));
        }
        if policy.max_projection_input_skew_ns == 0 {
            errors.push(format!(
                "{policy_context}.{}.max_projection_input_skew_ns must be positive",
                policy.policy_id
            ));
        }
        validate_iv_numeric_bounds(
            &mut errors,
            &format!("{policy_context}.{}.output_bounds", policy.policy_id),
            &policy.output_bounds,
            true,
        );
        validate_projection_output_bounds_subset(
            &mut errors,
            &format!("{policy_context}.{}.output_bounds", policy.policy_id),
            &profile.input_bounds,
            &policy.output_bounds,
        );
        if policy
            .source_eligibility
            .iter()
            .any(|source_id| !source_ids.contains(source_id.as_str()))
        {
            errors.push(format!(
                "{policy_context}.{}.source_eligibility contains unknown source",
                policy.policy_id
            ));
        }
        if policy
            .fallback_policy_ref
            .as_ref()
            .is_some_and(|policy_ref| !fallback_policy_ids.contains(policy_ref.as_str()))
        {
            errors.push(format!(
                "{policy_context}.{}.fallback_policy_ref must reference a configured fallback policy",
                policy.policy_id
            ));
        }
        if policy
            .interpolation_policy_ref
            .as_ref()
            .is_some_and(|policy_ref| !interpolation_policy_ids.contains(policy_ref.as_str()))
        {
            errors.push(format!(
                "{policy_context}.{}.interpolation_policy_ref must reference a configured interpolation policy",
                policy.policy_id
            ));
        }
        if policy
            .quorum_policy_ref
            .as_ref()
            .is_some_and(|policy_ref| !quorum_policy_ids.contains(policy_ref.as_str()))
        {
            errors.push(format!(
                "{policy_context}.{}.quorum_policy_ref must reference a configured quorum policy",
                policy.policy_id
            ));
        }
    }

    errors
}

fn validate_positive_bound(errors: &mut Vec<String>, context: &str, field: &str, value: usize) {
    if value == 0 {
        errors.push(format!("{context}.{field} must be positive"));
    }
}

fn validate_positive_u64_bound(errors: &mut Vec<String>, context: &str, field: &str, value: u64) {
    if value == 0 {
        errors.push(format!("{context}.{field} must be positive"));
    }
}

fn validate_iv_numeric_bounds(
    errors: &mut Vec<String>,
    context: &str,
    bounds: &IvNumericBounds,
    require_conventions: bool,
) {
    validate_numeric_bound_shape(errors, context, bounds);
    if !bounds.finite_required || !bounds.positive_required {
        errors.push(format!(
            "{context} must require finite and positive IV values"
        ));
    }
    if bounds.unit != IvBoundUnit::Unitless {
        errors.push(format!("{context}.unit must be unitless for IV values"));
    }
    if bounds.inclusive_max.is_none() && bounds.exclusive_max.is_none() {
        errors.push(format!(
            "{context} must define inclusive_max or exclusive_max for IV values"
        ));
    }
    if require_conventions && bounds.allowed_conventions.allowed_conventions.is_empty() {
        errors.push(format!("{context}.allowed_conventions must be non-empty"));
    }
}

fn validate_projection_output_bounds_subset(
    errors: &mut Vec<String>,
    context: &str,
    input_bounds: &IvNumericBounds,
    output_bounds: &IvNumericBounds,
) {
    validate_lower_bound_subset(errors, context, input_bounds, output_bounds);
    validate_upper_bound_subset(errors, context, input_bounds, output_bounds);
    if !input_bounds
        .allowed_conventions
        .allowed_conventions
        .is_superset(&output_bounds.allowed_conventions.allowed_conventions)
    {
        errors.push(format!(
            "{context}.allowed_conventions must be a subset of profile input_bounds.allowed_conventions"
        ));
    }
}

fn validate_lower_bound_subset(
    errors: &mut Vec<String>,
    context: &str,
    input_bounds: &IvNumericBounds,
    output_bounds: &IvNumericBounds,
) {
    let Some(input_lower) = lower_bound_edge(input_bounds) else {
        return;
    };
    let Some(output_lower) = lower_bound_edge(output_bounds) else {
        errors.push(format!(
            "{context} lower bound must be at least profile input_bounds lower bound"
        ));
        return;
    };
    if output_lower.value < input_lower.value
        || (output_lower.value == input_lower.value
            && output_lower.inclusive
            && !input_lower.inclusive)
    {
        errors.push(format!(
            "{context} lower bound must be at least profile input_bounds lower bound"
        ));
    }
}

fn validate_upper_bound_subset(
    errors: &mut Vec<String>,
    context: &str,
    input_bounds: &IvNumericBounds,
    output_bounds: &IvNumericBounds,
) {
    let Some(input_upper) = upper_bound_edge(input_bounds) else {
        return;
    };
    let Some(output_upper) = upper_bound_edge(output_bounds) else {
        errors.push(format!(
            "{context} upper bound must not exceed profile input_bounds upper bound"
        ));
        return;
    };
    if output_upper.value > input_upper.value
        || (output_upper.value == input_upper.value
            && output_upper.inclusive
            && !input_upper.inclusive)
    {
        errors.push(format!(
            "{context} upper bound must not exceed profile input_bounds upper bound"
        ));
    }
}

#[derive(Clone, Copy)]
struct BoundEdge {
    value: f64,
    inclusive: bool,
}

fn lower_bound_edge(bounds: &IvNumericBounds) -> Option<BoundEdge> {
    match (bounds.inclusive_min, bounds.exclusive_min) {
        (Some(inclusive), Some(exclusive)) => {
            if exclusive >= inclusive {
                Some(BoundEdge {
                    value: exclusive,
                    inclusive: false,
                })
            } else {
                Some(BoundEdge {
                    value: inclusive,
                    inclusive: true,
                })
            }
        }
        (Some(value), None) => Some(BoundEdge {
            value,
            inclusive: true,
        }),
        (None, Some(value)) => Some(BoundEdge {
            value,
            inclusive: false,
        }),
        (None, None) => None,
    }
}

fn upper_bound_edge(bounds: &IvNumericBounds) -> Option<BoundEdge> {
    match (bounds.inclusive_max, bounds.exclusive_max) {
        (Some(inclusive), Some(exclusive)) => {
            if exclusive <= inclusive {
                Some(BoundEdge {
                    value: exclusive,
                    inclusive: false,
                })
            } else {
                Some(BoundEdge {
                    value: inclusive,
                    inclusive: true,
                })
            }
        }
        (Some(value), None) => Some(BoundEdge {
            value,
            inclusive: true,
        }),
        (None, Some(value)) => Some(BoundEdge {
            value,
            inclusive: false,
        }),
        (None, None) => None,
    }
}

fn validate_numeric_bound_shape(errors: &mut Vec<String>, context: &str, bounds: &IvNumericBounds) {
    for (field, value) in [
        ("inclusive_min", bounds.inclusive_min),
        ("inclusive_max", bounds.inclusive_max),
        ("exclusive_min", bounds.exclusive_min),
        ("exclusive_max", bounds.exclusive_max),
    ] {
        if value.is_some_and(|value| !value.is_finite()) {
            errors.push(format!("{context}.{field} must be finite when set"));
        }
    }
    validate_bound_pair(
        errors,
        context,
        "inclusive_min",
        bounds.inclusive_min,
        "inclusive_max",
        bounds.inclusive_max,
        true,
    );
    validate_bound_pair(
        errors,
        context,
        "inclusive_min",
        bounds.inclusive_min,
        "exclusive_max",
        bounds.exclusive_max,
        false,
    );
    validate_bound_pair(
        errors,
        context,
        "exclusive_min",
        bounds.exclusive_min,
        "inclusive_max",
        bounds.inclusive_max,
        false,
    );
    validate_bound_pair(
        errors,
        context,
        "exclusive_min",
        bounds.exclusive_min,
        "exclusive_max",
        bounds.exclusive_max,
        false,
    );
}

fn validate_bound_pair(
    errors: &mut Vec<String>,
    context: &str,
    lower_field: &str,
    lower: Option<f64>,
    upper_field: &str,
    upper: Option<f64>,
    allow_equal: bool,
) {
    let (Some(lower), Some(upper)) = (lower, upper) else {
        return;
    };
    if (allow_equal && lower > upper) || (!allow_equal && lower >= upper) {
        let relation = if allow_equal { "<=" } else { "<" };
        errors.push(format!(
            "{context}.{lower_field} must be {relation} {upper_field}"
        ));
    }
}

fn validate_source(context: &str, source: &IvSourceConfig) -> Vec<String> {
    let mut errors = Vec::new();

    if source.source_id.trim().is_empty() {
        errors.push(format!("{context}.source_id must be non-empty"));
    }
    if source.selector_fingerprint.trim().is_empty() {
        errors.push(format!("{context}.selector_fingerprint must be non-empty"));
    }
    if source.client_id.trim().is_empty() {
        errors.push(format!("{context}.client_id must be non-empty"));
    }
    if source.accepted_conventions.is_empty() {
        errors.push(format!("{context}.accepted_conventions must be non-empty"));
    }
    if source.nt_provenance.nt_revision.trim().is_empty() {
        errors.push(format!(
            "{context}.nt_provenance.nt_revision must be non-empty"
        ));
    }
    if source.nt_provenance.nt_evidence_path.trim().is_empty() {
        errors.push(format!(
            "{context}.nt_provenance.nt_evidence_path must be non-empty"
        ));
    }
    if source.nt_provenance.nt_symbol.trim().is_empty() {
        errors.push(format!(
            "{context}.nt_provenance.nt_symbol must be non-empty"
        ));
    }
    if !selector_matches_source_kind(source.source_kind, &source.selector) {
        errors.push(format!(
            "{context}.selector variant must match source_kind {:?}",
            source.source_kind
        ));
    }
    errors.extend(validate_selector_not_empty(context, &source.selector));

    errors
}

fn validate_strategy_authorizations(
    context: &str,
    profile: &IvProfile,
    source_ids: &BTreeSet<String>,
    selector_fingerprints: &BTreeSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for auth in &profile.strategy_authorizations {
        let auth_context = if auth.strategy_id.trim().is_empty() {
            format!("{context}.strategy_authorizations")
        } else {
            format!("{context}.strategy_authorizations.{}", auth.strategy_id)
        };

        if auth.allowed_product_kinds.is_empty() {
            errors.push(format!(
                "{auth_context}.allowed_product_kinds must be non-empty"
            ));
        }
        for product_kind in &auth.allowed_product_kinds {
            if !profile.enabled_products.contains(product_kind) {
                errors.push(format!(
                    "{auth_context}.allowed_product_kinds contains disabled product kind {product_kind:?}"
                ));
            }
        }
        for source_id in &auth.allowed_source_ids {
            if !source_ids.contains(source_id) {
                errors.push(format!(
                    "{auth_context}.allowed_source_ids contains unknown source {source_id}"
                ));
            }
        }
        let source_scoped_source_health_only = auth.allowed_product_kinds.len() == 1
            && auth
                .allowed_product_kinds
                .contains(&IvProductKind::SourceHealth)
            && !auth.allowed_source_ids.is_empty();
        if auth.authorization_mode == IvAuthorizationMode::SelectorScoped
            && auth.allowed_selector_fingerprints.is_empty()
            && !source_scoped_source_health_only
        {
            errors.push(format!(
                "{auth_context}.allowed_selector_fingerprints must be non-empty for selector_scoped authorization"
            ));
        }
        for selector_fingerprint in &auth.allowed_selector_fingerprints {
            if !selector_fingerprints.contains(selector_fingerprint) {
                errors.push(format!(
                    "{auth_context}.allowed_selector_fingerprints contains unknown selector {selector_fingerprint}"
                ));
            }
        }
    }

    errors
}

fn selector_matches_source_kind(source_kind: IvSourceKind, selector: &IvSelector) -> bool {
    matches!(
        (source_kind, selector),
        (
            IvSourceKind::OptionGreeks,
            IvSelector::SourceOptionGreeks { .. }
        ) | (
            IvSourceKind::OptionChain,
            IvSelector::SourceOptionChain { .. }
        ) | (
            IvSourceKind::AggregateGreeks,
            IvSelector::SourceAggregateGreeks { .. }
        ) | (
            IvSourceKind::CustomImpliedVolatility,
            IvSelector::SourceCustomImpliedVolatility { .. }
        )
    )
}

fn validate_selector_not_empty(context: &str, selector: &IvSelector) -> Vec<String> {
    match selector {
        IvSelector::SourceOptionGreeks { instrument_ids, .. } if instrument_ids.is_empty() => {
            vec![format!(
                "{context}.selector.instrument_ids must be non-empty"
            )]
        }
        IvSelector::SourceOptionChain {
            series_ids,
            strike_range_policy,
            ..
        } if series_ids.is_empty() || strike_range_policy.trim().is_empty() => {
            vec![format!(
                "{context}.selector.series_ids and strike_range_policy must be non-empty"
            )]
        }
        IvSelector::SourceAggregateGreeks {
            aggregate_key,
            underlying_selectors,
            delta_field,
            gamma_field,
            vega_field,
            theta_field,
            rho_field,
            iv_field,
            iv_basis,
            iv_convention,
            ..
        } if aggregate_key.trim().is_empty()
            || underlying_selectors.is_empty()
            || delta_field.trim().is_empty()
            || gamma_field.trim().is_empty()
            || vega_field.trim().is_empty()
            || theta_field.trim().is_empty()
            || rho_field.trim().is_empty()
            || iv_field
                .as_ref()
                .is_some_and(|field| field.trim().is_empty())
            || (iv_field.is_some() || iv_basis.is_some() || iv_convention.is_some())
                && !(iv_field.is_some() && iv_basis.is_some() && iv_convention.is_some()) =>
        {
            vec![format!(
                "{context}.selector.aggregate_key, underlying_selectors, greek field names, and optional aggregate IV mapping must be complete and non-empty"
            )]
        }
        IvSelector::SourceCustomImpliedVolatility {
            custom_iv_data_type,
            custom_iv_data_fields,
            ..
        } if custom_iv_data_type.trim().is_empty() || custom_iv_data_fields.is_empty() => {
            vec![format!(
                "{context}.selector.custom_iv_data_type and custom_iv_data_fields must be non-empty"
            )]
        }
        _ => Vec::new(),
    }
}
