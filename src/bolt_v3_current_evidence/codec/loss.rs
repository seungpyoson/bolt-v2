use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{LossGovernorHaltFact, LossSnapshotSource, StaleLossReason},
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode(
    fact: LossGovernorHaltFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::LossGovernorHalt;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &LineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            halt: HaltV1::from_fact(fact),
        },
    )
}

pub(super) fn decode_fact(line: &str, line_number: usize) -> Result<LossGovernorHaltFact> {
    let decoded: LineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::LossGovernorHaltV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.halt.into_fact();
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &LossGovernorHaltFact) -> Result<(), RecordFailure> {
    let timestamps = [
        fact.snapshot_observed_at_ns,
        fact.last_account_state_ts_ns,
        fact.last_portfolio_snapshot_ts_ns,
        fact.last_position_event_ts_ns,
    ];
    let absent_snapshot_has_values = !fact.snapshot_present
        && (fact.snapshot_observed_at_ns.is_some()
            || fact.snapshot_age_ns.is_some()
            || fact.snapshot_source.is_some()
            || fact.has_per_trade_pnl
            || fact.has_daily_pnl
            || fact.has_rolling_pnl
            || fact.has_current_equity
            || fact.has_peak_equity);
    if fact.admission_now_ns == 0
        || fact.max_snapshot_age_ns == 0
        || fact.stable_halt_key.trim().is_empty()
        || fact.retry_count == 0
        || !fact.retry_count.is_power_of_two()
        || timestamps
            .into_iter()
            .flatten()
            .any(|timestamp| timestamp == 0)
        || absent_snapshot_has_values
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "loss-governor halt contains an empty, invalid, or contradictory field"
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
    halt: HaltV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HaltV1 {
    snapshot_present: bool,
    snapshot_observed_at_ns: Option<u64>,
    admission_now_ns: u64,
    snapshot_age_ns: Option<u64>,
    max_snapshot_age_ns: u64,
    snapshot_source: Option<SnapshotSourceV1>,
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
    stale_reason: StaleReasonV1,
    stable_halt_key: String,
    retry_count: u32,
    elapsed_since_first_halt_ns: u64,
}

impl HaltV1 {
    fn from_fact(fact: LossGovernorHaltFact) -> Self {
        Self {
            snapshot_present: fact.snapshot_present,
            snapshot_observed_at_ns: fact.snapshot_observed_at_ns,
            admission_now_ns: fact.admission_now_ns,
            snapshot_age_ns: fact.snapshot_age_ns,
            max_snapshot_age_ns: fact.max_snapshot_age_ns,
            snapshot_source: fact.snapshot_source.map(SnapshotSourceV1::from_fact),
            has_per_trade_pnl: fact.has_per_trade_pnl,
            has_daily_pnl: fact.has_daily_pnl,
            has_rolling_pnl: fact.has_rolling_pnl,
            has_current_equity: fact.has_current_equity,
            has_peak_equity: fact.has_peak_equity,
            last_account_state_ts_ns: fact.last_account_state_ts_ns,
            last_portfolio_snapshot_ts_ns: fact.last_portfolio_snapshot_ts_ns,
            last_position_event_ts_ns: fact.last_position_event_ts_ns,
            account_state_count: fact.account_state_count,
            portfolio_snapshot_count: fact.portfolio_snapshot_count,
            position_event_count: fact.position_event_count,
            stale_reason: StaleReasonV1::from_fact(fact.stale_reason),
            stable_halt_key: fact.stable_halt_key,
            retry_count: fact.retry_count,
            elapsed_since_first_halt_ns: fact.elapsed_since_first_halt_ns,
        }
    }

    fn into_fact(self) -> LossGovernorHaltFact {
        LossGovernorHaltFact {
            snapshot_present: self.snapshot_present,
            snapshot_observed_at_ns: self.snapshot_observed_at_ns,
            admission_now_ns: self.admission_now_ns,
            snapshot_age_ns: self.snapshot_age_ns,
            max_snapshot_age_ns: self.max_snapshot_age_ns,
            snapshot_source: self.snapshot_source.map(SnapshotSourceV1::into_fact),
            has_per_trade_pnl: self.has_per_trade_pnl,
            has_daily_pnl: self.has_daily_pnl,
            has_rolling_pnl: self.has_rolling_pnl,
            has_current_equity: self.has_current_equity,
            has_peak_equity: self.has_peak_equity,
            last_account_state_ts_ns: self.last_account_state_ts_ns,
            last_portfolio_snapshot_ts_ns: self.last_portfolio_snapshot_ts_ns,
            last_position_event_ts_ns: self.last_position_event_ts_ns,
            account_state_count: self.account_state_count,
            portfolio_snapshot_count: self.portfolio_snapshot_count,
            position_event_count: self.position_event_count,
            stale_reason: self.stale_reason.into_fact(),
            stable_halt_key: self.stable_halt_key,
            retry_count: self.retry_count,
            elapsed_since_first_halt_ns: self.elapsed_since_first_halt_ns,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotSourceV1 {
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

impl SnapshotSourceV1 {
    fn from_fact(value: LossSnapshotSource) -> Self {
        match value {
            LossSnapshotSource::NtLossRuntimeFeed => Self::NtLossRuntimeFeed,
            LossSnapshotSource::NtPortfolioSnapshot => Self::NtPortfolioSnapshot,
            LossSnapshotSource::NtAccountSnapshot => Self::NtAccountSnapshot,
            LossSnapshotSource::NtAccountAndPositionSnapshot => Self::NtAccountAndPositionSnapshot,
            LossSnapshotSource::NtPositionEvent => Self::NtPositionEvent,
            LossSnapshotSource::NtPositionChanged => Self::NtPositionChanged,
            LossSnapshotSource::NtPositionClosed => Self::NtPositionClosed,
            LossSnapshotSource::NtPositionAdjusted => Self::NtPositionAdjusted,
            LossSnapshotSource::NtCapitalAdmissionState => Self::NtCapitalAdmissionState,
            LossSnapshotSource::BoltLossSnapshot => Self::BoltLossSnapshot,
            LossSnapshotSource::LossGovernor => Self::LossGovernor,
            LossSnapshotSource::Unknown => Self::Unknown,
            LossSnapshotSource::Other => Self::Other,
        }
    }

    fn into_fact(self) -> LossSnapshotSource {
        match self {
            Self::NtLossRuntimeFeed => LossSnapshotSource::NtLossRuntimeFeed,
            Self::NtPortfolioSnapshot => LossSnapshotSource::NtPortfolioSnapshot,
            Self::NtAccountSnapshot => LossSnapshotSource::NtAccountSnapshot,
            Self::NtAccountAndPositionSnapshot => LossSnapshotSource::NtAccountAndPositionSnapshot,
            Self::NtPositionEvent => LossSnapshotSource::NtPositionEvent,
            Self::NtPositionChanged => LossSnapshotSource::NtPositionChanged,
            Self::NtPositionClosed => LossSnapshotSource::NtPositionClosed,
            Self::NtPositionAdjusted => LossSnapshotSource::NtPositionAdjusted,
            Self::NtCapitalAdmissionState => LossSnapshotSource::NtCapitalAdmissionState,
            Self::BoltLossSnapshot => LossSnapshotSource::BoltLossSnapshot,
            Self::LossGovernor => LossSnapshotSource::LossGovernor,
            Self::Unknown => LossSnapshotSource::Unknown,
            Self::Other => LossSnapshotSource::Other,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StaleReasonV1 {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

impl StaleReasonV1 {
    fn from_fact(value: StaleLossReason) -> Self {
        match value {
            StaleLossReason::MissingSnapshot => Self::MissingSnapshot,
            StaleLossReason::SourceEmpty => Self::SourceEmpty,
            StaleLossReason::FutureDated => Self::FutureDated,
            StaleLossReason::AgeExceeded => Self::AgeExceeded,
            StaleLossReason::MissingRequiredField => Self::MissingRequiredField,
        }
    }

    fn into_fact(self) -> StaleLossReason {
        match self {
            Self::MissingSnapshot => StaleLossReason::MissingSnapshot,
            Self::SourceEmpty => StaleLossReason::SourceEmpty,
            Self::FutureDated => StaleLossReason::FutureDated,
            Self::AgeExceeded => StaleLossReason::AgeExceeded,
            Self::MissingRequiredField => StaleLossReason::MissingRequiredField,
        }
    }
}
