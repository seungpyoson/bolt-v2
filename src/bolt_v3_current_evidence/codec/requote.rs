use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        RequoteActionCostClass, RequoteThrottleBlockReason, RequoteThrottleBound,
        RequoteThrottleObservationFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode(
    fact: RequoteThrottleObservationFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::RequoteThrottleObservation;
    let descriptor = current_line_descriptor(purpose);
    let observation = ObservationV1::from_fact(fact)?;
    encode_line(
        purpose,
        &LineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            observation,
        },
    )
}

pub(super) fn decode_fact(
    line: &str,
    line_number: usize,
) -> Result<RequoteThrottleObservationFact> {
    let decoded: LineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::RequoteThrottleObservationV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.observation.into_fact()?;
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &RequoteThrottleObservationFact) -> Result<(), RecordFailure> {
    if fact.strategy_id.trim().is_empty()
        || fact.family_key.trim().is_empty()
        || fact.leg.trim().is_empty()
        || fact
            .market_id
            .as_ref()
            .is_some_and(|id| id.trim().is_empty())
        || fact.now_ms == 0
        || fact.observed_at_ns == 0
        || fact.submit_command_cap == 0
        || fact.submit_window_ms == 0
        || fact.rest_cap_per_minute == 0
        || fact.rest_window_ms == 0
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "requote throttle observation contains an empty or invalid field"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    observation: ObservationV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationV1 {
    strategy_id: String,
    family_key: String,
    market_id: Option<String>,
    leg: String,
    now_ms: u64,
    observed_at_ns: u64,
    action_cost_class: ActionCostClassV1,
    block_reason: BlockReasonV1,
    bound_by: BoundV1,
    submit_commands_in_window: u64,
    submit_command_cap: u64,
    submit_window_ms: u64,
    rest_cost_in_window: u64,
    rest_cap_per_minute: u64,
    rest_window_ms: u64,
    min_interval_ms: u64,
}

impl ObservationV1 {
    fn from_fact(fact: RequoteThrottleObservationFact) -> Result<Self, RecordFailure> {
        Ok(Self {
            strategy_id: fact.strategy_id,
            family_key: fact.family_key,
            market_id: fact.market_id,
            leg: fact.leg,
            now_ms: fact.now_ms,
            observed_at_ns: fact.observed_at_ns,
            action_cost_class: ActionCostClassV1::from_fact(fact.action_cost_class),
            block_reason: BlockReasonV1::from_fact(fact.block_reason),
            bound_by: BoundV1::from_fact(fact.bound_by),
            submit_commands_in_window: u64::try_from(fact.submit_commands_in_window).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "submit_commands_in_window cannot be encoded: {source}"
                    ))
                },
            )?,
            submit_command_cap: fact.submit_command_cap,
            submit_window_ms: fact.submit_window_ms,
            rest_cost_in_window: fact.rest_cost_in_window,
            rest_cap_per_minute: fact.rest_cap_per_minute,
            rest_window_ms: fact.rest_window_ms,
            min_interval_ms: fact.min_interval_ms,
        })
    }

    fn into_fact(self) -> Result<RequoteThrottleObservationFact> {
        Ok(RequoteThrottleObservationFact {
            strategy_id: self.strategy_id,
            family_key: self.family_key,
            market_id: self.market_id,
            leg: self.leg,
            now_ms: self.now_ms,
            observed_at_ns: self.observed_at_ns,
            action_cost_class: self.action_cost_class.into_fact(),
            block_reason: self.block_reason.into_fact(),
            bound_by: self.bound_by.into_fact(),
            submit_commands_in_window: usize::try_from(self.submit_commands_in_window)
                .context("submit_commands_in_window does not fit usize")?,
            submit_command_cap: self.submit_command_cap,
            submit_window_ms: self.submit_window_ms,
            rest_cost_in_window: self.rest_cost_in_window,
            rest_cap_per_minute: self.rest_cap_per_minute,
            rest_window_ms: self.rest_window_ms,
            min_interval_ms: self.min_interval_ms,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BlockReasonV1 {
    RequoteBudgetExhausted,
}

impl BlockReasonV1 {
    fn from_fact(value: RequoteThrottleBlockReason) -> Self {
        match value {
            RequoteThrottleBlockReason::RequoteBudgetExhausted => Self::RequoteBudgetExhausted,
        }
    }

    fn into_fact(self) -> RequoteThrottleBlockReason {
        match self {
            Self::RequoteBudgetExhausted => RequoteThrottleBlockReason::RequoteBudgetExhausted,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ActionCostClassV1 {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

impl ActionCostClassV1 {
    fn from_fact(value: RequoteActionCostClass) -> Self {
        match value {
            RequoteActionCostClass::FreshSubmit => Self::FreshSubmit,
            RequoteActionCostClass::CancelResubmit => Self::CancelResubmit,
            RequoteActionCostClass::Cancel => Self::Cancel,
        }
    }

    fn into_fact(self) -> RequoteActionCostClass {
        match self {
            Self::FreshSubmit => RequoteActionCostClass::FreshSubmit,
            Self::CancelResubmit => RequoteActionCostClass::CancelResubmit,
            Self::Cancel => RequoteActionCostClass::Cancel,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BoundV1 {
    SubmitCommandWindow,
    RestCallWindow,
    MinInterval,
    WindowCap,
    OutOfOrderTs,
    Overflow,
}

impl BoundV1 {
    fn from_fact(value: RequoteThrottleBound) -> Self {
        match value {
            RequoteThrottleBound::SubmitCommandWindow => Self::SubmitCommandWindow,
            RequoteThrottleBound::RestCallWindow => Self::RestCallWindow,
            RequoteThrottleBound::MinInterval => Self::MinInterval,
            RequoteThrottleBound::WindowCap => Self::WindowCap,
            RequoteThrottleBound::OutOfOrderTs => Self::OutOfOrderTs,
            RequoteThrottleBound::Overflow => Self::Overflow,
        }
    }

    fn into_fact(self) -> RequoteThrottleBound {
        match self {
            Self::SubmitCommandWindow => RequoteThrottleBound::SubmitCommandWindow,
            Self::RestCallWindow => RequoteThrottleBound::RestCallWindow,
            Self::MinInterval => RequoteThrottleBound::MinInterval,
            Self::WindowCap => RequoteThrottleBound::WindowCap,
            Self::OutOfOrderTs => RequoteThrottleBound::OutOfOrderTs,
            Self::Overflow => RequoteThrottleBound::Overflow,
        }
    }
}
