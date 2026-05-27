//! Rotating-market target parsing for bolt-v3 strategy config.
//!
//! Each supported `target.rotating_market_family` has a module here
//! that owns its typed `[target]` fields, cadence checks, slug
//! construction, and instrument-filter errors.

pub mod updown;

use std::{any::Any, collections::BTreeMap, fmt, sync::Arc};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, LoadedStrategy},
    bolt_v3_instrument_filters::{
        InstrumentFilterConfig, InstrumentFilterError, InstrumentFilterTarget,
    },
};
use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};

/// Target metadata read by startup validation before dispatching to a
/// `target.rotating_market_family` validator.
#[derive(Debug, Clone, Deserialize)]
pub struct TargetMetadata {
    pub configured_target_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct TargetFamilyDispatch {
    rotating_market_family: String,
}

#[derive(Clone)]
pub struct MarketFamilyValidationBinding {
    pub key: &'static str,
    pub validate_target: fn(&str, &toml::Value) -> Vec<String>,
    /// Per-strategy projector. The parent dispatcher
    /// (`instrument_filters_from_config_with_bindings`) reads each
    /// strategy's `target.rotating_market_family` first and routes only
    /// the matching strategies into this function; family bindings
    /// never see strategies from a different family, so a future
    /// non-updown strategy cannot fail inside the updown binding's
    /// typed deserialization.
    pub instrument_filter_target_for_strategy:
        fn(&LoadedStrategy) -> Result<Option<InstrumentFilterTarget>, InstrumentFilterError>,
    pub target_runtime_fields:
        fn(&toml::Value) -> Result<TargetRuntimeFields, InstrumentFilterError>,
    pub select_binary_option_market:
        fn(MarketSelectionTarget<'_>, &[InstrumentAny], u64) -> Option<SelectedBinaryOptionMarket>,
    pub market_selection_candidate_windows:
        fn(
            MarketSelectionTarget<'_>,
            u64,
        ) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError>,
    pub selected_market_requirement: fn(
        &toml::Value,
        &SelectedBinaryOptionMarket,
        u64,
    )
        -> Result<SelectedMarketRequirement, InstrumentFilterError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketSelectionTarget<'a> {
    pub family_key: &'a str,
    pub underlying_asset: &'a str,
    pub cadence_seconds: i64,
    pub cadence_slug_token: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedBinaryOptionMarket {
    pub market_id: String,
    pub instrument_id: InstrumentId,
    pub up_instrument_id: InstrumentId,
    pub down_instrument_id: InstrumentId,
    pub selection_outcome: MarketSelectionOutcome,
    pub start_timestamp_milliseconds: u64,
    pub expiration_timestamp_milliseconds: u64,
    pub seconds_to_end: u64,
    pub source_identity: SelectedMarketSourceIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMarketSourceIdentity {
    pub condition_id: String,
    pub market_slug: String,
    pub question_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedMarketRequirement {
    pub configured_target_id: String,
    pub venue: String,
    pub family_key: String,
    pub market_id: String,
    pub instrument_ids: Vec<String>,
    pub market_class: String,
    pub resolution_kind: String,
    pub resolution_identity: String,
    pub value_kind: String,
    pub metadata_provenance_sha256: String,
    pub selected_market_key: String,
    pub selected_at_ms: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectedMarketRequirementParts<'a> {
    pub configured_target_id: &'a str,
    pub venue: &'a str,
    pub family_key: &'a str,
    pub market_id: &'a str,
    pub instrument_ids: Vec<String>,
    pub market_class: &'a str,
    pub resolution_kind: &'a str,
    pub resolution_identity: &'a str,
    pub value_kind: &'a str,
    pub metadata_provenance_fields: BTreeMap<String, serde_json::Value>,
    pub selected_at_ms: u64,
}

const SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD: &str = "configured_target_id";
const SELECTED_MARKET_FAMILY_KEY_FIELD: &str = "family_key";
const SELECTED_MARKET_INSTRUMENT_IDS_FIELD: &str = "instrument_ids";
const SELECTED_MARKET_MARKET_CLASS_FIELD: &str = "market_class";
const SELECTED_MARKET_MARKET_ID_FIELD: &str = "market_id";
const SELECTED_MARKET_METADATA_PROVENANCE_FIELD: &str = "metadata_provenance";
const SELECTED_MARKET_METADATA_PROVENANCE_KEY_FIELD: &str = "metadata_provenance key";
const SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD: &str = "metadata_provenance_sha256";
const SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD: &str = "resolution_identity";
const SELECTED_MARKET_RESOLUTION_KIND_FIELD: &str = "resolution_kind";
const SELECTED_MARKET_VALUE_KIND_FIELD: &str = "value_kind";
const SELECTED_MARKET_VENUE_FIELD: &str = "venue";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketSelectionOutcome {
    Current,
    Next,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketSelectionCandidateWindow {
    pub outcome: MarketSelectionOutcome,
    pub market_slug: String,
    pub start_timestamp_milliseconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRuntimeFields {
    pub configured_target_id: String,
    pub target_kind: String,
    pub rotating_market_family: String,
    pub underlying_asset: String,
    pub cadence_seconds: i64,
    pub cadence_seconds_source_field: &'static str,
    pub cadence_slug_token: String,
    pub market_selection_rule: String,
    pub retry_interval_seconds: u64,
    pub blocked_after_seconds: u64,
}

pub trait MarketIdentityTarget: fmt::Debug + Send + Sync + Any {
    fn family_key(&self) -> &'static str;
    fn configured_target_id(&self) -> &str;
    fn execution_client_id(&self) -> &str;
    fn as_any(&self) -> &dyn Any;
}

#[derive(Debug, Clone)]
pub struct MarketIdentityPlan {
    targets: Vec<Arc<dyn MarketIdentityTarget>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketIdentityPlanError {
    message: String,
}

impl MarketIdentityPlanError {
    fn new(message: String) -> Self {
        Self { message }
    }
}

impl std::fmt::Display for MarketIdentityPlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MarketIdentityPlanError {}

pub struct MarketIdentityExecutionClientTargetRef<'a> {
    pub family_key: &'static str,
    pub configured_target_id: &'a str,
    pub execution_client_id: &'a str,
}

impl MarketIdentityPlan {
    pub fn empty() -> Self {
        Self {
            targets: Vec::new(),
        }
    }

    pub fn push_target<T>(&mut self, target: T)
    where
        T: MarketIdentityTarget + 'static,
    {
        self.targets.push(Arc::new(target));
    }

    pub fn targets(&self) -> impl Iterator<Item = &dyn MarketIdentityTarget> {
        self.targets.iter().map(Arc::as_ref)
    }

    pub fn execution_client_target_refs(
        &self,
    ) -> impl Iterator<Item = MarketIdentityExecutionClientTargetRef<'_>> {
        self.targets()
            .map(|target| MarketIdentityExecutionClientTargetRef {
                family_key: target.family_key(),
                configured_target_id: target.configured_target_id(),
                execution_client_id: target.execution_client_id(),
            })
    }
}

const VALIDATION_BINDINGS: &[MarketFamilyValidationBinding] = &[MarketFamilyValidationBinding {
    key: updown::KEY,
    validate_target: updown::validate_target_block,
    instrument_filter_target_for_strategy: updown::instrument_filter_target_for_strategy,
    target_runtime_fields: updown::target_runtime_fields,
    select_binary_option_market: updown::select_binary_option_market,
    market_selection_candidate_windows: updown::market_selection_candidate_windows,
    selected_market_requirement: updown::selected_market_requirement,
}];

pub fn validation_bindings() -> &'static [MarketFamilyValidationBinding] {
    VALIDATION_BINDINGS
}

pub fn market_identity_plan_from_config(
    loaded: &LoadedBoltV3Config,
) -> Result<MarketIdentityPlan, MarketIdentityPlanError> {
    updown::plan_market_identity(loaded)
        .map_err(|error| MarketIdentityPlanError::new(error.to_string()))
}

pub fn instrument_filters_from_config(
    loaded: &LoadedBoltV3Config,
) -> Result<InstrumentFilterConfig, InstrumentFilterError> {
    instrument_filters_from_config_with_bindings(loaded, validation_bindings())
}

pub fn instrument_filters_from_config_with_bindings(
    loaded: &LoadedBoltV3Config,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<InstrumentFilterConfig, InstrumentFilterError> {
    let mut targets = Vec::new();
    for strategy in &loaded.strategies {
        let dispatch: TargetFamilyDispatch =
            strategy.config.target.clone().try_into().map_err(|error| {
                InstrumentFilterError::TargetParseFailed {
                    strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                    message: format!("target: {error}"),
                }
            })?;
        let binding = bindings
            .iter()
            .find(|binding| binding.key == dispatch.rotating_market_family)
            .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
                context: Some(format!(
                    "strategy `{}`",
                    strategy.config.strategy_instance_id
                )),
                family_key: dispatch.rotating_market_family.clone(),
                supported: bindings.iter().map(|b| b.key).collect(),
            })?;
        if let Some(target) = (binding.instrument_filter_target_for_strategy)(strategy)? {
            targets.push(target);
        }
    }
    Ok(InstrumentFilterConfig::new(targets))
}

pub fn target_runtime_fields_from_target(
    target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    target_runtime_fields_from_target_with_bindings(target, validation_bindings())
}

pub fn target_runtime_fields_from_target_with_bindings(
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    let dispatch: TargetFamilyDispatch =
        target
            .clone()
            .try_into()
            .map_err(|error| InstrumentFilterError::Other {
                message: format!("target: {error}"),
            })?;
    bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.target_runtime_fields)(target))
}

pub fn select_binary_option_market_from_target(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    select_binary_option_market_from_target_with_bindings(
        target,
        instruments,
        now_milliseconds,
        validation_bindings(),
    )
}

pub fn select_binary_option_market_from_target_with_bindings(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Option<SelectedBinaryOptionMarket> {
    bindings
        .iter()
        .find(|binding| binding.key == target.family_key)
        .and_then(|binding| {
            (binding.select_binary_option_market)(target, instruments, now_milliseconds)
        })
}

pub fn market_selection_candidate_windows_from_target(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    market_selection_candidate_windows_from_target_with_bindings(
        target,
        now_milliseconds,
        validation_bindings(),
    )
}

pub fn market_selection_candidate_windows_from_target_with_bindings(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    bindings
        .iter()
        .find(|binding| binding.key == target.family_key)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: target.family_key.to_string(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.market_selection_candidate_windows)(target, now_milliseconds))
}

pub fn selected_market_requirement_from_target(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    selected_market_requirement_from_target_with_bindings(
        target,
        selected,
        selected_at_ms,
        validation_bindings(),
    )
}

pub fn selected_market_requirement_from_target_with_bindings(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
    bindings: &[MarketFamilyValidationBinding],
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    let dispatch: TargetFamilyDispatch =
        target
            .clone()
            .try_into()
            .map_err(|error| InstrumentFilterError::Other {
                message: format!("target: {error}"),
            })?;
    bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
        .ok_or_else(|| InstrumentFilterError::UnsupportedFamily {
            context: None,
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        })
        .and_then(|binding| (binding.selected_market_requirement)(target, selected, selected_at_ms))
}

pub(crate) fn selected_market_metadata_provenance_fields<I, K, V>(
    fields: I,
) -> BTreeMap<String, serde_json::Value>
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    fields
        .into_iter()
        .map(|(key, value)| (key.into(), serde_json::Value::String(value.into())))
        .collect()
}

pub(crate) fn selected_market_requirement_from_parts(
    parts: SelectedMarketRequirementParts<'_>,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    ensure_selected_market_text(
        SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD,
        parts.configured_target_id,
        false,
    )?;
    ensure_selected_market_text(SELECTED_MARKET_VENUE_FIELD, parts.venue, false)?;
    ensure_selected_market_text(SELECTED_MARKET_FAMILY_KEY_FIELD, parts.family_key, false)?;
    ensure_selected_market_text(SELECTED_MARKET_MARKET_ID_FIELD, parts.market_id, false)?;
    ensure_selected_market_text(
        SELECTED_MARKET_MARKET_CLASS_FIELD,
        parts.market_class,
        false,
    )?;
    ensure_selected_market_text(
        SELECTED_MARKET_RESOLUTION_KIND_FIELD,
        parts.resolution_kind,
        false,
    )?;
    ensure_selected_market_text(
        SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD,
        parts.resolution_identity,
        false,
    )?;
    ensure_selected_market_text(SELECTED_MARKET_VALUE_KIND_FIELD, parts.value_kind, false)?;
    if parts.instrument_ids.is_empty() {
        return Err(selected_market_requirement_error(
            "selected-market instrument_ids must not be empty",
        ));
    }
    let mut previous_instrument_id = None;
    for instrument_id in &parts.instrument_ids {
        if let Some(previous) = previous_instrument_id
            && previous >= instrument_id
        {
            return Err(selected_market_requirement_error(
                "selected-market instrument_ids must be sorted and unique",
            ));
        }
        ensure_selected_market_text(SELECTED_MARKET_INSTRUMENT_IDS_FIELD, instrument_id, false)?;
        previous_instrument_id = Some(instrument_id);
    }
    for (key, value) in &parts.metadata_provenance_fields {
        ensure_selected_market_text(SELECTED_MARKET_METADATA_PROVENANCE_KEY_FIELD, key, false)?;
        ensure_selected_market_json_text(SELECTED_MARKET_METADATA_PROVENANCE_FIELD, value)?;
    }

    let metadata_provenance_sha256 = canonical_json_sha256(&parts.metadata_provenance_fields)?;
    let mut requirement = SelectedMarketRequirement {
        configured_target_id: parts.configured_target_id.to_string(),
        venue: parts.venue.to_string(),
        family_key: parts.family_key.to_string(),
        market_id: parts.market_id.to_string(),
        instrument_ids: parts.instrument_ids,
        market_class: parts.market_class.to_string(),
        resolution_kind: parts.resolution_kind.to_string(),
        resolution_identity: parts.resolution_identity.to_string(),
        value_kind: parts.value_kind.to_string(),
        metadata_provenance_sha256,
        selected_market_key: String::new(),
        selected_at_ms: parts.selected_at_ms,
    };
    requirement.selected_market_key = selected_market_key_for_requirement(&requirement)?;
    Ok(requirement)
}

pub(crate) fn selected_market_key_for_requirement(
    requirement: &SelectedMarketRequirement,
) -> Result<String, InstrumentFilterError> {
    let input = BTreeMap::from([
        (
            SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD.to_string(),
            serde_json::json!(requirement.configured_target_id),
        ),
        (
            SELECTED_MARKET_FAMILY_KEY_FIELD.to_string(),
            serde_json::json!(requirement.family_key),
        ),
        (
            SELECTED_MARKET_INSTRUMENT_IDS_FIELD.to_string(),
            serde_json::json!(requirement.instrument_ids),
        ),
        (
            SELECTED_MARKET_MARKET_CLASS_FIELD.to_string(),
            serde_json::json!(requirement.market_class),
        ),
        (
            SELECTED_MARKET_MARKET_ID_FIELD.to_string(),
            serde_json::json!(requirement.market_id),
        ),
        (
            SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD.to_string(),
            serde_json::json!(requirement.metadata_provenance_sha256),
        ),
        (
            SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD.to_string(),
            serde_json::json!(requirement.resolution_identity),
        ),
        (
            SELECTED_MARKET_RESOLUTION_KIND_FIELD.to_string(),
            serde_json::json!(requirement.resolution_kind),
        ),
        (
            SELECTED_MARKET_VALUE_KIND_FIELD.to_string(),
            serde_json::json!(requirement.value_kind),
        ),
        (
            SELECTED_MARKET_VENUE_FIELD.to_string(),
            serde_json::json!(requirement.venue),
        ),
    ]);
    canonical_json_sha256(&input)
}

pub(crate) fn canonical_json_sha256<T>(value: &T) -> Result<String, InstrumentFilterError>
where
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|error| InstrumentFilterError::Other {
        message: format!("selected-market canonical JSON serialization failed: {error}"),
    })?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

fn ensure_selected_market_json_text(
    field: &'static str,
    value: &serde_json::Value,
) -> Result<(), InstrumentFilterError> {
    match value {
        serde_json::Value::String(text) => ensure_selected_market_text(field, text, false),
        serde_json::Value::Array(items) => {
            for item in items {
                ensure_selected_market_json_text(field, item)?;
            }
            Ok(())
        }
        serde_json::Value::Object(map) => {
            for (key, nested) in map {
                ensure_selected_market_text(field, key, false)?;
                ensure_selected_market_json_text(field, nested)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn ensure_selected_market_text(
    field: &'static str,
    value: &str,
    allow_empty: bool,
) -> Result<(), InstrumentFilterError> {
    if !allow_empty && value.is_empty() {
        return Err(selected_market_requirement_error(format!(
            "selected-market {field} must not be empty"
        )));
    }
    if value.contains('|') {
        return Err(selected_market_requirement_error(format!(
            "selected-market {field} must not contain `|`"
        )));
    }
    Ok(())
}

pub(crate) fn selected_market_requirement_error(
    message: impl Into<String>,
) -> InstrumentFilterError {
    InstrumentFilterError::TargetValidationFailure {
        message: message.into(),
    }
}

impl From<updown::BoltV3InstrumentFilterError> for InstrumentFilterError {
    fn from(error: updown::BoltV3InstrumentFilterError) -> Self {
        match error {
            updown::BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => Self::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            },
            updown::BoltV3InstrumentFilterError::NegativeNowUnixSeconds { now_unix_seconds } => {
                Self::NegativeNowUnixSeconds { now_unix_seconds }
            }
            updown::BoltV3InstrumentFilterError::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => Self::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            },
            updown::BoltV3InstrumentFilterError::TargetParseFailed {
                strategy_instance_id,
                message,
            } => Self::TargetParseFailed {
                strategy_instance_id,
                message,
            },
        }
    }
}

/// Target validation entry point used by core startup validation.
/// Returns `(metadata, errors)`: the metadata is `None` when the raw
/// `[target]` value cannot even produce a `configured_target_id` (in
/// which case the family-specific validator's full error set still
/// surfaces in `errors`).
pub fn validate_strategy_target(
    context: &str,
    target: &toml::Value,
) -> (Option<TargetMetadata>, Vec<InstrumentFilterError>) {
    validate_strategy_target_with_bindings(context, target, validation_bindings())
}

pub fn validate_strategy_target_with_bindings(
    context: &str,
    target: &toml::Value,
    bindings: &[MarketFamilyValidationBinding],
) -> (Option<TargetMetadata>, Vec<InstrumentFilterError>) {
    let metadata = target.clone().try_into::<TargetMetadata>().ok();
    let dispatch: TargetFamilyDispatch = match target.clone().try_into() {
        Ok(value) => value,
        Err(error) => {
            return (
                metadata,
                vec![InstrumentFilterError::Other {
                    message: format!("{context}: target: {error}"),
                }],
            );
        }
    };
    let errors = match bindings
        .iter()
        .find(|binding| binding.key == dispatch.rotating_market_family)
    {
        Some(binding) => (binding.validate_target)(context, target)
            .into_iter()
            .map(|message| InstrumentFilterError::TargetValidationFailure { message })
            .collect(),
        None => vec![InstrumentFilterError::UnsupportedFamily {
            context: Some(context.to_string()),
            family_key: dispatch.rotating_market_family.clone(),
            supported: bindings.iter().map(|b| b.key).collect(),
        }],
    };
    (metadata, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_validate_target(_context: &str, _target: &toml::Value) -> Vec<String> {
        Vec::new()
    }

    const FAKE_FAMILY_BINDINGS: &[MarketFamilyValidationBinding] =
        &[MarketFamilyValidationBinding {
            key: "fixture_family",
            validate_target: fake_validate_target,
            instrument_filter_target_for_strategy: fake_instrument_filter_target_for_strategy,
            target_runtime_fields: fake_target_runtime_fields,
            select_binary_option_market: fake_select_binary_option_market,
            market_selection_candidate_windows: fake_market_selection_candidate_windows,
            selected_market_requirement: fake_selected_market_requirement,
        }];

    fn fake_instrument_filter_target_for_strategy(
        strategy: &LoadedStrategy,
    ) -> Result<Option<InstrumentFilterTarget>, InstrumentFilterError> {
        Err(InstrumentFilterError::Other {
            message: format!(
                "fixture_family binding invoked for strategy `{}`",
                strategy.config.strategy_instance_id
            ),
        })
    }

    fn fake_target_runtime_fields(
        _target: &toml::Value,
    ) -> Result<TargetRuntimeFields, InstrumentFilterError> {
        Err(InstrumentFilterError::Other {
            message: "fixture_family target runtime binding invoked".to_string(),
        })
    }

    fn fake_select_binary_option_market(
        _target: MarketSelectionTarget<'_>,
        _instruments: &[InstrumentAny],
        _now_milliseconds: u64,
    ) -> Option<SelectedBinaryOptionMarket> {
        Some(fake_selected_binary_option_market())
    }

    fn fake_selected_binary_option_market() -> SelectedBinaryOptionMarket {
        SelectedBinaryOptionMarket {
            market_id: "fixture-market".to_string(),
            instrument_id: InstrumentId::from("fixture-market.FIXTURE"),
            up_instrument_id: InstrumentId::from("fixture-up.FIXTURE"),
            down_instrument_id: InstrumentId::from("fixture-down.FIXTURE"),
            selection_outcome: MarketSelectionOutcome::Current,
            start_timestamp_milliseconds: 1_000,
            expiration_timestamp_milliseconds: 61_000,
            seconds_to_end: 60,
            source_identity: SelectedMarketSourceIdentity {
                condition_id: "fixture-condition".to_string(),
                market_slug: "fixture-market".to_string(),
                question_id: "fixture-question".to_string(),
            },
        }
    }

    fn fake_selected_market_requirement(
        _target: &toml::Value,
        selected: &SelectedBinaryOptionMarket,
        selected_at_ms: u64,
    ) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
        selected_market_requirement_from_parts(SelectedMarketRequirementParts {
            configured_target_id: "fixture-target",
            venue: "FIXTURE",
            family_key: "fixture_family",
            market_id: selected.market_id.as_str(),
            instrument_ids: vec![
                selected.down_instrument_id.to_string(),
                selected.up_instrument_id.to_string(),
            ],
            market_class: "fixture_market",
            resolution_kind: "fixture_resolution",
            resolution_identity: "fixture-resolution-1",
            value_kind: "fixture_value",
            metadata_provenance_fields: selected_market_metadata_provenance_fields([
                ("family_key", "fixture_family"),
                ("market_class", "fixture_market"),
                ("market_id", selected.market_id.as_str()),
                ("source_kind", "fixture_metadata"),
                ("venue", "FIXTURE"),
            ]),
            selected_at_ms,
        })
    }

    fn fixture_requirement_parts(
        market_id: &'static str,
        selected_at_ms: u64,
    ) -> SelectedMarketRequirementParts<'static> {
        SelectedMarketRequirementParts {
            configured_target_id: "target-a",
            venue: "FIXTURE",
            family_key: "fixture_family",
            market_id,
            instrument_ids: vec![
                "fixture-down.FIXTURE".to_string(),
                "fixture-up.FIXTURE".to_string(),
            ],
            market_class: "fixture_market",
            resolution_kind: "fixture_resolution",
            resolution_identity: "fixture-resolution-1",
            value_kind: "fixture_value",
            metadata_provenance_fields: selected_market_metadata_provenance_fields([
                ("family_key", "fixture_family"),
                ("market_class", "fixture_market"),
                ("market_id", market_id),
                ("source_kind", "fixture_metadata"),
                ("venue", "FIXTURE"),
            ]),
            selected_at_ms,
        }
    }

    fn fake_market_selection_candidate_windows(
        _target: MarketSelectionTarget<'_>,
        _now_milliseconds: u64,
    ) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
        Ok(vec![MarketSelectionCandidateWindow {
            outcome: MarketSelectionOutcome::Current,
            market_slug: "fixture-market".to_string(),
            start_timestamp_milliseconds: 1_000,
        }])
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root: crate::bolt_v3_config::BoltV3RootConfig =
            toml::from_str(include_str!("../../tests/fixtures/bolt_v3/root.toml")).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: "test-config-bundle-checksum".to_string(),
            root,
            strategies: Vec::new(),
        }
    }

    /// Deserialize the fixture strategy and overwrite its
    /// `target.rotating_market_family` discriminator. Used by tests
    /// that need a non-updown strategy in `loaded.strategies` so the
    /// per-strategy dispatcher routes to an injected fake binding.
    fn fixture_strategy_with_family(family: &str) -> LoadedStrategy {
        let strategy_config: crate::bolt_v3_config::BoltV3StrategyConfig = toml::from_str(
            include_str!("../../tests/fixtures/bolt_v3/strategies/binary_oracle.toml"),
        )
        .unwrap();
        let mut strategy = LoadedStrategy {
            config_path: std::path::PathBuf::from(
                "tests/fixtures/bolt_v3/strategies/binary_oracle.toml",
            ),
            relative_path: "strategies/binary_oracle.toml".to_string(),
            config: strategy_config,
        };
        strategy
            .config
            .target
            .as_table_mut()
            .expect("strategy [target] should be a TOML table")
            .insert(
                "rotating_market_family".to_string(),
                toml::Value::String(family.to_string()),
            );
        strategy
    }

    /// Deserialize the fixture strategy and remove the
    /// `rotating_market_family` discriminator from its raw `[target]`
    /// envelope. Used by tests that exercise the parent dispatcher's
    /// own `TargetParseFailed` arm (raw TOML missing the field the
    /// dispatcher reads to route).
    fn fixture_strategy_without_family_discriminator() -> LoadedStrategy {
        let mut strategy = fixture_strategy_with_family("updown");
        strategy
            .config
            .target
            .as_table_mut()
            .expect("strategy [target] should be a TOML table")
            .remove("rotating_market_family");
        strategy
    }

    #[test]
    fn validation_can_use_injected_family_binding_without_editing_production_registry() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let (_, production_errors) = validate_strategy_target("strategy `fixture`", &target);
        assert!(
            production_errors
                .iter()
                .any(|error| error.to_string().contains("not supported by this build")),
            "production registry should not know the test family: {production_errors:?}"
        );

        let (_, injected_errors) = validate_strategy_target_with_bindings(
            "strategy `fixture`",
            &target,
            FAKE_FAMILY_BINDINGS,
        );
        assert!(
            injected_errors.is_empty(),
            "injected family binding should own target dispatch: {injected_errors:?}"
        );
    }

    #[test]
    fn instrument_filters_use_injected_family_binding_without_parent_family_branch() {
        let mut loaded = fixture_loaded_config();
        loaded
            .strategies
            .push(fixture_strategy_with_family("fixture_family"));

        // Production registry has only updown; a fixture_family
        // strategy must be rejected as UnsupportedFamily, not silently
        // dispatched to updown.
        let production_error = instrument_filters_from_config(&loaded)
            .expect_err("production registry must reject unknown family");
        match &production_error {
            InstrumentFilterError::UnsupportedFamily { family_key, .. } => {
                assert_eq!(family_key, "fixture_family");
            }
            other => panic!("expected UnsupportedFamily, got {other:?}"),
        }

        // Injected fake binding owns dispatch for the fixture_family
        // strategy and returns its own error, proving the per-strategy
        // dispatcher routes by `target.rotating_market_family`.
        let injected_error =
            instrument_filters_from_config_with_bindings(&loaded, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error.to_string(),
            format!(
                "fixture_family binding invoked for strategy `{}`",
                loaded.strategies[0].config.strategy_instance_id
            )
        );
    }

    #[test]
    fn instrument_filters_dispatch_routes_each_strategy_to_its_family_binding() {
        // Two strategies, one updown and one fixture_family. The
        // per-strategy dispatcher must read each strategy's
        // `target.rotating_market_family` and call only the matching
        // binding. With the prior broadcast dispatch, the updown
        // binding would have iterated every strategy and failed
        // updown-typed deserialization on the fixture_family strategy
        // before the fake binding could handle it.
        let mut loaded = fixture_loaded_config();
        let updown_strategy = fixture_strategy_with_family(updown::KEY);
        let fake_strategy = fixture_strategy_with_family("fixture_family");
        loaded.strategies.push(updown_strategy);
        loaded.strategies.push(fake_strategy);

        let combined_bindings: Vec<MarketFamilyValidationBinding> = validation_bindings()
            .iter()
            .chain(FAKE_FAMILY_BINDINGS.iter())
            .cloned()
            .collect();

        // The fake binding errors loud when its strategy reaches it.
        // The dispatcher must surface that error, proving the fake
        // strategy was routed to the fake binding and not to updown.
        let dispatch_error =
            instrument_filters_from_config_with_bindings(&loaded, &combined_bindings)
                .expect_err("fake binding must reject the fixture_family strategy");
        match &dispatch_error {
            InstrumentFilterError::Other { message } => {
                assert!(
                    message.contains("fixture_family binding invoked for strategy"),
                    "fake binding should surface its own error, not an updown deserialization \
                     failure: {message}"
                );
            }
            other => panic!(
                "expected fake binding's Other error, got {other:?} — \
                 a TargetParseFailed here means updown was incorrectly called on the \
                 fixture_family strategy"
            ),
        }
    }

    #[test]
    fn instrument_filters_dispatcher_rejects_strategy_with_missing_family_discriminator() {
        // The parent dispatcher reads `target.rotating_market_family`
        // from each strategy's raw TOML before routing to a family
        // binding. If the discriminator field is absent, the dispatcher
        // must surface its own `TargetParseFailed` keyed on the
        // strategy id — not silently fall through to UnsupportedFamily
        // or to a family binding.
        let mut loaded = fixture_loaded_config();
        loaded
            .strategies
            .push(fixture_strategy_without_family_discriminator());
        let strategy_id = loaded.strategies[0].config.strategy_instance_id.clone();

        match instrument_filters_from_config(&loaded) {
            Err(InstrumentFilterError::TargetParseFailed {
                strategy_instance_id,
                message,
            }) => {
                assert_eq!(strategy_instance_id, strategy_id);
                assert!(
                    message.contains("rotating_market_family"),
                    "error message should name the missing discriminator field: {message}"
                );
                assert!(
                    message.contains("missing field"),
                    "error message should say missing field: {message}"
                );
            }
            other => panic!(
                "expected TargetParseFailed from the parent dispatcher when \
                 rotating_market_family is absent, got {other:?}"
            ),
        }
    }

    #[test]
    fn target_runtime_fields_use_injected_family_binding_without_parent_family_branch() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();

        let production_error = target_runtime_fields_from_target(&target)
            .expect_err("production registry should not know the test family");
        assert!(
            production_error
                .to_string()
                .contains("not supported by this build"),
            "production registry should not know the test family: {production_error}"
        );

        let injected_error =
            target_runtime_fields_from_target_with_bindings(&target, FAKE_FAMILY_BINDINGS)
                .expect_err("fake binding should own this dispatch and return its error");
        assert_eq!(
            injected_error.to_string(),
            "fixture_family target runtime binding invoked"
        );
    }

    #[test]
    fn market_selection_uses_injected_family_binding_without_parent_family_branch() {
        let target = MarketSelectionTarget {
            family_key: "fixture_family",
            underlying_asset: "FIXTURE",
            cadence_seconds: 60,
            cadence_slug_token: "fixture",
        };

        assert!(
            select_binary_option_market_from_target(target, &[], 0).is_none(),
            "production registry should not know the test family"
        );

        let selected = select_binary_option_market_from_target_with_bindings(
            target,
            &[],
            0,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own market selection dispatch");

        assert_eq!(selected.market_id, "fixture-market");
    }

    #[test]
    fn selected_market_requirement_uses_injected_family_binding_without_parent_family_branch() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "fixture_family"
        }
        .into();
        let selected = fake_selected_binary_option_market();

        let production_error = selected_market_requirement_from_target(&target, &selected, 123)
            .expect_err("production registry should not know the test family");
        assert!(
            production_error
                .to_string()
                .contains("not supported by this build"),
            "production registry should not know the test family: {production_error}"
        );

        let requirement = selected_market_requirement_from_target_with_bindings(
            &target,
            &selected,
            123,
            FAKE_FAMILY_BINDINGS,
        )
        .expect("injected family binding should own requirement extraction");

        assert_eq!(requirement.configured_target_id, "fixture-target");
        assert_eq!(requirement.family_key, "fixture_family");
        assert_eq!(requirement.selected_at_ms, 123);
        assert_eq!(
            requirement.instrument_ids,
            vec!["fixture-down.FIXTURE", "fixture-up.FIXTURE"]
        );
    }

    #[test]
    fn selected_market_key_excludes_selected_at_and_tracks_identity_fields() {
        let first = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            123,
        ))
        .expect("fixture requirement should build");
        let later = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            456,
        ))
        .expect("later fixture requirement should build");
        assert_eq!(first.selected_market_key, later.selected_market_key);

        let changed = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market-2",
            123,
        ))
        .expect("changed fixture requirement should build");
        assert_ne!(first.selected_market_key, changed.selected_market_key);
    }

    #[test]
    fn selected_market_requirement_rejects_unsorted_or_duplicate_instrument_ids() {
        let mut unsorted = fixture_requirement_parts("fixture-market", 123);
        unsorted.instrument_ids = vec![
            "fixture-up.FIXTURE".to_string(),
            "fixture-down.FIXTURE".to_string(),
        ];
        let error = selected_market_requirement_from_parts(unsorted)
            .expect_err("unsorted instrument ids should fail closed");
        assert!(
            error.to_string().contains("sorted and unique"),
            "expected sorted ids rejection, got: {error}"
        );

        let mut duplicate = fixture_requirement_parts("fixture-market", 123);
        duplicate.instrument_ids = vec![
            "fixture-up.FIXTURE".to_string(),
            "fixture-up.FIXTURE".to_string(),
        ];
        let error = selected_market_requirement_from_parts(duplicate)
            .expect_err("duplicate instrument ids should fail closed");
        assert!(
            error.to_string().contains("sorted and unique"),
            "expected unique ids rejection, got: {error}"
        );
    }

    #[test]
    fn selected_market_requirement_rejects_empty_metadata_provenance_strings() {
        let mut parts = fixture_requirement_parts("fixture-market", 123);
        parts.metadata_provenance_fields = selected_market_metadata_provenance_fields([
            ("family_key", "fixture_family"),
            ("source_kind", ""),
        ]);

        let error = selected_market_requirement_from_parts(parts)
            .expect_err("empty provenance string should fail closed");
        assert!(
            error.to_string().contains("metadata_provenance"),
            "expected provenance string rejection, got: {error}"
        );
    }

    #[test]
    fn selected_market_key_uses_lexicographically_sorted_canonical_json() {
        use std::collections::BTreeMap;

        let requirement = selected_market_requirement_from_parts(fixture_requirement_parts(
            "fixture-market",
            123,
        ))
        .expect("fixture requirement should build");

        let expected_input = BTreeMap::from([
            (
                SELECTED_MARKET_CONFIGURED_TARGET_ID_FIELD.to_string(),
                serde_json::json!("target-a"),
            ),
            (
                SELECTED_MARKET_FAMILY_KEY_FIELD.to_string(),
                serde_json::json!("fixture_family"),
            ),
            (
                SELECTED_MARKET_INSTRUMENT_IDS_FIELD.to_string(),
                serde_json::json!(["fixture-down.FIXTURE", "fixture-up.FIXTURE"]),
            ),
            (
                SELECTED_MARKET_MARKET_CLASS_FIELD.to_string(),
                serde_json::json!("fixture_market"),
            ),
            (
                SELECTED_MARKET_MARKET_ID_FIELD.to_string(),
                serde_json::json!("fixture-market"),
            ),
            (
                SELECTED_MARKET_METADATA_PROVENANCE_SHA256_FIELD.to_string(),
                serde_json::json!(requirement.metadata_provenance_sha256),
            ),
            (
                SELECTED_MARKET_RESOLUTION_IDENTITY_FIELD.to_string(),
                serde_json::json!("fixture-resolution-1"),
            ),
            (
                SELECTED_MARKET_RESOLUTION_KIND_FIELD.to_string(),
                serde_json::json!("fixture_resolution"),
            ),
            (
                SELECTED_MARKET_VALUE_KIND_FIELD.to_string(),
                serde_json::json!("fixture_value"),
            ),
            (
                SELECTED_MARKET_VENUE_FIELD.to_string(),
                serde_json::json!("FIXTURE"),
            ),
        ]);
        assert_eq!(
            requirement.selected_market_key,
            canonical_json_sha256(&expected_input).expect("canonical BTreeMap should hash")
        );
    }

    #[test]
    fn from_internal_preserves_typed_non_positive_cadence_seconds() {
        let internal = updown::BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id: Some("alpha".to_string()),
            configured_target_id: Some("target_a".to_string()),
            cadence_seconds: -1,
        };

        let public: InstrumentFilterError = internal.into();

        match public {
            InstrumentFilterError::NonPositiveCadenceSeconds {
                strategy_instance_id,
                configured_target_id,
                cadence_seconds,
            } => {
                assert_eq!(strategy_instance_id.as_deref(), Some("alpha"));
                assert_eq!(configured_target_id.as_deref(), Some("target_a"));
                assert_eq!(cadence_seconds, -1);
            }
            other => panic!("expected NonPositiveCadenceSeconds, got {other:?}"),
        }
    }

    #[test]
    fn display_for_non_positive_cadence_seconds_preserves_internal_operator_message() {
        let strategy_instance_id = Some("alpha".to_string());
        let configured_target_id = Some("target_a".to_string());
        let cadence_seconds = -1;

        let public = InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id: strategy_instance_id.clone(),
            configured_target_id: configured_target_id.clone(),
            cadence_seconds,
        };
        let internal = updown::BoltV3InstrumentFilterError::NonPositiveCadenceSeconds {
            strategy_instance_id,
            configured_target_id,
            cadence_seconds,
        };
        assert_eq!(public.to_string(), internal.to_string());
    }

    #[test]
    fn from_internal_preserves_typed_negative_now_unix_seconds() {
        let internal = updown::BoltV3InstrumentFilterError::NegativeNowUnixSeconds {
            now_unix_seconds: -42,
        };
        let internal_message = internal.to_string();

        let public: InstrumentFilterError = internal.into();

        match &public {
            InstrumentFilterError::NegativeNowUnixSeconds { now_unix_seconds } => {
                assert_eq!(*now_unix_seconds, -42);
            }
            other => panic!("expected NegativeNowUnixSeconds, got {other:?}"),
        }
        assert_eq!(public.to_string(), internal_message);
    }

    #[test]
    fn from_internal_preserves_typed_period_pair_overflow() {
        let internal = updown::BoltV3InstrumentFilterError::PeriodPairOverflow {
            now_unix_seconds: i64::MAX,
            cadence_seconds: 60,
        };
        let internal_message = internal.to_string();

        let public: InstrumentFilterError = internal.into();

        match &public {
            InstrumentFilterError::PeriodPairOverflow {
                now_unix_seconds,
                cadence_seconds,
            } => {
                assert_eq!(*now_unix_seconds, i64::MAX);
                assert_eq!(*cadence_seconds, 60);
            }
            other => panic!("expected PeriodPairOverflow, got {other:?}"),
        }
        assert_eq!(public.to_string(), internal_message);
    }

    #[test]
    fn from_internal_preserves_typed_target_parse_failed() {
        let internal = updown::BoltV3InstrumentFilterError::TargetParseFailed {
            strategy_instance_id: "alpha".to_string(),
            message: "missing field `cadence_seconds`".to_string(),
        };
        let internal_message = internal.to_string();

        let public: InstrumentFilterError = internal.into();

        match &public {
            InstrumentFilterError::TargetParseFailed {
                strategy_instance_id,
                message,
            } => {
                assert_eq!(strategy_instance_id, "alpha");
                assert_eq!(message, "missing field `cadence_seconds`");
            }
            other => panic!("expected TargetParseFailed, got {other:?}"),
        }
        assert_eq!(public.to_string(), internal_message);
    }

    #[test]
    fn validate_strategy_target_wraps_target_block_errors_as_typed_target_validation_failure() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "CONFIGURED_ASSET"
            cadence_secs = -1
            cadence_slug_token = "configuredwindow"
            market_selection_rule = "active_or_next"
            retry_interval_secs = 1
            blocked_after_secs = 1
        }
        .into();

        let (_, errors) = validate_strategy_target("strategy `alpha`", &target);
        let cadence_failure = errors.iter().find(|e| {
            matches!(
                e,
                InstrumentFilterError::TargetValidationFailure { message, .. }
                    if message.contains("target.cadence_secs")
            )
        });
        assert!(
            cadence_failure.is_some(),
            "expected TargetValidationFailure for cadence_secs: {errors:#?}"
        );
    }

    #[test]
    fn validate_strategy_target_emits_typed_unsupported_family_for_unknown_key() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "unicorn"
        }
        .into();

        let (_, errors) = validate_strategy_target("strategy `alpha`", &target);
        let unsupported = errors.iter().find_map(|e| match e {
            InstrumentFilterError::UnsupportedFamily {
                context,
                family_key,
                ..
            } => Some((context.clone(), family_key.clone())),
            _ => None,
        });
        assert_eq!(
            unsupported,
            Some((Some("strategy `alpha`".to_string()), "unicorn".to_string()))
        );
    }

    #[test]
    fn target_runtime_fields_returns_typed_unsupported_family_for_unknown_key() {
        let target = toml::toml! {
            configured_target_id = "fixture-target"
            rotating_market_family = "unicorn"
        }
        .into();

        let error = target_runtime_fields_from_target(&target).expect_err("unknown family");
        match error {
            InstrumentFilterError::UnsupportedFamily {
                context,
                family_key,
                supported,
            } => {
                assert_eq!(context, None);
                assert_eq!(family_key, "unicorn");
                assert!(
                    supported.contains(&updown::KEY),
                    "supported list should include the registered family keys: {supported:?}"
                );
            }
            other => panic!("expected UnsupportedFamily, got {other:?}"),
        }
    }
}
