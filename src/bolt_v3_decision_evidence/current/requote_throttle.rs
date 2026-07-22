use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3RequoteActionCostClass, BoltV3RequoteThrottleBlockReason, BoltV3RequoteThrottleBound,
    BoltV3RequoteThrottleEvidence,
    facts::{
        RequoteActionCostClass, RequoteThrottleBlockReason, RequoteThrottleBound,
        RequoteThrottleFact,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequoteThrottleV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    requote_throttle: RequoteThrottleV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequoteThrottleV1Wire {
    strategy_id: String,
    family_key: String,
    market_id: Option<String>,
    leg: String,
    now_ms: u64,
    observed_at_ns: u64,
    action_cost_class: RequoteActionCostClassV1,
    block_reason: RequoteThrottleBlockReasonV1,
    bound_by: RequoteThrottleBoundV1,
    submit_commands_in_window: usize,
    submit_command_cap: u64,
    submit_window_ms: u64,
    rest_cost_in_window: u64,
    rest_cap_per_minute: u64,
    rest_window_ms: u64,
    min_interval_ms: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequoteActionCostClassV1 {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequoteThrottleBlockReasonV1 {
    RequoteBudgetExhausted,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequoteThrottleBoundV1 {
    SubmitCommandWindow,
    RestCallWindow,
    MinInterval,
    WindowCap,
    OutOfOrderTs,
    Overflow,
}

pub fn encode_requote_throttle(
    evidence: &BoltV3RequoteThrottleEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_requote_throttle_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_requote_throttle_at(
    evidence: &BoltV3RequoteThrottleEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::RequoteThrottleObservation;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "requote_throttle",
        "requote-throttle identity has wrong payload member"
    );
    let line = RequoteThrottleV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        requote_throttle: RequoteThrottleV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "requote throttle")
}

impl TryFrom<&BoltV3RequoteThrottleEvidence> for RequoteThrottleV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3RequoteThrottleEvidence) -> Result<Self> {
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            family_key: required_text(&value.family_key, "family_key")?,
            market_id: value
                .market_id
                .as_deref()
                .map(|value| required_text(value, "market_id"))
                .transpose()?,
            leg: required_text(&value.leg, "leg")?,
            now_ms: positive(value.now_ms, "now_ms")?,
            observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
            action_cost_class: value.action_cost_class.into(),
            block_reason: value.block_reason.into(),
            bound_by: value.bound_by.into(),
            submit_commands_in_window: value.submit_commands_in_window,
            submit_command_cap: positive(value.submit_command_cap, "submit_command_cap")?,
            submit_window_ms: positive(value.submit_window_ms, "submit_window_ms")?,
            rest_cost_in_window: value.rest_cost_in_window,
            rest_cap_per_minute: positive(value.rest_cap_per_minute, "rest_cap_per_minute")?,
            rest_window_ms: positive(value.rest_window_ms, "rest_window_ms")?,
            min_interval_ms: positive(value.min_interval_ms, "min_interval_ms")?,
        })
    }
}

impl From<BoltV3RequoteActionCostClass> for RequoteActionCostClassV1 {
    fn from(value: BoltV3RequoteActionCostClass) -> Self {
        match value {
            BoltV3RequoteActionCostClass::FreshSubmit => Self::FreshSubmit,
            BoltV3RequoteActionCostClass::CancelResubmit => Self::CancelResubmit,
            BoltV3RequoteActionCostClass::Cancel => Self::Cancel,
        }
    }
}

impl From<BoltV3RequoteThrottleBlockReason> for RequoteThrottleBlockReasonV1 {
    fn from(value: BoltV3RequoteThrottleBlockReason) -> Self {
        match value {
            BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted => {
                Self::RequoteBudgetExhausted
            }
        }
    }
}

impl From<BoltV3RequoteThrottleBound> for RequoteThrottleBoundV1 {
    fn from(value: BoltV3RequoteThrottleBound) -> Self {
        match value {
            BoltV3RequoteThrottleBound::SubmitCommandWindow => Self::SubmitCommandWindow,
            BoltV3RequoteThrottleBound::RestCallWindow => Self::RestCallWindow,
            BoltV3RequoteThrottleBound::MinInterval => Self::MinInterval,
            BoltV3RequoteThrottleBound::WindowCap => Self::WindowCap,
            BoltV3RequoteThrottleBound::OutOfOrderTs => Self::OutOfOrderTs,
            BoltV3RequoteThrottleBound::Overflow => Self::Overflow,
        }
    }
}

pub(crate) fn decode_requote_throttle(line: &[u8]) -> Result<RequoteThrottleFact> {
    let line: RequoteThrottleV1Line =
        serde_json::from_slice(line).context("failed to decode current requote throttle")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::RequoteThrottleObservation,
        "requote_throttle",
    )?;
    let value = line.requote_throttle;
    Ok(RequoteThrottleFact {
        strategy_id: required_text(&value.strategy_id, "strategy_id")?,
        family_key: required_text(&value.family_key, "family_key")?,
        market_id: value
            .market_id
            .as_deref()
            .map(|value| required_text(value, "market_id"))
            .transpose()?,
        leg: required_text(&value.leg, "leg")?,
        now_ms: positive(value.now_ms, "now_ms")?,
        observed_at_ns: positive(value.observed_at_ns, "observed_at_ns")?,
        action_cost_class: value.action_cost_class.into(),
        block_reason: value.block_reason.into(),
        bound_by: value.bound_by.into(),
        submit_commands_in_window: value.submit_commands_in_window,
        submit_command_cap: positive(value.submit_command_cap, "submit_command_cap")?,
        submit_window_ms: positive(value.submit_window_ms, "submit_window_ms")?,
        rest_cost_in_window: value.rest_cost_in_window,
        rest_cap_per_minute: positive(value.rest_cap_per_minute, "rest_cap_per_minute")?,
        rest_window_ms: positive(value.rest_window_ms, "rest_window_ms")?,
        min_interval_ms: positive(value.min_interval_ms, "min_interval_ms")?,
    })
}

impl From<RequoteActionCostClassV1> for RequoteActionCostClass {
    fn from(value: RequoteActionCostClassV1) -> Self {
        match value {
            RequoteActionCostClassV1::FreshSubmit => Self::FreshSubmit,
            RequoteActionCostClassV1::CancelResubmit => Self::CancelResubmit,
            RequoteActionCostClassV1::Cancel => Self::Cancel,
        }
    }
}

impl From<RequoteThrottleBlockReasonV1> for RequoteThrottleBlockReason {
    fn from(value: RequoteThrottleBlockReasonV1) -> Self {
        match value {
            RequoteThrottleBlockReasonV1::RequoteBudgetExhausted => Self::RequoteBudgetExhausted,
        }
    }
}

impl From<RequoteThrottleBoundV1> for RequoteThrottleBound {
    fn from(value: RequoteThrottleBoundV1) -> Self {
        match value {
            RequoteThrottleBoundV1::SubmitCommandWindow => Self::SubmitCommandWindow,
            RequoteThrottleBoundV1::RestCallWindow => Self::RestCallWindow,
            RequoteThrottleBoundV1::MinInterval => Self::MinInterval,
            RequoteThrottleBoundV1::WindowCap => Self::WindowCap,
            RequoteThrottleBoundV1::OutOfOrderTs => Self::OutOfOrderTs,
            RequoteThrottleBoundV1::Overflow => Self::Overflow,
        }
    }
}

fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn evidence() -> BoltV3RequoteThrottleEvidence {
        BoltV3RequoteThrottleEvidence {
            strategy_id: "maker-strategy".to_string(),
            family_key: "market-one".to_string(),
            market_id: Some("market-one".to_string()),
            leg: "yes".to_string(),
            now_ms: 1_000,
            observed_at_ns: 1_000_000,
            action_cost_class: BoltV3RequoteActionCostClass::FreshSubmit,
            block_reason: BoltV3RequoteThrottleBlockReason::RequoteBudgetExhausted,
            bound_by: BoltV3RequoteThrottleBound::SubmitCommandWindow,
            submit_commands_in_window: 40,
            submit_command_cap: 40,
            submit_window_ms: 60_000,
            rest_cost_in_window: 99,
            rest_cap_per_minute: 100,
            rest_window_ms: 60_000,
            min_interval_ms: 500,
        }
    }

    #[test]
    fn current_requote_throttle_codec_is_byte_exact_and_decodable() {
        let encoded = encode_requote_throttle_at(&evidence(), 7)
            .expect("valid requote throttle should encode");
        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/requote_throttle_observation_v1.jsonl"
        ));
        assert_eq!(encoded.bytes(), fixture);
        let decoded = decode_registered_line(fixture).expect("fixture should decode");
        let DecodedFact::RequoteThrottle(decoded) = decoded else {
            panic!("requote-throttle fixture decoded to wrong fact");
        };
        assert_eq!(
            decoded.action_cost_class,
            RequoteActionCostClass::FreshSubmit
        );
        assert_eq!(decoded.bound_by, RequoteThrottleBound::SubmitCommandWindow);
        assert_eq!(decoded.submit_commands_in_window, 40);
    }

    #[test]
    fn current_requote_throttle_codec_rejects_invalid_input_and_unknown_fields() {
        let mut invalid = evidence();
        invalid.min_interval_ms = 0;
        assert!(encode_requote_throttle(&invalid).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/requote_throttle_observation_v1.jsonl"
        ));
        let mut value: serde_json::Value =
            serde_json::from_slice(fixture).expect("fixture should parse");
        value["requote_throttle"]["extra"] = serde_json::json!(true);
        let bytes = serde_json::to_vec(&value).expect("mutated fixture should serialize");
        assert!(decode_registered_line(&bytes).is_err());
    }
}
