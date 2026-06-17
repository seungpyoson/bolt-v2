//! Static binary-event market-family identity binding for configured Polymarket events.

use std::{sync::Arc, time::Duration};

use nautilus_model::{identifiers::InstrumentId, instruments::InstrumentAny};
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::LoadedStrategy,
    bolt_v3_instrument_filters::InstrumentFilterError,
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::{FamilyQuoteInputs, QuoteTargets},
};

use super::{
    FairProbabilityInputs, MarketIdentityPlan, MarketIdentityTarget,
    MarketSelectionCandidateWindow, MarketSelectionOutcome, MarketSelectionTarget, OutcomeSide,
    SelectedBinaryOptionMarket, SelectedMarketRequirement, SelectedMarketRequirementParts,
    SelectedMarketSourceIdentity, TargetRuntimeFields, selected_market_metadata_provenance_fields,
    selected_market_requirement_error, selected_market_requirement_from_parts, updown,
};

pub const KEY: &str = "static_binary_event";
pub const REFERENCE_CURRENT_PRICE_FAIR_PROBABILITY_SOURCE: &str = "reference_current_price";
const BINARY_OPTION_MARKET_CLASS: &str = "binary_option";
const NT_INSTRUMENT_METADATA_SOURCE_KIND: &str = "nt_instrument_metadata";
const REQUIRED_STATIC_OUTCOME_INSTRUMENT_COUNT: usize = 2;
const TARGET_ENUM_SERIALIZE_FAILURE_MESSAGE: &str =
    "static_binary_event target discriminator enum could not serialize to a string token";
const METADATA_CONDITION_ID_FIELD: &str = "condition_id";
const METADATA_FAMILY_KEY_FIELD: &str = "family_key";
const METADATA_INSTRUMENT_IDS_FIELD: &str = "instrument_ids";
const METADATA_MARKET_CLASS_FIELD: &str = "market_class";
const METADATA_MARKET_ID_FIELD: &str = "market_id";
const METADATA_MARKET_SLUG_FIELD: &str = "market_slug";
const METADATA_NO_OUTCOME_FIELD: &str = "no_outcome";
const METADATA_QUESTION_ID_FIELD: &str = "question_id";
const METADATA_SOURCE_KIND_FIELD: &str = "source_kind";
const METADATA_VENUE_FIELD: &str = "venue";
const METADATA_YES_OUTCOME_FIELD: &str = "yes_outcome";
const STATIC_RESOLUTION_KIND: &str = "polymarket_condition";
const STATIC_VALUE_KIND: &str = "binary_outcome";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TargetBlock {
    pub configured_target_id: String,
    pub kind: TargetKind,
    pub rotating_market_family: RotatingMarketFamily,
    pub event_key: String,
    pub market_slug: String,
    pub condition_id: Option<String>,
    pub yes_outcome: String,
    pub no_outcome: String,
    pub fair_probability_source: FairProbabilitySource,
    pub selection_window_secs: i64,
    pub market_selection_rule: MarketSelectionRule,
    pub retry_interval_secs: u64,
    pub blocked_after_secs: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    StaticMarket,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RotatingMarketFamily {
    StaticBinaryEvent,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MarketSelectionRule {
    ConfiguredStatic,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FairProbabilitySource {
    ReferenceCurrentPrice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticBinaryEventTargetPlan {
    pub strategy_instance_id: String,
    pub configured_target_id: String,
    pub execution_client_id: String,
    pub event_key: String,
    pub market_slug: String,
    pub condition_id: Option<String>,
    pub yes_outcome: String,
    pub no_outcome: String,
}

impl MarketIdentityTarget for StaticBinaryEventTargetPlan {
    fn family_key(&self) -> &'static str {
        KEY
    }

    fn configured_target_id(&self) -> &str {
        &self.configured_target_id
    }

    fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub fn target_plans(
    plan: &MarketIdentityPlan,
) -> impl Iterator<Item = &StaticBinaryEventTargetPlan> {
    plan.targets().filter_map(|target| {
        target
            .as_any()
            .downcast_ref::<StaticBinaryEventTargetPlan>()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StaticBinaryEventSelectionTarget<'a> {
    market_slug: &'a str,
    condition_id: Option<&'a str>,
    yes_outcome: &'a str,
    no_outcome: &'a str,
}

#[derive(Debug, Clone)]
struct StaticOutcomeInstrument {
    side: OutcomeSide,
    market_id: String,
    condition_id: String,
    market_slug: String,
    question_id: String,
    instrument_id: InstrumentId,
    activation_milliseconds: u64,
    expiration_milliseconds: u64,
}

#[derive(Debug)]
struct StaticOutcomePair {
    yes: Option<StaticOutcomeInstrument>,
    no: Option<StaticOutcomeInstrument>,
}

impl StaticOutcomePair {
    fn empty() -> Self {
        Self {
            yes: None,
            no: None,
        }
    }
}

pub fn deserialize_target_block(target: &toml::Value) -> Result<TargetBlock, String> {
    target
        .clone()
        .try_into::<TargetBlock>()
        .map_err(|error| error.to_string())
}

pub fn validate_target_block(context: &str, target: &toml::Value) -> Vec<String> {
    let block = match deserialize_target_block(target) {
        Ok(value) => value,
        Err(message) => return vec![format!("{context}: target: {message}")],
    };
    validate_static_target_block(context, &block)
}

pub fn plan_strategy_target(
    strategy: &LoadedStrategy,
) -> Result<Option<Arc<dyn MarketIdentityTarget>>, InstrumentFilterError> {
    let strategy_instance_id = strategy.config.strategy_instance_id.clone();
    let execution_client_id = strategy.config.execution_client_id.to_string();
    let target = deserialize_target_block(&strategy.config.target).map_err(|message| {
        InstrumentFilterError::TargetParseFailed {
            strategy_instance_id: strategy_instance_id.clone(),
            message,
        }
    })?;
    let errors =
        validate_static_target_block(&format!("strategy `{strategy_instance_id}`"), &target);
    if let Some(message) = errors.into_iter().next() {
        return Err(InstrumentFilterError::TargetValidationFailure { message });
    }

    Ok(Some(Arc::new(StaticBinaryEventTargetPlan {
        strategy_instance_id,
        configured_target_id: target.configured_target_id,
        execution_client_id,
        event_key: target.event_key,
        market_slug: target.market_slug,
        condition_id: target.condition_id,
        yes_outcome: target.yes_outcome,
        no_outcome: target.no_outcome,
    })))
}

pub fn target_runtime_fields(
    target: &toml::Value,
) -> Result<TargetRuntimeFields, InstrumentFilterError> {
    let target = deserialize_target_block(target)
        .map_err(|message| InstrumentFilterError::Other { message })?;
    let errors = validate_static_target_block("", &target);
    if let Some(message) = errors.into_iter().next() {
        return Err(InstrumentFilterError::TargetValidationFailure { message });
    }
    Ok(TargetRuntimeFields {
        configured_target_id: target.configured_target_id,
        target_kind: target_runtime_string(target.kind)?,
        rotating_market_family: target_runtime_string(target.rotating_market_family)?,
        underlying_asset: target.event_key,
        cadence_seconds: target.selection_window_secs,
        cadence_seconds_source_field: "target.selection_window_secs",
        cadence_slug_token: target.market_slug.clone(),
        market_selection_rule: target_runtime_string(target.market_selection_rule)?,
        static_market_slug: Some(target.market_slug),
        static_condition_id: target.condition_id,
        static_yes_outcome: Some(target.yes_outcome),
        static_no_outcome: Some(target.no_outcome),
        static_fair_probability_source: Some(target_runtime_string(
            target.fair_probability_source,
        )?),
        retry_interval_seconds: target.retry_interval_secs,
        blocked_after_seconds: target.blocked_after_secs,
    })
}

pub fn select_binary_option_market(
    target: MarketSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedBinaryOptionMarket> {
    let (Some(yes_outcome), Some(no_outcome)) =
        (target.static_yes_outcome, target.static_no_outcome)
    else {
        log::error!(
            "bolt-v3 static_binary_event selection: missing configured yes/no outcome labels for market_slug `{}`; selecting no market",
            target.cadence_slug_token
        );
        return None;
    };
    let market = select_static_market_from_instruments(
        StaticBinaryEventSelectionTarget {
            market_slug: target.cadence_slug_token,
            condition_id: target.static_condition_id,
            yes_outcome,
            no_outcome,
        },
        instruments,
        now_milliseconds,
    )?;
    Some(SelectedBinaryOptionMarket {
        market_id: market.market_id,
        instrument_id: market.instrument_id,
        up_instrument_id: market.up_instrument_id,
        down_instrument_id: market.down_instrument_id,
        selection_outcome: market.selection_outcome,
        start_timestamp_milliseconds: market.start_timestamp_milliseconds,
        expiration_timestamp_milliseconds: market.expiration_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
        source_identity: market.source_identity,
    })
}

pub fn market_selection_candidate_windows(
    target: MarketSelectionTarget<'_>,
    now_milliseconds: u64,
) -> Result<Vec<MarketSelectionCandidateWindow>, InstrumentFilterError> {
    Ok(vec![MarketSelectionCandidateWindow {
        outcome: MarketSelectionOutcome::Current,
        market_slug: target.cadence_slug_token.to_string(),
        start_timestamp_milliseconds: now_milliseconds,
    }])
}

pub fn selected_market_requirement(
    target: &toml::Value,
    selected: &SelectedBinaryOptionMarket,
    selected_at_ms: u64,
) -> Result<SelectedMarketRequirement, InstrumentFilterError> {
    let target = deserialize_target_block(target)
        .map_err(|message| InstrumentFilterError::Other { message })?;
    let mut instrument_ids = vec![
        selected.down_instrument_id.to_string(),
        selected.up_instrument_id.to_string(),
    ];
    instrument_ids.sort();
    instrument_ids.dedup();
    if instrument_ids.len() != REQUIRED_STATIC_OUTCOME_INSTRUMENT_COUNT {
        return Err(selected_market_requirement_error(
            "selected-market instrument_ids must include distinct yes/no outcomes",
        ));
    }
    let yes_venue = selected.up_instrument_id.venue.as_str();
    let no_venue = selected.down_instrument_id.venue.as_str();
    if yes_venue != no_venue {
        return Err(selected_market_requirement_error(
            "selected-market yes/no instrument venues must match",
        ));
    }

    let mut provenance_fields = selected_market_metadata_provenance_fields([
        (
            METADATA_CONDITION_ID_FIELD,
            selected.source_identity.condition_id.as_str(),
        ),
        (METADATA_FAMILY_KEY_FIELD, KEY),
        (METADATA_MARKET_CLASS_FIELD, BINARY_OPTION_MARKET_CLASS),
        (METADATA_MARKET_ID_FIELD, selected.market_id.as_str()),
        (
            METADATA_MARKET_SLUG_FIELD,
            selected.source_identity.market_slug.as_str(),
        ),
        (METADATA_NO_OUTCOME_FIELD, target.no_outcome.as_str()),
        (
            METADATA_QUESTION_ID_FIELD,
            selected.source_identity.question_id.as_str(),
        ),
        (
            METADATA_SOURCE_KIND_FIELD,
            NT_INSTRUMENT_METADATA_SOURCE_KIND,
        ),
        (METADATA_VENUE_FIELD, yes_venue),
        (METADATA_YES_OUTCOME_FIELD, target.yes_outcome.as_str()),
    ]);
    provenance_fields.insert(
        METADATA_INSTRUMENT_IDS_FIELD.to_string(),
        serde_json::json!(instrument_ids),
    );

    selected_market_requirement_from_parts(SelectedMarketRequirementParts {
        configured_target_id: target.configured_target_id.as_str(),
        venue: yes_venue,
        family_key: KEY,
        market_id: selected.market_id.as_str(),
        instrument_ids,
        market_class: BINARY_OPTION_MARKET_CLASS,
        resolution_kind: STATIC_RESOLUTION_KIND,
        resolution_identity: selected.source_identity.condition_id.as_str(),
        value_kind: STATIC_VALUE_KIND,
        metadata_provenance_fields: provenance_fields,
        selected_at_ms,
    })
}

pub fn fair_probability_up(_inputs: &FairProbabilityInputs) -> Option<f64> {
    None
}

pub fn maker_quote_targets(inputs: FamilyQuoteInputs) -> Option<QuoteTargets> {
    updown::maker_quote_targets(inputs)
}

pub fn maker_settlement_payout(outcome: OutcomeSide, leg: Leg) -> Option<f64> {
    updown::maker_settlement_payout(outcome, leg)
}

pub fn maker_binary_fee_curve(fee_rate: f64, price: f64) -> Option<f64> {
    updown::maker_binary_fee_curve(fee_rate, price)
}

fn validate_static_target_block(context: &str, block: &TargetBlock) -> Vec<String> {
    let mut errors = Vec::new();
    validate_non_empty(
        context,
        "target.configured_target_id",
        block.configured_target_id.as_str(),
        &mut errors,
    );
    validate_event_key(context, block.event_key.as_str(), &mut errors);
    validate_market_slug(context, block.market_slug.as_str(), &mut errors);
    if let Some(condition_id) = &block.condition_id {
        validate_non_empty(
            context,
            "target.condition_id",
            condition_id.as_str(),
            &mut errors,
        );
    }
    validate_non_empty(
        context,
        "target.yes_outcome",
        block.yes_outcome.as_str(),
        &mut errors,
    );
    validate_non_empty(
        context,
        "target.no_outcome",
        block.no_outcome.as_str(),
        &mut errors,
    );
    if !block.yes_outcome.is_empty()
        && !block.no_outcome.is_empty()
        && block.yes_outcome == block.no_outcome
    {
        errors.push(format!(
            "{context}: target.yes_outcome and target.no_outcome must be distinct"
        ));
    }
    if block.selection_window_secs <= 0 {
        errors.push(format!(
            "{context}: target.selection_window_secs must be a positive integer (got {})",
            block.selection_window_secs
        ));
    }
    if block.retry_interval_secs == 0 {
        errors.push(format!(
            "{context}: target.retry_interval_secs must be a positive integer"
        ));
    }
    if block.blocked_after_secs == 0 {
        errors.push(format!(
            "{context}: target.blocked_after_secs must be a positive integer"
        ));
    }

    let TargetKind::StaticMarket = block.kind;
    let RotatingMarketFamily::StaticBinaryEvent = block.rotating_market_family;
    let MarketSelectionRule::ConfiguredStatic = block.market_selection_rule;
    let FairProbabilitySource::ReferenceCurrentPrice = block.fair_probability_source;

    errors
}

fn validate_non_empty(context: &str, field: &str, value: &str, errors: &mut Vec<String>) {
    if value.is_empty() {
        errors.push(format!("{context}: {field} must not be empty"));
    }
}

fn validate_event_key(context: &str, event_key: &str, errors: &mut Vec<String>) {
    if event_key.is_empty() {
        errors.push(format!("{context}: target.event_key must not be empty"));
    } else if !event_key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        errors.push(format!(
            "{context}: target.event_key must use only lowercase ASCII letters, digits, and underscores (got `{event_key}`)"
        ));
    }
}

fn validate_market_slug(context: &str, market_slug: &str, errors: &mut Vec<String>) {
    if market_slug.is_empty() {
        errors.push(format!("{context}: target.market_slug must not be empty"));
    } else if !market_slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        errors.push(format!(
            "{context}: target.market_slug must use only lowercase ASCII letters, digits, and hyphens (got `{market_slug}`)"
        ));
    }
}

fn target_runtime_string<T>(value: T) -> Result<String, InstrumentFilterError>
where
    T: serde::Serialize,
{
    toml::Value::try_from(value)
        .ok()
        .as_ref()
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| InstrumentFilterError::Other {
            message: TARGET_ENUM_SERIALIZE_FAILURE_MESSAGE.to_string(),
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedStaticBinaryEventMarket {
    market_id: String,
    instrument_id: InstrumentId,
    up_instrument_id: InstrumentId,
    down_instrument_id: InstrumentId,
    selection_outcome: MarketSelectionOutcome,
    start_timestamp_milliseconds: u64,
    expiration_timestamp_milliseconds: u64,
    seconds_to_end: u64,
    source_identity: SelectedMarketSourceIdentity,
}

fn select_static_market_from_instruments(
    target: StaticBinaryEventSelectionTarget<'_>,
    instruments: &[InstrumentAny],
    now_milliseconds: u64,
) -> Option<SelectedStaticBinaryEventMarket> {
    if target.yes_outcome == target.no_outcome {
        log::error!(
            "bolt-v3 static_binary_event selection: yes_outcome and no_outcome are identical for market_slug `{}`; selecting no market",
            target.market_slug
        );
        return None;
    }

    let mut pair = StaticOutcomePair::empty();
    for instrument in instruments {
        let Some(outcome) = static_outcome_instrument(instrument, target) else {
            continue;
        };
        match outcome.side {
            OutcomeSide::Up if pair.yes.is_none() => pair.yes = Some(outcome),
            OutcomeSide::Down if pair.no.is_none() => pair.no = Some(outcome),
            _ => return None,
        }
    }

    let yes = pair.yes?;
    let no = pair.no?;
    if yes.market_id != no.market_id
        || yes.condition_id != no.condition_id
        || yes.market_slug != no.market_slug
        || yes.question_id != no.question_id
    {
        return None;
    }

    let expiration_milliseconds = yes.expiration_milliseconds.min(no.expiration_milliseconds);
    if expiration_milliseconds <= now_milliseconds {
        return None;
    }
    let start_timestamp_milliseconds = yes.activation_milliseconds.max(no.activation_milliseconds);

    Some(SelectedStaticBinaryEventMarket {
        market_id: yes.market_id,
        source_identity: SelectedMarketSourceIdentity {
            condition_id: yes.condition_id,
            market_slug: yes.market_slug,
            question_id: yes.question_id,
        },
        instrument_id: yes.instrument_id,
        up_instrument_id: yes.instrument_id,
        down_instrument_id: no.instrument_id,
        selection_outcome: MarketSelectionOutcome::Current,
        start_timestamp_milliseconds,
        expiration_timestamp_milliseconds: expiration_milliseconds,
        seconds_to_end: Duration::from_millis(
            expiration_milliseconds.saturating_sub(now_milliseconds),
        )
        .as_secs(),
    })
}

fn static_outcome_instrument(
    instrument: &InstrumentAny,
    target: StaticBinaryEventSelectionTarget<'_>,
) -> Option<StaticOutcomeInstrument> {
    let InstrumentAny::BinaryOption(binary) = instrument else {
        return None;
    };
    let info = binary.info.as_ref()?;
    if info.get_str(METADATA_MARKET_SLUG_FIELD)? != target.market_slug {
        return None;
    }
    if let Some(condition_id) = target.condition_id
        && info.get_str(METADATA_CONDITION_ID_FIELD)? != condition_id
    {
        return None;
    }
    let outcome_label = binary.outcome.as_ref()?.as_str();
    let side = if outcome_label == target.yes_outcome {
        OutcomeSide::Up
    } else if outcome_label == target.no_outcome {
        OutcomeSide::Down
    } else {
        return None;
    };
    Some(StaticOutcomeInstrument {
        side,
        market_id: info.get_str(METADATA_MARKET_ID_FIELD)?.to_string(),
        condition_id: info.get_str(METADATA_CONDITION_ID_FIELD)?.to_string(),
        market_slug: info.get_str(METADATA_MARKET_SLUG_FIELD)?.to_string(),
        question_id: info.get_str(METADATA_QUESTION_ID_FIELD)?.to_string(),
        instrument_id: binary.id,
        activation_milliseconds: u64::try_from(
            Duration::from_nanos(binary.activation_ns.as_u64()).as_millis(),
        )
        .ok()?,
        expiration_milliseconds: u64::try_from(
            Duration::from_nanos(binary.expiration_ns.as_u64()).as_millis(),
        )
        .ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use nautilus_core::Params;
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{InstrumentId, Symbol},
        instruments::BinaryOption,
        types::{Currency, Price, Quantity},
    };

    const TEST_EVENT_KEY: &str = "sample_event_2026";
    const TEST_MARKET_SLUG: &str = "will-sample-event-resolve-yes";
    const TEST_MARKET_ID: &str = "sample-event-yes-no";
    const TEST_CONDITION_ID: &str = "condition-sample-event";
    const TEST_QUESTION_ID: &str = "question-sample-event";
    const TEST_OTHER_MARKET_ID: &str = "other-sample-event-yes-no";
    const TEST_OTHER_CONDITION_ID: &str = "condition-other-sample-event";
    const TEST_OTHER_QUESTION_ID: &str = "question-other-sample-event";
    const TEST_YES_OUTCOME: &str = "Yes";
    const TEST_NO_OUTCOME: &str = "No";
    const NANOS_PER_MILLI_U64: u64 = 1_000_000;

    #[test]
    fn target_runtime_fields_project_configured_static_selection_metadata() {
        let target = toml::toml! {
            configured_target_id = "sample-event-yes-no"
            kind = "static_market"
            rotating_market_family = "static_binary_event"
            event_key = "sample_event_2026"
            market_slug = "will-sample-event-resolve-yes"
            condition_id = "condition-sample-event"
            yes_outcome = "Yes"
            no_outcome = "No"
            fair_probability_source = "reference_current_price"
            selection_window_secs = 1
            market_selection_rule = "configured_static"
            retry_interval_secs = 5
            blocked_after_secs = 60
        }
        .into();

        let runtime = target_runtime_fields(&target).expect("static target should project");

        assert_eq!(runtime.configured_target_id, "sample-event-yes-no");
        assert_eq!(runtime.target_kind, "static_market");
        assert_eq!(runtime.rotating_market_family, KEY);
        assert_eq!(runtime.underlying_asset, TEST_EVENT_KEY);
        assert_eq!(runtime.cadence_seconds, 1);
        assert_eq!(runtime.cadence_slug_token, TEST_MARKET_SLUG);
        assert_eq!(runtime.market_selection_rule, "configured_static");
        assert_eq!(
            runtime.static_condition_id.as_deref(),
            Some(TEST_CONDITION_ID)
        );
        assert_eq!(
            runtime.static_yes_outcome.as_deref(),
            Some(TEST_YES_OUTCOME)
        );
        assert_eq!(runtime.static_no_outcome.as_deref(), Some(TEST_NO_OUTCOME));
        assert_eq!(
            runtime.static_fair_probability_source.as_deref(),
            Some("reference_current_price")
        );
    }

    #[test]
    fn selects_configured_static_event_pair_by_slug_condition_and_outcomes() {
        let instruments = vec![
            test_binary_option(
                "WRONG-CONDITION-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                "condition-other",
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "WRONG-SLUG-NO.POLYMARKET",
                "will-other-sample-event-resolve-yes",
                TEST_OTHER_MARKET_ID,
                TEST_OTHER_CONDITION_ID,
                TEST_OTHER_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        let selected = select_binary_option_market(static_selection_target(), &instruments, 10_000)
            .expect("configured static event should select the matching yes/no pair");

        assert_eq!(selected.market_id, TEST_MARKET_ID);
        assert_eq!(
            selected.up_instrument_id,
            InstrumentId::from("SAMPLE-EVENT-YES.POLYMARKET")
        );
        assert_eq!(
            selected.down_instrument_id,
            InstrumentId::from("SAMPLE-EVENT-NO.POLYMARKET")
        );
        assert_eq!(selected.source_identity.condition_id, TEST_CONDITION_ID);
        assert_eq!(selected.source_identity.market_slug, TEST_MARKET_SLUG);
        assert_eq!(selected.source_identity.question_id, TEST_QUESTION_ID);
        assert_eq!(
            selected.selection_outcome,
            super::super::MarketSelectionOutcome::Current
        );
        assert_eq!(selected.start_timestamp_milliseconds, 1_000);
        assert_eq!(selected.expiration_timestamp_milliseconds, 30_000);
        assert_eq!(selected.seconds_to_end, 20);
    }

    #[test]
    fn rejects_static_event_when_configured_condition_id_is_absent_from_instruments() {
        let instruments = vec![
            test_binary_option(
                "OTHER-CONDITION-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                "condition-other",
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "OTHER-CONDITION-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                "condition-other",
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        assert!(
            select_binary_option_market(static_selection_target(), &instruments, 10_000).is_none(),
            "condition_id is config-owned and must disambiguate same-slug instruments"
        );
    }

    #[test]
    fn rejects_static_event_when_duplicate_configured_yes_outcomes_match() {
        let instruments = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        assert!(
            select_binary_option_market(static_selection_target(), &instruments, 10_000).is_none(),
            "duplicate matching yes outcomes must make selection ambiguous"
        );
    }

    #[test]
    fn rejects_static_event_when_duplicate_configured_no_outcomes_match() {
        let instruments = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        assert!(
            select_binary_option_market(static_selection_target(), &instruments, 10_000).is_none(),
            "duplicate matching no outcomes must make selection ambiguous"
        );
    }

    #[test]
    fn rejects_static_event_when_one_configured_outcome_side_is_missing() {
        let lone_yes = vec![test_binary_option(
            "SAMPLE-EVENT-YES.POLYMARKET",
            TEST_MARKET_SLUG,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_YES_OUTCOME,
            1_000,
            30_000,
        )];
        let lone_no = vec![test_binary_option(
            "SAMPLE-EVENT-NO.POLYMARKET",
            TEST_MARKET_SLUG,
            TEST_MARKET_ID,
            TEST_CONDITION_ID,
            TEST_QUESTION_ID,
            TEST_NO_OUTCOME,
            1_000,
            30_000,
        )];

        assert!(
            select_binary_option_market(static_selection_target(), &lone_yes, 10_000).is_none(),
            "a static event must not select without the configured no outcome"
        );
        assert!(
            select_binary_option_market(static_selection_target(), &lone_no, 10_000).is_none(),
            "a static event must not select without the configured yes outcome"
        );
    }

    #[test]
    fn rejects_static_event_when_yes_no_identity_fields_disagree() {
        let target_without_condition = MarketSelectionTarget {
            static_condition_id: None,
            ..static_selection_target()
        };
        let market_id_mismatch = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_OTHER_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];
        let condition_id_mismatch = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_OTHER_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];
        let question_id_mismatch = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_OTHER_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        assert!(
            select_binary_option_market(target_without_condition, &market_id_mismatch, 10_000)
                .is_none(),
            "yes/no instruments with different market IDs must not be paired"
        );
        assert!(
            select_binary_option_market(target_without_condition, &condition_id_mismatch, 10_000)
                .is_none(),
            "yes/no instruments with different condition IDs must not be paired"
        );
        assert!(
            select_binary_option_market(target_without_condition, &question_id_mismatch, 10_000)
                .is_none(),
            "yes/no instruments with different question IDs must not be paired"
        );
    }

    #[test]
    fn rejects_static_event_at_or_after_expiration() {
        let instruments = vec![
            test_binary_option(
                "SAMPLE-EVENT-YES.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_YES_OUTCOME,
                1_000,
                30_000,
            ),
            test_binary_option(
                "SAMPLE-EVENT-NO.POLYMARKET",
                TEST_MARKET_SLUG,
                TEST_MARKET_ID,
                TEST_CONDITION_ID,
                TEST_QUESTION_ID,
                TEST_NO_OUTCOME,
                1_000,
                30_000,
            ),
        ];

        assert!(
            select_binary_option_market(static_selection_target(), &instruments, 30_000).is_none(),
            "expiration equality must fail closed"
        );
        assert!(
            select_binary_option_market(static_selection_target(), &instruments, 30_001).is_none(),
            "past expiration must fail closed"
        );
    }

    #[test]
    fn rejects_static_event_when_runtime_outcome_labels_are_missing() {
        assert!(
            select_binary_option_market(
                MarketSelectionTarget {
                    static_yes_outcome: None,
                    ..static_selection_target()
                },
                &[],
                10_000
            )
            .is_none(),
            "runtime selection must fail closed without a yes label"
        );
        assert!(
            select_binary_option_market(
                MarketSelectionTarget {
                    static_no_outcome: None,
                    ..static_selection_target()
                },
                &[],
                10_000
            )
            .is_none(),
            "runtime selection must fail closed without a no label"
        );
    }

    #[test]
    fn fair_probability_remains_unavailable_until_reference_price_runtime_exists() {
        let inputs = FairProbabilityInputs {
            spot_price: 0.5,
            strike_price: 0.5,
            seconds_to_market_end: 60,
            realized_vol: 0.0,
            pricing_kurtosis: 0.0,
        };

        assert!(
            super::super::fair_probability_up_for_family(KEY, &inputs).is_none(),
            "static_binary_event must stay untradeable until PR730 supplies fair probability"
        );
    }

    #[test]
    fn selected_market_requirement_rejects_identical_static_outcome_instruments() {
        let mut selected = selected_market_fixture();
        selected.down_instrument_id = selected.up_instrument_id;

        let error = selected_market_requirement(&static_target_block(), &selected, 700_000)
            .expect_err("identical yes/no instruments must fail closed");

        assert!(
            error.to_string().contains("distinct yes/no outcomes"),
            "expected distinct-outcome rejection, got: {error}"
        );
    }

    #[test]
    fn selected_market_requirement_rejects_static_outcome_venue_mismatch() {
        let mut selected = selected_market_fixture();
        selected.down_instrument_id = InstrumentId::from("SAMPLE-EVENT-NO.SIM");

        let error = selected_market_requirement(&static_target_block(), &selected, 700_000)
            .expect_err("yes/no venue mismatch must fail closed");

        assert!(
            error.to_string().contains("venues must match"),
            "expected venue-mismatch rejection, got: {error}"
        );
    }

    #[test]
    fn validate_static_target_rejects_invalid_event_key_and_market_slug_shapes() {
        for (field, value, expected) in [
            ("event_key", "", "target.event_key must not be empty"),
            ("event_key", "Sample_Event_2026", "lowercase ASCII"),
            ("event_key", "sample-event-2026", "lowercase ASCII"),
            ("event_key", "sample/event/2026", "lowercase ASCII"),
            ("market_slug", "", "target.market_slug must not be empty"),
            ("market_slug", "Will-Sample-Event", "lowercase ASCII"),
            ("market_slug", "will_sample_event", "lowercase ASCII"),
            ("market_slug", "https://sample-event", "lowercase ASCII"),
        ] {
            let mut target = static_target_block();
            set_static_target_string(&mut target, field, value);

            assert_validation_error_contains(
                validate_target_block("strategy `sample-static-event`", &target),
                expected,
            );
        }

        let mut target = static_target_block();
        set_static_target_string(&mut target, "no_outcome", TEST_YES_OUTCOME);
        assert_validation_error_contains(
            validate_target_block("strategy `sample-static-event`", &target),
            "target.yes_outcome and target.no_outcome must be distinct",
        );
    }

    fn static_selection_target() -> MarketSelectionTarget<'static> {
        MarketSelectionTarget {
            family_key: KEY,
            underlying_asset: TEST_EVENT_KEY,
            cadence_seconds: 1,
            cadence_slug_token: TEST_MARKET_SLUG,
            static_condition_id: Some(TEST_CONDITION_ID),
            static_yes_outcome: Some(TEST_YES_OUTCOME),
            static_no_outcome: Some(TEST_NO_OUTCOME),
        }
    }

    fn static_target_block() -> toml::Value {
        toml::toml! {
            configured_target_id = "sample-event-yes-no"
            kind = "static_market"
            rotating_market_family = "static_binary_event"
            event_key = "sample_event_2026"
            market_slug = "will-sample-event-resolve-yes"
            condition_id = "condition-sample-event"
            yes_outcome = "Yes"
            no_outcome = "No"
            fair_probability_source = "reference_current_price"
            selection_window_secs = 1
            market_selection_rule = "configured_static"
            retry_interval_secs = 5
            blocked_after_secs = 60
        }
        .into()
    }

    fn selected_market_fixture() -> SelectedBinaryOptionMarket {
        SelectedBinaryOptionMarket {
            market_id: TEST_MARKET_ID.to_string(),
            instrument_id: InstrumentId::from("SAMPLE-EVENT-YES.POLYMARKET"),
            up_instrument_id: InstrumentId::from("SAMPLE-EVENT-YES.POLYMARKET"),
            down_instrument_id: InstrumentId::from("SAMPLE-EVENT-NO.POLYMARKET"),
            selection_outcome: MarketSelectionOutcome::Current,
            start_timestamp_milliseconds: 1_000,
            expiration_timestamp_milliseconds: 30_000,
            seconds_to_end: 20,
            source_identity: SelectedMarketSourceIdentity {
                condition_id: TEST_CONDITION_ID.to_string(),
                market_slug: TEST_MARKET_SLUG.to_string(),
                question_id: TEST_QUESTION_ID.to_string(),
            },
        }
    }

    fn set_static_target_string(target: &mut toml::Value, field: &str, value: &str) {
        target
            .as_table_mut()
            .expect("static target should be a table")
            .insert(field.to_string(), toml::Value::String(value.to_string()));
    }

    fn assert_validation_error_contains(errors: Vec<String>, expected: &str) {
        assert!(
            errors.iter().any(|error| error.contains(expected)),
            "expected validation error containing `{expected}`, got: {errors:?}"
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn test_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        condition_id: &str,
        question_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        info.insert(
            "market_slug".to_string(),
            serde_json::Value::String(market_slug.to_string()),
        );
        info.insert(
            "market_id".to_string(),
            serde_json::Value::String(market_id.to_string()),
        );
        info.insert(
            "condition_id".to_string(),
            serde_json::Value::String(condition_id.to_string()),
        );
        info.insert(
            "question_id".to_string(),
            serde_json::Value::String(question_id.to_string()),
        );
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            (expiration_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
            None,
            None,
            None,
            None,
            None,
            Some(Price::from("0.999")),
            None,
            None,
            None,
            None,
            None,
            Some(info),
            1.into(),
            1.into(),
        ))
    }
}
