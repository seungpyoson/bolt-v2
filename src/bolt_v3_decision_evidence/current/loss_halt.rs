use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    BOLT_V3_DECISION_EVIDENCE_GATE_VERSION, EncodedEvidenceRecord, KnownPurpose, encode_record,
    identity_metadata, positive_recorded_at_utc_ns, required_text, validate_current_header,
};
use crate::bolt_v3_decision_evidence::{
    BoltV3LossGovernorHaltEvidence, BoltV3StaleLossReason,
    facts::{LossGovernorHaltFact, StaleLossReason},
    generated_contract::current_identity_for_purpose,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LossGovernorHaltV1Line {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    loss_governor_halt: LossGovernorHaltV1Wire,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LossGovernorHaltV1Wire {
    snapshot_present: bool,
    snapshot_observed_at_ns: Option<u64>,
    admission_now_ns: u64,
    snapshot_age_ns: Option<u64>,
    max_snapshot_age_ns: u64,
    snapshot_source: Option<String>,
    has_per_trade_pnl: bool,
    has_daily_pnl: bool,
    has_rolling_pnl: bool,
    has_current_equity: bool,
    has_peak_equity: bool,
    last_account_state_ts_ns: Option<u64>,
    last_portfolio_snapshot_ts_ns: Option<u64>,
    last_position_event_ts_ns: Option<u64>,
    account_state_count: u64,
    portfolio_snapshot_count: u64,
    position_event_count: u64,
    stale_reason: StaleLossReasonV1,
    stable_halt_key: String,
    retry_count: u32,
    elapsed_since_first_halt_ns: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StaleLossReasonV1 {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

pub fn encode_loss_governor_halt(
    evidence: &BoltV3LossGovernorHaltEvidence,
) -> Result<EncodedEvidenceRecord> {
    encode_loss_governor_halt_at(evidence, positive_recorded_at_utc_ns()?)
}

fn encode_loss_governor_halt_at(
    evidence: &BoltV3LossGovernorHaltEvidence,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord> {
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive"
    );
    let purpose = KnownPurpose::LossGovernorHalt;
    let identity = current_identity_for_purpose(purpose);
    let (kind, schema_version, gate_id, payload_member) = identity_metadata(identity);
    ensure!(
        payload_member == "loss_governor_halt",
        "loss-governor-halt identity has wrong payload member"
    );
    let wire = LossGovernorHaltV1Wire::try_from(evidence)?;
    validate_snapshot_shape(&wire)?;
    encode_record(
        &LossGovernorHaltV1Line {
            schema_version,
            recorded_at_utc_ns,
            gate_id: gate_id.to_string(),
            gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION.to_string(),
            kind: kind.to_string(),
            loss_governor_halt: wire,
        },
        purpose,
        "loss governor halt",
    )
}

impl TryFrom<&BoltV3LossGovernorHaltEvidence> for LossGovernorHaltV1Wire {
    type Error = anyhow::Error;

    fn try_from(value: &BoltV3LossGovernorHaltEvidence) -> Result<Self> {
        Ok(Self {
            snapshot_present: value.snapshot_present,
            snapshot_observed_at_ns: optional_positive(
                value.snapshot_observed_at_ns,
                "snapshot_observed_at_ns",
            )?,
            admission_now_ns: positive(value.admission_now_ns, "admission_now_ns")?,
            snapshot_age_ns: value.snapshot_age_ns,
            max_snapshot_age_ns: positive(value.max_snapshot_age_ns, "max_snapshot_age_ns")?,
            snapshot_source: optional_text(value.snapshot_source.as_deref(), "snapshot_source")?,
            has_per_trade_pnl: value.has_per_trade_pnl,
            has_daily_pnl: value.has_daily_pnl,
            has_rolling_pnl: value.has_rolling_pnl,
            has_current_equity: value.has_current_equity,
            has_peak_equity: value.has_peak_equity,
            last_account_state_ts_ns: optional_positive(
                value.last_account_state_ts_ns,
                "last_account_state_ts_ns",
            )?,
            last_portfolio_snapshot_ts_ns: optional_positive(
                value.last_portfolio_snapshot_ts_ns,
                "last_portfolio_snapshot_ts_ns",
            )?,
            last_position_event_ts_ns: optional_positive(
                value.last_position_event_ts_ns,
                "last_position_event_ts_ns",
            )?,
            account_state_count: value.account_state_count,
            portfolio_snapshot_count: value.portfolio_snapshot_count,
            position_event_count: value.position_event_count,
            stale_reason: value.stale_reason.into(),
            stable_halt_key: required_text(&value.stable_halt_key, "stable_halt_key")?,
            retry_count: value.retry_count,
            elapsed_since_first_halt_ns: value.elapsed_since_first_halt_ns,
        })
    }
}

impl From<BoltV3StaleLossReason> for StaleLossReasonV1 {
    fn from(value: BoltV3StaleLossReason) -> Self {
        match value {
            BoltV3StaleLossReason::MissingSnapshot => Self::MissingSnapshot,
            BoltV3StaleLossReason::SourceEmpty => Self::SourceEmpty,
            BoltV3StaleLossReason::FutureDated => Self::FutureDated,
            BoltV3StaleLossReason::AgeExceeded => Self::AgeExceeded,
            BoltV3StaleLossReason::MissingRequiredField => Self::MissingRequiredField,
        }
    }
}

pub(crate) fn decode_loss_governor_halt(line: &[u8]) -> Result<LossGovernorHaltFact> {
    let line: LossGovernorHaltV1Line =
        serde_json::from_slice(line).context("failed to decode current loss governor halt")?;
    validate_current_header(
        line.schema_version,
        line.recorded_at_utc_ns,
        &line.gate_id,
        &line.gate_version,
        &line.kind,
        KnownPurpose::LossGovernorHalt,
        "loss_governor_halt",
    )?;
    validate_snapshot_shape(&line.loss_governor_halt)?;
    line.loss_governor_halt.decode()
}

impl LossGovernorHaltV1Wire {
    fn decode(self) -> Result<LossGovernorHaltFact> {
        Ok(LossGovernorHaltFact {
            snapshot_present: self.snapshot_present,
            snapshot_observed_at_ns: optional_positive(
                self.snapshot_observed_at_ns,
                "snapshot_observed_at_ns",
            )?,
            admission_now_ns: positive(self.admission_now_ns, "admission_now_ns")?,
            snapshot_age_ns: self.snapshot_age_ns,
            max_snapshot_age_ns: positive(self.max_snapshot_age_ns, "max_snapshot_age_ns")?,
            snapshot_source: optional_text(self.snapshot_source.as_deref(), "snapshot_source")?,
            has_per_trade_pnl: self.has_per_trade_pnl,
            has_daily_pnl: self.has_daily_pnl,
            has_rolling_pnl: self.has_rolling_pnl,
            has_current_equity: self.has_current_equity,
            has_peak_equity: self.has_peak_equity,
            last_account_state_ts_ns: optional_positive(
                self.last_account_state_ts_ns,
                "last_account_state_ts_ns",
            )?,
            last_portfolio_snapshot_ts_ns: optional_positive(
                self.last_portfolio_snapshot_ts_ns,
                "last_portfolio_snapshot_ts_ns",
            )?,
            last_position_event_ts_ns: optional_positive(
                self.last_position_event_ts_ns,
                "last_position_event_ts_ns",
            )?,
            account_state_count: self.account_state_count,
            portfolio_snapshot_count: self.portfolio_snapshot_count,
            position_event_count: self.position_event_count,
            stale_reason: self.stale_reason.into(),
            stable_halt_key: required_text(&self.stable_halt_key, "stable_halt_key")?,
            retry_count: self.retry_count,
            elapsed_since_first_halt_ns: self.elapsed_since_first_halt_ns,
        })
    }
}

impl From<StaleLossReasonV1> for StaleLossReason {
    fn from(value: StaleLossReasonV1) -> Self {
        match value {
            StaleLossReasonV1::MissingSnapshot => Self::MissingSnapshot,
            StaleLossReasonV1::SourceEmpty => Self::SourceEmpty,
            StaleLossReasonV1::FutureDated => Self::FutureDated,
            StaleLossReasonV1::AgeExceeded => Self::AgeExceeded,
            StaleLossReasonV1::MissingRequiredField => Self::MissingRequiredField,
        }
    }
}

fn validate_snapshot_shape(value: &LossGovernorHaltV1Wire) -> Result<()> {
    if value.snapshot_present {
        ensure!(
            value.snapshot_observed_at_ns.is_some()
                && value.snapshot_age_ns.is_some()
                && value.snapshot_source.is_some(),
            "present loss snapshot requires observed time, age, and source"
        );
        ensure!(
            !matches!(value.stale_reason, StaleLossReasonV1::MissingSnapshot),
            "present loss snapshot cannot use missing-snapshot reason"
        );
    } else {
        ensure!(
            value.snapshot_observed_at_ns.is_none()
                && value.snapshot_age_ns.is_none()
                && value.snapshot_source.is_none(),
            "missing loss snapshot cannot carry snapshot fields"
        );
        ensure!(
            matches!(value.stale_reason, StaleLossReasonV1::MissingSnapshot),
            "missing loss snapshot requires missing-snapshot reason"
        );
    }
    Ok(())
}

fn positive(value: u64, field: &str) -> Result<u64> {
    ensure!(value > 0, "`{field}` must be positive");
    Ok(value)
}
fn optional_positive(value: Option<u64>, field: &str) -> Result<Option<u64>> {
    value.map(|value| positive(value, field)).transpose()
}
fn optional_text(value: Option<&str>, field: &str) -> Result<Option<String>> {
    value.map(|value| required_text(value, field)).transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_decision_evidence::{decode::decode_registered_line, facts::DecodedFact};

    fn present() -> BoltV3LossGovernorHaltEvidence {
        BoltV3LossGovernorHaltEvidence {
            snapshot_present: true,
            snapshot_observed_at_ns: Some(1_700_000_000_000_000_000),
            admission_now_ns: 1_700_000_005_000_000_000,
            snapshot_age_ns: Some(5_000_000_000),
            max_snapshot_age_ns: 5_000_000_000,
            snapshot_source: Some("portfolio_snapshot".into()),
            has_per_trade_pnl: true,
            has_daily_pnl: true,
            has_rolling_pnl: false,
            has_current_equity: true,
            has_peak_equity: false,
            last_account_state_ts_ns: Some(1_699_999_999_000_000_000),
            last_portfolio_snapshot_ts_ns: Some(1_700_000_000_000_000_000),
            last_position_event_ts_ns: Some(1_699_999_998_000_000_000),
            account_state_count: 3,
            portfolio_snapshot_count: 1,
            position_event_count: 7,
            stale_reason: BoltV3StaleLossReason::AgeExceeded,
            stable_halt_key: "halt-key-one".into(),
            retry_count: 2,
            elapsed_since_first_halt_ns: 10_000_000_000,
        }
    }
    fn missing() -> BoltV3LossGovernorHaltEvidence {
        BoltV3LossGovernorHaltEvidence {
            snapshot_present: false,
            snapshot_observed_at_ns: None,
            admission_now_ns: 1_700_000_010_000_000_000,
            snapshot_age_ns: None,
            max_snapshot_age_ns: 5_000_000_000,
            snapshot_source: None,
            has_per_trade_pnl: false,
            has_daily_pnl: false,
            has_rolling_pnl: false,
            has_current_equity: false,
            has_peak_equity: false,
            last_account_state_ts_ns: None,
            last_portfolio_snapshot_ts_ns: None,
            last_position_event_ts_ns: None,
            account_state_count: 0,
            portfolio_snapshot_count: 0,
            position_event_count: 0,
            stale_reason: BoltV3StaleLossReason::MissingSnapshot,
            stable_halt_key: "halt-key-two".into(),
            retry_count: 0,
            elapsed_since_first_halt_ns: 0,
        }
    }

    #[test]
    fn current_loss_halt_codec_freezes_present_and_absent_states() {
        for (evidence, recorded_at, fixture) in [
            (
                present(),
                12,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/loss_governor_halt_present_v1.jsonl"
                ))
                .as_slice(),
            ),
            (
                missing(),
                13,
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/loss_governor_halt_missing_v1.jsonl"
                ))
                .as_slice(),
            ),
        ] {
            let encoded = encode_loss_governor_halt_at(&evidence, recorded_at).unwrap();
            assert_eq!(encoded.bytes(), fixture);
            assert!(matches!(
                decode_registered_line(fixture).unwrap(),
                DecodedFact::LossGovernorHalt(_)
            ));
        }
    }

    #[test]
    fn current_loss_halt_codec_rejects_contradictory_snapshot_state() {
        let mut invalid = missing();
        invalid.snapshot_source = Some("impossible".into());
        assert!(encode_loss_governor_halt(&invalid).is_err());

        let fixture = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/decision_evidence_contract/positive/loss_governor_halt_missing_v1.jsonl"
        ));
        let mut value: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        value["loss_governor_halt"]["snapshot_age_ns"] = serde_json::json!(1);
        assert!(decode_registered_line(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}
