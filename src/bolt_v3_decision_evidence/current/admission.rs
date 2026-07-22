use std::collections::BTreeSet;

use anyhow::{Context, Result, ensure};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3AdmissionDecisionEvidence, BoltV3AdmissionOutcome, BoltV3LossHaltReason,
    BoltV3LossSnapshotSource, BoltV3LossSnapshotStaleReason, BoltV3SubmitIntentKind,
    facts::{
        AdmissionFact, AdmissionLossHaltReasonFact, AdmissionLossSnapshotSourceFact,
        AdmissionLossSnapshotStaleReasonFact, AdmissionOutcomeFact,
    },
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: AdmissionV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AdmissionV1Wire {
    strategy_id: String,
    execution_client_id: String,
    client_order_id: String,
    instrument_id: String,
    notional: String,
    outcome: AdmissionOutcomeV1,
    loss_halt_reasons: Vec<AdmissionLossHaltReasonV1>,
    snapshot_present: bool,
    snapshot_observed_at_ns: Option<u64>,
    admission_now_ns: u64,
    snapshot_age_ns: Option<u64>,
    max_snapshot_age_ns: Option<u64>,
    snapshot_source: Option<AdmissionLossSnapshotSourceV1>,
    per_trade_pnl_present: bool,
    daily_pnl_present: bool,
    rolling_pnl_present: bool,
    current_equity_present: bool,
    peak_equity_present: bool,
    last_account_state_observed_at_ns: Option<u64>,
    last_portfolio_snapshot_observed_at_ns: Option<u64>,
    last_position_event_observed_at_ns: Option<u64>,
    stale_reason: Option<AdmissionLossSnapshotStaleReasonV1>,
    loss_snapshot_observed_at_ns: Option<u64>,
    loss_eval_now_ns: Option<u64>,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionOutcomeV1 {
    Admitted,
    RejectedKillSwitchLatched,
    RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof,
    RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded,
    RejectedCapitalAdmission,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionLossHaltReasonV1 {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionLossSnapshotSourceV1 {
    NtLossRuntimeFeed,
    NtPortfolioSnapshot,
    NtAccountSnapshot,
    NtAccountAndPositionSnapshot,
    NtPositionEvent,
    NtPositionChanged,
    NtPositionClosed,
    NtPositionAdjusted,
    NtCapitalAdmissionState,
    BoltLossSnapshot,
    LossGovernor,
    Unknown,
    Other,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionLossSnapshotStaleReasonV1 {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

pub fn encode_admitted_entry_admission(
    evidence: &BoltV3AdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3SubmitIntentKind::Entry
            && evidence.outcome == BoltV3AdmissionOutcome::Admitted,
        "admitted-entry encoder requires an admitted entry decision"
    );
    encode_admission_decision_at(
        evidence,
        KnownPurpose::AdmittedEntryAdmission,
        positive_recorded_at_utc_ns()?,
    )
}

pub fn encode_rejected_entry_admission(
    evidence: &BoltV3AdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3SubmitIntentKind::Entry
            && evidence.outcome != BoltV3AdmissionOutcome::Admitted,
        "rejected-entry encoder requires a rejected entry decision"
    );
    encode_admission_decision_at(
        evidence,
        KnownPurpose::RejectedEntryAdmission,
        positive_recorded_at_utc_ns()?,
    )
}

pub fn encode_risk_reducing_exit_admission(
    evidence: &BoltV3AdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3SubmitIntentKind::RiskReducingExit,
        "risk-reducing-exit encoder requires a risk-reducing exit decision"
    );
    encode_admission_decision_at(
        evidence,
        KnownPurpose::RiskReducingExitAdmission,
        positive_recorded_at_utc_ns()?,
    )
}

pub fn encode_forced_reduction_admission(
    evidence: &BoltV3AdmissionDecisionEvidence,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        evidence.intent_kind == BoltV3SubmitIntentKind::KillSwitchForcedReduction,
        "forced-reduction encoder requires a forced-reduction decision"
    );
    encode_admission_decision_at(
        evidence,
        KnownPurpose::ForcedReductionAdmission,
        positive_recorded_at_utc_ns()?,
    )
}

fn encode_admission_decision_at(
    evidence: &BoltV3AdmissionDecisionEvidence,
    purpose: KnownPurpose,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "decision",
        "admission identity has wrong payload member"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let line = AdmissionV1Line {
        schema_version,
        recorded_at_utc_ns,
        gate_id: gate_id.to_string(),
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
        kind: kind.to_string(),
        decision: AdmissionV1Wire::try_from(evidence)?,
    };
    encode_record(&line, purpose, "admission decision")
}

pub(crate) fn decode_admitted_entry_admission(line: &[u8]) -> Result<AdmissionFact> {
    let fact = decode_admission(line, KnownPurpose::AdmittedEntryAdmission)?;
    ensure!(
        fact.outcome == AdmissionOutcomeFact::Admitted,
        "admitted-entry identity requires admitted outcome"
    );
    Ok(fact)
}

pub(crate) fn decode_rejected_entry_admission(line: &[u8]) -> Result<AdmissionFact> {
    let fact = decode_admission(line, KnownPurpose::RejectedEntryAdmission)?;
    ensure!(
        fact.outcome != AdmissionOutcomeFact::Admitted,
        "rejected-entry identity cannot contain admitted outcome"
    );
    Ok(fact)
}

pub(crate) fn decode_risk_reducing_exit_admission(line: &[u8]) -> Result<AdmissionFact> {
    decode_admission(line, KnownPurpose::RiskReducingExitAdmission)
}

pub(crate) fn decode_forced_reduction_admission(line: &[u8]) -> Result<AdmissionFact> {
    decode_admission(line, KnownPurpose::ForcedReductionAdmission)
}

fn decode_admission(line: &[u8], purpose: KnownPurpose) -> Result<AdmissionFact> {
    let decoded: AdmissionV1Line =
        serde_json::from_slice(line).context("failed to decode current admission decision")?;
    validate_current_header(
        decoded.schema_version,
        decoded.recorded_at_utc_ns,
        &decoded.gate_id,
        &decoded.gate_version,
        &decoded.kind,
        purpose,
        "decision",
    )?;
    decoded.decision.try_into()
}

impl TryFrom<&BoltV3AdmissionDecisionEvidence> for AdmissionV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3AdmissionDecisionEvidence) -> Result<Self> {
        let wire = Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            execution_client_id: required_text(&value.execution_client_id, "execution_client_id")?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            notional: canonical_decimal_text(&value.notional, "notional")?,
            outcome: value.outcome.clone().into(),
            loss_halt_reasons: value
                .loss_halt_reasons
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            snapshot_present: value.snapshot_present,
            snapshot_observed_at_ns: value.snapshot_observed_at_ns,
            admission_now_ns: value.admission_now_ns,
            snapshot_age_ns: value.snapshot_age_ns,
            max_snapshot_age_ns: value.max_snapshot_age_ns,
            snapshot_source: value.snapshot_source.map(Into::into),
            per_trade_pnl_present: value.per_trade_pnl_present,
            daily_pnl_present: value.daily_pnl_present,
            rolling_pnl_present: value.rolling_pnl_present,
            current_equity_present: value.current_equity_present,
            peak_equity_present: value.peak_equity_present,
            last_account_state_observed_at_ns: value.last_account_state_observed_at_ns,
            last_portfolio_snapshot_observed_at_ns: value.last_portfolio_snapshot_observed_at_ns,
            last_position_event_observed_at_ns: value.last_position_event_observed_at_ns,
            stale_reason: value.stale_reason.map(Into::into),
            loss_snapshot_observed_at_ns: value.loss_snapshot_observed_at_ns,
            loss_eval_now_ns: value.loss_eval_now_ns,
        };
        wire.validate()?;
        Ok(wire)
    }
}

impl AdmissionV1Wire {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.admission_now_ns > 0,
            "admission_now_ns must be positive"
        );
        for (field, value) in [
            ("snapshot_observed_at_ns", self.snapshot_observed_at_ns),
            (
                "last_account_state_observed_at_ns",
                self.last_account_state_observed_at_ns,
            ),
            (
                "last_portfolio_snapshot_observed_at_ns",
                self.last_portfolio_snapshot_observed_at_ns,
            ),
            (
                "last_position_event_observed_at_ns",
                self.last_position_event_observed_at_ns,
            ),
            (
                "loss_snapshot_observed_at_ns",
                self.loss_snapshot_observed_at_ns,
            ),
            ("loss_eval_now_ns", self.loss_eval_now_ns),
        ] {
            if let Some(value) = value {
                ensure!(value > 0, "{field} must be positive when present");
            }
        }
        ensure!(
            self.loss_halt_reasons.iter().collect::<BTreeSet<_>>().len()
                == self.loss_halt_reasons.len(),
            "loss_halt_reasons must not contain duplicates"
        );
        let loss_halted = matches!(self.outcome, AdmissionOutcomeV1::RejectedLossGovernorHalted);
        ensure!(
            loss_halted == !self.loss_halt_reasons.is_empty(),
            "loss-halt reasons must be present exactly for a loss-governor rejection"
        );
        Ok(())
    }
}

impl TryFrom<AdmissionV1Wire> for AdmissionFact {
    type Error = anyhow::Error;

    fn try_from(value: AdmissionV1Wire) -> Result<Self> {
        value.validate()?;
        Ok(Self {
            strategy_id: required_text(&value.strategy_id, "strategy_id")?,
            execution_client_id: required_text(&value.execution_client_id, "execution_client_id")?,
            client_order_id: required_text(&value.client_order_id, "client_order_id")?,
            instrument_id: required_text(&value.instrument_id, "instrument_id")?,
            notional: canonical_decimal(&value.notional, "notional")?,
            outcome: value.outcome.into(),
            loss_halt_reasons: value
                .loss_halt_reasons
                .into_iter()
                .map(Into::into)
                .collect(),
            snapshot_present: Some(value.snapshot_present),
            snapshot_observed_at_ns: value.snapshot_observed_at_ns,
            admission_now_ns: Some(value.admission_now_ns),
            snapshot_age_ns: value.snapshot_age_ns,
            max_snapshot_age_ns: value.max_snapshot_age_ns,
            snapshot_source: value.snapshot_source.map(Into::into),
            per_trade_pnl_present: Some(value.per_trade_pnl_present),
            daily_pnl_present: Some(value.daily_pnl_present),
            rolling_pnl_present: Some(value.rolling_pnl_present),
            current_equity_present: Some(value.current_equity_present),
            peak_equity_present: Some(value.peak_equity_present),
            last_account_state_observed_at_ns: value.last_account_state_observed_at_ns,
            last_portfolio_snapshot_observed_at_ns: value.last_portfolio_snapshot_observed_at_ns,
            last_position_event_observed_at_ns: value.last_position_event_observed_at_ns,
            stale_reason: value.stale_reason.map(Into::into),
            loss_snapshot_observed_at_ns: value.loss_snapshot_observed_at_ns,
            loss_eval_now_ns: value.loss_eval_now_ns,
        })
    }
}

impl From<BoltV3AdmissionOutcome> for AdmissionOutcomeV1 {
    fn from(value: BoltV3AdmissionOutcome) -> Self {
        match value {
            BoltV3AdmissionOutcome::Admitted => Self::Admitted,
            BoltV3AdmissionOutcome::RejectedKillSwitchLatched => Self::RejectedKillSwitchLatched,
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                Self::RejectedSubmitLifecycleDisallowed
            }
            BoltV3AdmissionOutcome::RejectedLossGovernorHalted => Self::RejectedLossGovernorHalted,
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                Self::RejectedNonPositiveNotional
            }
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                Self::RejectedNotionalCapExceeded
            }
            BoltV3AdmissionOutcome::RejectedInvalidRiskReducingExitProof => {
                Self::RejectedInvalidRiskReducingExitProof
            }
            BoltV3AdmissionOutcome::RejectedCountCapExhausted => Self::RejectedCountCapExhausted,
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionProofInvalid => {
                Self::RejectedKillSwitchForcedReductionProofInvalid
            }
            BoltV3AdmissionOutcome::RejectedKillSwitchForcedReductionCapExceeded => {
                Self::RejectedKillSwitchForcedReductionCapExceeded
            }
            BoltV3AdmissionOutcome::RejectedCapitalAdmission => Self::RejectedCapitalAdmission,
        }
    }
}

impl From<AdmissionOutcomeV1> for AdmissionOutcomeFact {
    fn from(value: AdmissionOutcomeV1) -> Self {
        match value {
            AdmissionOutcomeV1::Admitted => Self::Admitted,
            AdmissionOutcomeV1::RejectedKillSwitchLatched => Self::RejectedKillSwitchLatched,
            AdmissionOutcomeV1::RejectedSubmitLifecycleDisallowed => {
                Self::RejectedSubmitLifecycleDisallowed
            }
            AdmissionOutcomeV1::RejectedLossGovernorHalted => Self::RejectedLossGovernorHalted,
            AdmissionOutcomeV1::RejectedNonPositiveNotional => Self::RejectedNonPositiveNotional,
            AdmissionOutcomeV1::RejectedNotionalCapExceeded => Self::RejectedNotionalCapExceeded,
            AdmissionOutcomeV1::RejectedInvalidRiskReducingExitProof => {
                Self::RejectedInvalidRiskReducingExitProof
            }
            AdmissionOutcomeV1::RejectedCountCapExhausted => Self::RejectedCountCapExhausted,
            AdmissionOutcomeV1::RejectedKillSwitchForcedReductionProofInvalid => {
                Self::RejectedKillSwitchForcedReductionProofInvalid
            }
            AdmissionOutcomeV1::RejectedKillSwitchForcedReductionCapExceeded => {
                Self::RejectedKillSwitchForcedReductionCapExceeded
            }
            AdmissionOutcomeV1::RejectedCapitalAdmission => Self::RejectedCapitalAdmission,
        }
    }
}

macro_rules! map_enum {
    ($from:ty => $to:ty, $($variant:ident),+ $(,)?) => {
        impl From<$from> for $to {
            fn from(value: $from) -> Self {
                match value { $(<$from>::$variant => Self::$variant,)+ }
            }
        }
    };
}

map_enum!(BoltV3LossHaltReason => AdmissionLossHaltReasonV1,
    PerTradeLossLimit, DailyLossLimit, RollingLossLimit, MaxDrawdownLimit, StaleLossSnapshot);
map_enum!(AdmissionLossHaltReasonV1 => AdmissionLossHaltReasonFact,
    PerTradeLossLimit, DailyLossLimit, RollingLossLimit, MaxDrawdownLimit, StaleLossSnapshot);
map_enum!(BoltV3LossSnapshotSource => AdmissionLossSnapshotSourceV1,
    NtLossRuntimeFeed, NtPortfolioSnapshot, NtAccountSnapshot, NtAccountAndPositionSnapshot,
    NtPositionEvent, NtPositionChanged, NtPositionClosed, NtPositionAdjusted,
    NtCapitalAdmissionState, BoltLossSnapshot, LossGovernor, Unknown, Other);
map_enum!(AdmissionLossSnapshotSourceV1 => AdmissionLossSnapshotSourceFact,
    NtLossRuntimeFeed, NtPortfolioSnapshot, NtAccountSnapshot, NtAccountAndPositionSnapshot,
    NtPositionEvent, NtPositionChanged, NtPositionClosed, NtPositionAdjusted,
    NtCapitalAdmissionState, BoltLossSnapshot, LossGovernor, Unknown, Other);
map_enum!(BoltV3LossSnapshotStaleReason => AdmissionLossSnapshotStaleReasonV1,
    MissingSnapshot, SourceEmpty, FutureDated, AgeExceeded, MissingRequiredField);
map_enum!(AdmissionLossSnapshotStaleReasonV1 => AdmissionLossSnapshotStaleReasonFact,
    MissingSnapshot, SourceEmpty, FutureDated, AgeExceeded, MissingRequiredField);

fn canonical_decimal_text(value: &str, field: &str) -> Result<String> {
    Ok(canonical_decimal(value, field)?.normalize().to_string())
}

fn canonical_decimal(value: &str, field: &str) -> Result<Decimal> {
    ensure!(
        !value.is_empty() && value.trim() == value,
        "{field} must be canonical"
    );
    value
        .parse::<Decimal>()
        .with_context(|| format!("{field} must parse as decimal"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(
        intent_kind: BoltV3SubmitIntentKind,
        outcome: BoltV3AdmissionOutcome,
    ) -> BoltV3AdmissionDecisionEvidence {
        BoltV3AdmissionDecisionEvidence {
            strategy_id: "strategy".into(),
            execution_client_id: "execution".into(),
            client_order_id: "order".into(),
            instrument_id: "instrument".into(),
            notional: "10".into(),
            intent_kind,
            outcome,
            loss_halt_reasons: vec![],
            snapshot_present: false,
            snapshot_observed_at_ns: None,
            admission_now_ns: 1,
            snapshot_age_ns: None,
            max_snapshot_age_ns: None,
            snapshot_source: None,
            per_trade_pnl_present: false,
            daily_pnl_present: false,
            rolling_pnl_present: false,
            current_equity_present: false,
            peak_equity_present: false,
            last_account_state_observed_at_ns: None,
            last_portfolio_snapshot_observed_at_ns: None,
            last_position_event_observed_at_ns: None,
            stale_reason: None,
            loss_snapshot_observed_at_ns: None,
            loss_eval_now_ns: None,
        }
    }

    #[test]
    fn registered_admission_roles_use_four_disjoint_current_identities() {
        let cases = [
            (
                evidence(
                    BoltV3SubmitIntentKind::Entry,
                    BoltV3AdmissionOutcome::Admitted,
                ),
                KnownPurpose::AdmittedEntryAdmission,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/admitted_entry_admission_v1.jsonl"
                )) as &[u8],
            ),
            (
                evidence(
                    BoltV3SubmitIntentKind::Entry,
                    BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
                ),
                KnownPurpose::RejectedEntryAdmission,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/rejected_entry_admission_v1.jsonl"
                )) as &[u8],
            ),
            (
                evidence(
                    BoltV3SubmitIntentKind::RiskReducingExit,
                    BoltV3AdmissionOutcome::Admitted,
                ),
                KnownPurpose::RiskReducingExitAdmission,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/risk_reducing_exit_admission_v1.jsonl"
                )) as &[u8],
            ),
            (
                evidence(
                    BoltV3SubmitIntentKind::KillSwitchForcedReduction,
                    BoltV3AdmissionOutcome::Admitted,
                ),
                KnownPurpose::ForcedReductionAdmission,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/forced_reduction_admission_v1.jsonl"
                )) as &[u8],
            ),
        ];
        let records = cases
            .iter()
            .map(|(evidence, purpose, fixture)| {
                let record = encode_admission_decision_at(evidence, *purpose, 1).unwrap();
                assert_eq!(record.bytes(), *fixture);
                let fact = decode_admission(record.bytes(), *purpose).unwrap();
                assert_eq!(
                    fact.outcome,
                    AdmissionOutcomeFact::from(AdmissionOutcomeV1::from(evidence.outcome.clone()))
                );
                record
            })
            .collect::<Vec<_>>();
        let unique = records
            .iter()
            .map(|record| {
                let value: serde_json::Value = serde_json::from_slice(record.bytes()).unwrap();
                (
                    value["kind"].as_str().unwrap().to_string(),
                    value["schema_version"].as_u64().unwrap(),
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(unique.len(), 4);
    }

    #[test]
    fn replace_admission_has_no_current_evidence_identity() {
        let error = encode_admitted_entry_admission(&evidence(
            BoltV3SubmitIntentKind::ReplaceSubmit,
            BoltV3AdmissionOutcome::Admitted,
        ))
        .expect_err("replace admission must remain outside the current evidence contract");
        assert!(
            error
                .to_string()
                .contains("admitted-entry encoder requires an admitted entry decision")
        );
    }
}
