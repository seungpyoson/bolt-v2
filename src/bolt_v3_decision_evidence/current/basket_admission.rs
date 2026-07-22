use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3BasketAdmissionDecisionEvidence, BoltV3BasketAdmissionOutcome,
    facts::{
        BasketAdmissionFact, BasketAdmissionGrantedFact, BasketAdmissionRejectedFact,
        BasketAdmissionRejection,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasketAdmissionGrantedV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: BasketAdmissionGrantedV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasketAdmissionRejectedV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: BasketAdmissionRejectedV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasketAdmissionGrantedV1Wire {
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BasketAdmissionRejectedV1Wire {
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
    reason: BasketAdmissionRejectionV1,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BasketAdmissionRejectionV1 {
    BasketNotionalCapExceeded,
    MaxOpenBasketCapExceeded,
    StaleScannerEvidence,
    StaleSubmitRecheck,
    NonPositiveCandidateCost,
    NonPositiveEdge,
    EdgeThreshold,
    MissingGroupingProof,
    MissingSettlementRules,
    RetryBudgetExceeded,
    SubmitSlots,
}

pub fn encode_basket_admission_granted(
    evidence: &BoltV3BasketAdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_basket_admission_granted_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_basket_admission_granted_at(
    evidence: &BoltV3BasketAdmissionDecisionEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.outcome == BoltV3BasketAdmissionOutcome::Admitted,
        "granted basket-admission identity requires admitted outcome"
    );
    let purpose = KnownPurpose::BasketAdmissionGranted;
    let line = current_line_metadata(purpose, recorded_at_utc_ns)?;
    let record = BasketAdmissionGrantedV1Line {
        schema_version: line.schema_version,
        recorded_at_utc_ns,
        gate_id: line.gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: line.kind.to_string(),
        decision: BasketAdmissionGrantedV1Wire::try_from(evidence)?,
    };
    encode_record(&record, purpose, "granted basket admission")
}

pub fn encode_basket_admission_rejected(
    evidence: &BoltV3BasketAdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_basket_admission_rejected_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_basket_admission_rejected_at(
    evidence: &BoltV3BasketAdmissionDecisionEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let reason = BasketAdmissionRejectionV1::try_from(evidence.outcome.clone())?;
    let purpose = KnownPurpose::BasketAdmissionRejected;
    let line = current_line_metadata(purpose, recorded_at_utc_ns)?;
    let common = validated_common(evidence, false)?;
    let record = BasketAdmissionRejectedV1Line {
        schema_version: line.schema_version,
        recorded_at_utc_ns,
        gate_id: line.gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: line.kind.to_string(),
        decision: BasketAdmissionRejectedV1Wire {
            strategy_id: common.strategy_id,
            execution_client_id: common.execution_client_id,
            basket_id: common.basket_id,
            group_id: common.group_id,
            leg_instrument_ids: common.leg_instrument_ids,
            total_notional: common.total_notional,
            leg_order_count: common.leg_order_count,
            reason,
        },
    };
    encode_record(&record, purpose, "rejected basket admission")
}

struct CurrentLineMetadata {
    kind: &'static str,
    schema_version: u32,
    gate_id: &'static str,
}

fn current_line_metadata(
    purpose: KnownPurpose,
    recorded_at_utc_ns: i64,
) -> Result<CurrentLineMetadata> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "decision",
        "basket-admission identity has wrong payload member"
    );
    Ok(CurrentLineMetadata {
        kind,
        schema_version,
        gate_id,
    })
}

impl TryFrom<&BoltV3BasketAdmissionDecisionEvidence> for BasketAdmissionGrantedV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3BasketAdmissionDecisionEvidence) -> Result<Self> {
        let common = validated_common(value, true)?;
        Ok(Self {
            strategy_id: common.strategy_id,
            execution_client_id: common.execution_client_id,
            basket_id: common.basket_id,
            group_id: common.group_id,
            leg_instrument_ids: common.leg_instrument_ids,
            total_notional: common.total_notional,
            leg_order_count: common.leg_order_count,
        })
    }
}

struct ValidatedCommon {
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
}

fn validated_common(
    value: &BoltV3BasketAdmissionDecisionEvidence,
    require_positive_notional: bool,
) -> Result<ValidatedCommon> {
    ensure!(
        !value.leg_instrument_ids.is_empty(),
        "basket admission requires at least one leg"
    );
    ensure!(
        usize::try_from(value.leg_order_count).ok() == Some(value.leg_instrument_ids.len()),
        "leg_order_count must equal leg_instrument_ids length"
    );
    let leg_instrument_ids = validated_leg_ids(&value.leg_instrument_ids)?;
    let notional = parse_decimal(&value.total_notional, "total_notional")?;
    if require_positive_notional {
        ensure!(notional > Decimal::ZERO, "total_notional must be positive");
    }
    Ok(ValidatedCommon {
        strategy_id: required_text(&value.strategy_id, "strategy_id")?,
        execution_client_id: required_text(&value.execution_client_id, "execution_client_id")?,
        basket_id: required_text(&value.basket_id, "basket_id")?,
        group_id: required_text(&value.group_id, "group_id")?,
        leg_instrument_ids,
        total_notional: notional.normalize().to_string(),
        leg_order_count: value.leg_order_count,
    })
}

impl TryFrom<BoltV3BasketAdmissionOutcome> for BasketAdmissionRejectionV1 {
    type Error = anyhow::Error;

    fn try_from(value: BoltV3BasketAdmissionOutcome) -> Result<Self> {
        match value {
            BoltV3BasketAdmissionOutcome::Admitted => {
                anyhow::bail!("rejected basket-admission identity cannot encode admitted outcome")
            }
            BoltV3BasketAdmissionOutcome::RejectedBasketNotionalCapExceeded => {
                Ok(Self::BasketNotionalCapExceeded)
            }
            BoltV3BasketAdmissionOutcome::RejectedMaxOpenBasketCapExceeded => {
                Ok(Self::MaxOpenBasketCapExceeded)
            }
            BoltV3BasketAdmissionOutcome::RejectedStaleScannerEvidence => {
                Ok(Self::StaleScannerEvidence)
            }
            BoltV3BasketAdmissionOutcome::RejectedStaleSubmitRecheck => {
                Ok(Self::StaleSubmitRecheck)
            }
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveCandidateCost => {
                Ok(Self::NonPositiveCandidateCost)
            }
            BoltV3BasketAdmissionOutcome::RejectedNonPositiveEdge => Ok(Self::NonPositiveEdge),
            BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold => Ok(Self::EdgeThreshold),
            BoltV3BasketAdmissionOutcome::RejectedMissingGroupingProof => {
                Ok(Self::MissingGroupingProof)
            }
            BoltV3BasketAdmissionOutcome::RejectedMissingSettlementRules => {
                Ok(Self::MissingSettlementRules)
            }
            BoltV3BasketAdmissionOutcome::RejectedRetryBudgetExceeded => {
                Ok(Self::RetryBudgetExceeded)
            }
            BoltV3BasketAdmissionOutcome::RejectedSubmitSlots => Ok(Self::SubmitSlots),
        }
    }
}

impl From<BasketAdmissionRejectionV1> for BasketAdmissionRejection {
    fn from(value: BasketAdmissionRejectionV1) -> Self {
        match value {
            BasketAdmissionRejectionV1::BasketNotionalCapExceeded => {
                Self::BasketNotionalCapExceeded
            }
            BasketAdmissionRejectionV1::MaxOpenBasketCapExceeded => Self::MaxOpenBasketCapExceeded,
            BasketAdmissionRejectionV1::StaleScannerEvidence => Self::StaleScannerEvidence,
            BasketAdmissionRejectionV1::StaleSubmitRecheck => Self::StaleSubmitRecheck,
            BasketAdmissionRejectionV1::NonPositiveCandidateCost => Self::NonPositiveCandidateCost,
            BasketAdmissionRejectionV1::NonPositiveEdge => Self::NonPositiveEdge,
            BasketAdmissionRejectionV1::EdgeThreshold => Self::EdgeThreshold,
            BasketAdmissionRejectionV1::MissingGroupingProof => Self::MissingGroupingProof,
            BasketAdmissionRejectionV1::MissingSettlementRules => Self::MissingSettlementRules,
            BasketAdmissionRejectionV1::RetryBudgetExceeded => Self::RetryBudgetExceeded,
            BasketAdmissionRejectionV1::SubmitSlots => Self::SubmitSlots,
        }
    }
}

pub(crate) fn decode_basket_admission_granted(line: &[u8]) -> Result<BasketAdmissionGrantedFact> {
    let line: BasketAdmissionGrantedV1Line = serde_json::from_slice(line)
        .context("failed to decode current granted basket admission")?;
    validate_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::BasketAdmissionGranted,
    )?;
    decode_common(
        line.decision.strategy_id,
        line.decision.execution_client_id,
        line.decision.basket_id,
        line.decision.group_id,
        line.decision.leg_instrument_ids,
        line.decision.total_notional,
        line.decision.leg_order_count,
        true,
    )
}

pub(crate) fn decode_basket_admission_rejected(line: &[u8]) -> Result<BasketAdmissionRejectedFact> {
    let line: BasketAdmissionRejectedV1Line = serde_json::from_slice(line)
        .context("failed to decode current rejected basket admission")?;
    validate_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::BasketAdmissionRejected,
    )?;
    Ok(BasketAdmissionRejectedFact {
        admission: decode_common(
            line.decision.strategy_id,
            line.decision.execution_client_id,
            line.decision.basket_id,
            line.decision.group_id,
            line.decision.leg_instrument_ids,
            line.decision.total_notional,
            line.decision.leg_order_count,
            false,
        )?,
        reason: line.decision.reason.into(),
    })
}

fn validate_header(
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &str,
    gate_version: &str,
    kind: &str,
    purpose: KnownPurpose,
) -> Result<()> {
    validate_current_header(
        schema_version,
        recorded_at_utc_ns,
        gate_id,
        gate_version,
        kind,
        purpose,
        "decision",
    )
}

#[allow(clippy::too_many_arguments)]
fn decode_common(
    strategy_id: String,
    execution_client_id: String,
    basket_id: String,
    group_id: String,
    leg_instrument_ids: Vec<String>,
    total_notional: String,
    leg_order_count: u32,
    require_positive_notional: bool,
) -> Result<BasketAdmissionFact> {
    ensure!(
        !leg_instrument_ids.is_empty(),
        "basket admission requires at least one leg"
    );
    ensure!(
        usize::try_from(leg_order_count).ok() == Some(leg_instrument_ids.len()),
        "leg_order_count must equal leg_instrument_ids length"
    );
    let leg_instrument_ids = validated_leg_ids(&leg_instrument_ids)?;
    let total_notional = parse_decimal(&total_notional, "total_notional")?;
    if require_positive_notional {
        ensure!(
            total_notional > Decimal::ZERO,
            "total_notional must be positive"
        );
    }
    Ok(BasketAdmissionFact {
        strategy_id: required_text(&strategy_id, "strategy_id")?,
        execution_client_id: required_text(&execution_client_id, "execution_client_id")?,
        basket_id: required_text(&basket_id, "basket_id")?,
        group_id: required_text(&group_id, "group_id")?,
        leg_instrument_ids,
        total_notional,
        leg_order_count,
    })
}

fn validated_leg_ids(values: &[String]) -> Result<Vec<String>> {
    let values = values
        .iter()
        .map(|value| required_text(value, "leg_instrument_id"))
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        values.iter().collect::<BTreeSet<_>>().len() == values.len(),
        "leg_instrument_ids must be unique"
    );
    Ok(values)
}

fn parse_decimal(value: &str, field: &str) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "`{field}` must be canonical"
    );
    value
        .parse::<Decimal>()
        .with_context(|| format!("`{field}` must parse as decimal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};
    use serde_json::Value;

    fn decision(outcome: BoltV3BasketAdmissionOutcome) -> BoltV3BasketAdmissionDecisionEvidence {
        BoltV3BasketAdmissionDecisionEvidence {
            strategy_id: "complete-set-arbitrage".to_string(),
            execution_client_id: "client-1".to_string(),
            basket_id: "basket-1".to_string(),
            group_id: "group-1".to_string(),
            leg_instrument_ids: vec!["leg-a".to_string(), "leg-b".to_string()],
            total_notional: "12.50".to_string(),
            leg_order_count: 2,
            outcome,
        }
    }

    #[test]
    fn current_basket_admission_codecs_are_purpose_homogeneous() {
        let granted = encode_basket_admission_granted_at(
            &decision(BoltV3BasketAdmissionOutcome::Admitted),
            4,
        )
        .expect("granted basket admission should encode");
        let rejected = encode_basket_admission_rejected_at(
            &decision(BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold),
            5,
        )
        .expect("rejected basket admission should encode");
        let granted_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/basket_admission_granted_v1.jsonl"
        ));
        let rejected_fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/basket_admission_rejected_v1.jsonl"
        ));
        assert_eq!(granted.bytes(), granted_fixture);
        assert_eq!(rejected.bytes(), rejected_fixture);
        assert!(matches!(
            decode_registered_line(granted_fixture).expect("granted fixture should decode"),
            DecodedFact::BasketAdmissionGranted(_)
        ));
        assert!(matches!(
            decode_registered_line(rejected_fixture).expect("rejected fixture should decode"),
            DecodedFact::BasketAdmissionRejected(BasketAdmissionRejectedFact {
                reason: BasketAdmissionRejection::EdgeThreshold,
                ..
            })
        ));
    }

    #[test]
    fn current_basket_admission_codecs_reject_cross_purpose_or_malformed_input() {
        assert!(
            encode_basket_admission_granted(&decision(
                BoltV3BasketAdmissionOutcome::RejectedEdgeThreshold
            ))
            .is_err()
        );
        assert!(
            encode_basket_admission_rejected(&decision(BoltV3BasketAdmissionOutcome::Admitted))
                .is_err()
        );

        let mut invalid = decision(BoltV3BasketAdmissionOutcome::Admitted);
        invalid.leg_order_count = 1;
        assert!(encode_basket_admission_granted(&invalid).is_err());

        let mut duplicate = decision(BoltV3BasketAdmissionOutcome::Admitted);
        duplicate.leg_instrument_ids[1] = duplicate.leg_instrument_ids[0].clone();
        assert!(encode_basket_admission_granted(&duplicate).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/basket_admission_rejected_v1.jsonl"
        ));
        let mut value: Value =
            serde_json::from_slice(fixture).expect("rejected basket fixture should parse");
        value["decision"]["outcome"] = serde_json::json!("admitted");
        let bytes = serde_json::to_vec(&value).expect("mutated basket line should serialize");
        assert!(decode_registered_line(&bytes).is_err());
    }
}
