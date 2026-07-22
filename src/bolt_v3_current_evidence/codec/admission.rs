use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_current_evidence::{
    facts::{
        AdmissionDecisionOutcome, AdmissionDetails, AdmissionRejectionReason,
        AdmittedEntryAdmissionFact, CapitalAdmissionRebuildFact, CapitalAdmissionRebuildOutcome,
        CapitalAdmissionRejectionReason, ForcedReductionAdmissionFact, LossHaltReason,
        LossSnapshotSource, LossSnapshotStaleReason, RejectedEntryAdmissionFact,
        RiskReducingExitAdmissionFact,
    },
    generated_contract::{KnownIdentity, KnownPurpose},
    record::{EncodedEvidenceRecord, RecordFailure},
};

use super::{
    current_line_descriptor, decode, encode_line, validate_envelope, validate_recorded_at,
};

pub(super) fn encode_capital_rebuild(
    fact: CapitalAdmissionRebuildFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_fact(&fact)?;
    let purpose = KnownPurpose::CapitalAdmissionRebuild;
    let descriptor = current_line_descriptor(purpose);
    let decision = CapitalAdmissionRebuildV1::from_fact(fact)?;
    encode_line(
        purpose,
        &CapitalAdmissionRebuildLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision,
        },
    )
}

pub(super) fn decode_capital_rebuild(
    line: &str,
    line_number: usize,
) -> Result<CapitalAdmissionRebuildFact> {
    let decoded: CapitalAdmissionRebuildLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::CapitalAdmissionRebuildV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let fact = decoded.decision.into_fact()?;
    validate_fact(&fact).map_err(anyhow::Error::new)?;
    Ok(fact)
}

fn validate_fact(fact: &CapitalAdmissionRebuildFact) -> Result<(), RecordFailure> {
    if fact.observed_at_ns == 0
        || fact.source.trim().is_empty()
        || fact.live_reserved_liability.trim().is_empty()
        || fact.recovered_reservation_count > fact.attempted_reservation_count
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "capital admission rebuild contains invalid or inconsistent fields"
        )));
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildLineV1 {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
    decision: CapitalAdmissionRebuildV1,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapitalAdmissionRebuildV1 {
    observed_at_ns: u64,
    source: String,
    observed_open_order_count: u64,
    all_open_orders_attributed: bool,
    outcome: CapitalAdmissionRebuildOutcomeV1,
    attempted_reservation_count: u64,
    recovered_reservation_count: u64,
    live_reserved_liability: String,
}

impl CapitalAdmissionRebuildV1 {
    fn from_fact(fact: CapitalAdmissionRebuildFact) -> Result<Self, RecordFailure> {
        Ok(Self {
            observed_at_ns: fact.observed_at_ns,
            source: fact.source,
            observed_open_order_count: u64::try_from(fact.observed_open_order_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "observed_open_order_count cannot be encoded: {source}"
                    ))
                },
            )?,
            all_open_orders_attributed: fact.all_open_orders_attributed,
            outcome: CapitalAdmissionRebuildOutcomeV1::from_fact(fact.outcome),
            attempted_reservation_count: u64::try_from(fact.attempted_reservation_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "attempted_reservation_count cannot be encoded: {source}"
                    ))
                },
            )?,
            recovered_reservation_count: u64::try_from(fact.recovered_reservation_count).map_err(
                |source| {
                    RecordFailure::Rejected(anyhow::anyhow!(
                        "recovered_reservation_count cannot be encoded: {source}"
                    ))
                },
            )?,
            live_reserved_liability: fact.live_reserved_liability,
        })
    }

    fn into_fact(self) -> Result<CapitalAdmissionRebuildFact> {
        Ok(CapitalAdmissionRebuildFact {
            observed_at_ns: self.observed_at_ns,
            source: self.source,
            observed_open_order_count: usize::try_from(self.observed_open_order_count)
                .context("observed_open_order_count does not fit usize")?,
            all_open_orders_attributed: self.all_open_orders_attributed,
            outcome: self.outcome.into_fact(),
            attempted_reservation_count: usize::try_from(self.attempted_reservation_count)
                .context("attempted_reservation_count does not fit usize")?,
            recovered_reservation_count: usize::try_from(self.recovered_reservation_count)
                .context("recovered_reservation_count does not fit usize")?,
            live_reserved_liability: self.live_reserved_liability,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
enum CapitalAdmissionRebuildOutcomeV1 {
    Accepted,
    Rejected(CapitalAdmissionRejectionReasonV1),
}

impl CapitalAdmissionRebuildOutcomeV1 {
    fn from_fact(outcome: CapitalAdmissionRebuildOutcome) -> Self {
        match outcome {
            CapitalAdmissionRebuildOutcome::Accepted => Self::Accepted,
            CapitalAdmissionRebuildOutcome::Rejected(reason) => {
                Self::Rejected(CapitalAdmissionRejectionReasonV1::from_fact(reason))
            }
        }
    }

    fn into_fact(self) -> CapitalAdmissionRebuildOutcome {
        match self {
            Self::Accepted => CapitalAdmissionRebuildOutcome::Accepted,
            Self::Rejected(reason) => CapitalAdmissionRebuildOutcome::Rejected(reason.into_fact()),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CapitalAdmissionRejectionReasonV1 {
    MissingEvidence,
    StaleRequest,
    PoolMismatch,
    OverBudget,
    InvalidRequest,
    CollateralGroupMismatch,
    DuplicateReservation,
    UnknownReservation,
    UnknownRelease,
    ReconciliationRequired,
}

impl CapitalAdmissionRejectionReasonV1 {
    fn from_fact(reason: CapitalAdmissionRejectionReason) -> Self {
        match reason {
            CapitalAdmissionRejectionReason::MissingEvidence => Self::MissingEvidence,
            CapitalAdmissionRejectionReason::StaleRequest => Self::StaleRequest,
            CapitalAdmissionRejectionReason::PoolMismatch => Self::PoolMismatch,
            CapitalAdmissionRejectionReason::OverBudget => Self::OverBudget,
            CapitalAdmissionRejectionReason::InvalidRequest => Self::InvalidRequest,
            CapitalAdmissionRejectionReason::CollateralGroupMismatch => {
                Self::CollateralGroupMismatch
            }
            CapitalAdmissionRejectionReason::DuplicateReservation => Self::DuplicateReservation,
            CapitalAdmissionRejectionReason::UnknownReservation => Self::UnknownReservation,
            CapitalAdmissionRejectionReason::UnknownRelease => Self::UnknownRelease,
            CapitalAdmissionRejectionReason::ReconciliationRequired => Self::ReconciliationRequired,
        }
    }

    fn into_fact(self) -> CapitalAdmissionRejectionReason {
        match self {
            Self::MissingEvidence => CapitalAdmissionRejectionReason::MissingEvidence,
            Self::StaleRequest => CapitalAdmissionRejectionReason::StaleRequest,
            Self::PoolMismatch => CapitalAdmissionRejectionReason::PoolMismatch,
            Self::OverBudget => CapitalAdmissionRejectionReason::OverBudget,
            Self::InvalidRequest => CapitalAdmissionRejectionReason::InvalidRequest,
            Self::CollateralGroupMismatch => {
                CapitalAdmissionRejectionReason::CollateralGroupMismatch
            }
            Self::DuplicateReservation => CapitalAdmissionRejectionReason::DuplicateReservation,
            Self::UnknownReservation => CapitalAdmissionRejectionReason::UnknownReservation,
            Self::UnknownRelease => CapitalAdmissionRejectionReason::UnknownRelease,
            Self::ReconciliationRequired => CapitalAdmissionRejectionReason::ReconciliationRequired,
        }
    }
}

pub(super) fn encode_admitted_entry(
    fact: AdmittedEntryAdmissionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_admission_details(&fact.details)?;
    let purpose = KnownPurpose::AdmittedEntryAdmission;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &AdmittedEntryLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: AdmittedEntryDecisionV1::from_parts(
                fact.details,
                AdmittedEntryOutcomeV1::Admitted,
            ),
        },
    )
}

pub(super) fn decode_admitted_entry(
    line: &str,
    line_number: usize,
) -> Result<AdmittedEntryAdmissionFact> {
    let decoded: AdmittedEntryLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::AdmittedEntryAdmissionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let (details, AdmittedEntryOutcomeV1::Admitted) = decoded.decision.into_parts();
    validate_admission_details(&details).map_err(anyhow::Error::new)?;
    Ok(AdmittedEntryAdmissionFact { details })
}

pub(super) fn encode_rejected_entry(
    fact: RejectedEntryAdmissionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_admission_details(&fact.details)?;
    let purpose = KnownPurpose::RejectedEntryAdmission;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &RejectedEntryLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: RejectedEntryDecisionV1::from_parts(
                fact.details,
                RejectedEntryOutcomeV1::from_reason(fact.reason),
            ),
        },
    )
}

pub(super) fn decode_rejected_entry(
    line: &str,
    line_number: usize,
) -> Result<RejectedEntryAdmissionFact> {
    let decoded: RejectedEntryLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::RejectedEntryAdmissionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let (details, outcome) = decoded.decision.into_parts();
    validate_admission_details(&details).map_err(anyhow::Error::new)?;
    Ok(RejectedEntryAdmissionFact {
        details,
        reason: outcome.into_reason(),
    })
}

pub(super) fn encode_risk_reducing_exit(
    fact: RiskReducingExitAdmissionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_admission_details(&fact.details)?;
    let purpose = KnownPurpose::RiskReducingExitAdmission;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &RiskReducingExitLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: RiskReducingExitDecisionV1::from_parts(
                fact.details,
                RiskReducingExitOutcomeV1::from_fact(fact.outcome),
            ),
        },
    )
}

pub(super) fn decode_risk_reducing_exit(
    line: &str,
    line_number: usize,
) -> Result<RiskReducingExitAdmissionFact> {
    let decoded: RiskReducingExitLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::RiskReducingExitAdmissionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let (details, outcome) = decoded.decision.into_parts();
    validate_admission_details(&details).map_err(anyhow::Error::new)?;
    Ok(RiskReducingExitAdmissionFact {
        details,
        outcome: outcome.into_fact(),
    })
}

pub(super) fn encode_forced_reduction(
    fact: ForcedReductionAdmissionFact,
    recorded_at_utc_ns: i64,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    validate_recorded_at(recorded_at_utc_ns)?;
    validate_admission_details(&fact.details)?;
    let purpose = KnownPurpose::ForcedReductionAdmission;
    let descriptor = current_line_descriptor(purpose);
    encode_line(
        purpose,
        &ForcedReductionLineV1 {
            schema_version: descriptor.schema_version,
            recorded_at_utc_ns,
            gate_id: descriptor.gate_id.to_string(),
            gate_version: env!("CARGO_PKG_VERSION").to_string(),
            kind: descriptor.kind.to_string(),
            decision: ForcedReductionDecisionV1::from_parts(
                fact.details,
                ForcedReductionOutcomeV1::from_fact(fact.outcome),
            ),
        },
    )
}

pub(super) fn decode_forced_reduction(
    line: &str,
    line_number: usize,
) -> Result<ForcedReductionAdmissionFact> {
    let decoded: ForcedReductionLineV1 = decode(line, line_number)?;
    validate_envelope(
        KnownIdentity::ForcedReductionAdmissionV1,
        &decoded.kind,
        decoded.schema_version,
        &decoded.gate_id,
        decoded.recorded_at_utc_ns,
        line_number,
    )?;
    let (details, outcome) = decoded.decision.into_parts();
    validate_admission_details(&details).map_err(anyhow::Error::new)?;
    Ok(ForcedReductionAdmissionFact {
        details,
        outcome: outcome.into_fact(),
    })
}

fn validate_admission_details(details: &AdmissionDetails) -> Result<(), RecordFailure> {
    let timestamps = [
        details.snapshot_observed_at_ns,
        details.last_account_state_observed_at_ns,
        details.last_portfolio_snapshot_observed_at_ns,
        details.last_position_event_observed_at_ns,
        details.loss_snapshot_observed_at_ns,
        details.loss_eval_now_ns,
    ];
    let absent_snapshot_has_values = !details.snapshot_present
        && (details.snapshot_observed_at_ns.is_some()
            || details.snapshot_age_ns.is_some()
            || details.snapshot_source.is_some()
            || details.per_trade_pnl_present
            || details.daily_pnl_present
            || details.rolling_pnl_present
            || details.current_equity_present
            || details.peak_equity_present);
    if details.strategy_id.trim().is_empty()
        || details.execution_client_id.trim().is_empty()
        || details.client_order_id.trim().is_empty()
        || details.instrument_id.trim().is_empty()
        || details.notional.trim().is_empty()
        || details.admission_now_ns == 0
        || timestamps
            .into_iter()
            .flatten()
            .any(|timestamp| timestamp == 0)
        || absent_snapshot_has_values
    {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "admission decision contains an empty, invalid, or contradictory field"
        )));
    }
    Ok(())
}

macro_rules! define_admission_wire {
    ($line:ident, $decision:ident, $outcome:ident, $halt:ident, $source:ident, $stale:ident) => {
        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $line {
            schema_version: u32,
            recorded_at_utc_ns: i64,
            gate_id: String,
            gate_version: String,
            kind: String,
            decision: $decision,
        }

        #[derive(Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct $decision {
            strategy_id: String,
            execution_client_id: String,
            client_order_id: String,
            instrument_id: String,
            notional: String,
            outcome: $outcome,
            loss_halt_reasons: Vec<$halt>,
            snapshot_present: bool,
            snapshot_observed_at_ns: Option<u64>,
            admission_now_ns: u64,
            snapshot_age_ns: Option<u64>,
            max_snapshot_age_ns: Option<u64>,
            snapshot_source: Option<$source>,
            per_trade_pnl_present: bool,
            daily_pnl_present: bool,
            rolling_pnl_present: bool,
            current_equity_present: bool,
            peak_equity_present: bool,
            last_account_state_observed_at_ns: Option<u64>,
            last_portfolio_snapshot_observed_at_ns: Option<u64>,
            last_position_event_observed_at_ns: Option<u64>,
            stale_reason: Option<$stale>,
            loss_snapshot_observed_at_ns: Option<u64>,
            loss_eval_now_ns: Option<u64>,
        }

        impl $decision {
            fn from_parts(details: AdmissionDetails, outcome: $outcome) -> Self {
                Self {
                    strategy_id: details.strategy_id,
                    execution_client_id: details.execution_client_id,
                    client_order_id: details.client_order_id,
                    instrument_id: details.instrument_id,
                    notional: details.notional,
                    outcome,
                    loss_halt_reasons: details
                        .loss_halt_reasons
                        .into_iter()
                        .map($halt::from_fact)
                        .collect(),
                    snapshot_present: details.snapshot_present,
                    snapshot_observed_at_ns: details.snapshot_observed_at_ns,
                    admission_now_ns: details.admission_now_ns,
                    snapshot_age_ns: details.snapshot_age_ns,
                    max_snapshot_age_ns: details.max_snapshot_age_ns,
                    snapshot_source: details.snapshot_source.map($source::from_fact),
                    per_trade_pnl_present: details.per_trade_pnl_present,
                    daily_pnl_present: details.daily_pnl_present,
                    rolling_pnl_present: details.rolling_pnl_present,
                    current_equity_present: details.current_equity_present,
                    peak_equity_present: details.peak_equity_present,
                    last_account_state_observed_at_ns: details.last_account_state_observed_at_ns,
                    last_portfolio_snapshot_observed_at_ns: details
                        .last_portfolio_snapshot_observed_at_ns,
                    last_position_event_observed_at_ns: details.last_position_event_observed_at_ns,
                    stale_reason: details.stale_reason.map($stale::from_fact),
                    loss_snapshot_observed_at_ns: details.loss_snapshot_observed_at_ns,
                    loss_eval_now_ns: details.loss_eval_now_ns,
                }
            }

            fn into_parts(self) -> (AdmissionDetails, $outcome) {
                (
                    AdmissionDetails {
                        strategy_id: self.strategy_id,
                        execution_client_id: self.execution_client_id,
                        client_order_id: self.client_order_id,
                        instrument_id: self.instrument_id,
                        notional: self.notional,
                        loss_halt_reasons: self
                            .loss_halt_reasons
                            .into_iter()
                            .map($halt::into_fact)
                            .collect(),
                        snapshot_present: self.snapshot_present,
                        snapshot_observed_at_ns: self.snapshot_observed_at_ns,
                        admission_now_ns: self.admission_now_ns,
                        snapshot_age_ns: self.snapshot_age_ns,
                        max_snapshot_age_ns: self.max_snapshot_age_ns,
                        snapshot_source: self.snapshot_source.map($source::into_fact),
                        per_trade_pnl_present: self.per_trade_pnl_present,
                        daily_pnl_present: self.daily_pnl_present,
                        rolling_pnl_present: self.rolling_pnl_present,
                        current_equity_present: self.current_equity_present,
                        peak_equity_present: self.peak_equity_present,
                        last_account_state_observed_at_ns: self.last_account_state_observed_at_ns,
                        last_portfolio_snapshot_observed_at_ns: self
                            .last_portfolio_snapshot_observed_at_ns,
                        last_position_event_observed_at_ns: self.last_position_event_observed_at_ns,
                        stale_reason: self.stale_reason.map($stale::into_fact),
                        loss_snapshot_observed_at_ns: self.loss_snapshot_observed_at_ns,
                        loss_eval_now_ns: self.loss_eval_now_ns,
                    },
                    self.outcome,
                )
            }
        }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $halt {
            PerTradeLossLimit,
            DailyLossLimit,
            RollingLossLimit,
            MaxDrawdownLimit,
            StaleLossSnapshot,
        }

        impl $halt {
            fn from_fact(value: LossHaltReason) -> Self {
                match value {
                    LossHaltReason::PerTradeLossLimit => Self::PerTradeLossLimit,
                    LossHaltReason::DailyLossLimit => Self::DailyLossLimit,
                    LossHaltReason::RollingLossLimit => Self::RollingLossLimit,
                    LossHaltReason::MaxDrawdownLimit => Self::MaxDrawdownLimit,
                    LossHaltReason::StaleLossSnapshot => Self::StaleLossSnapshot,
                }
            }

            fn into_fact(self) -> LossHaltReason {
                match self {
                    Self::PerTradeLossLimit => LossHaltReason::PerTradeLossLimit,
                    Self::DailyLossLimit => LossHaltReason::DailyLossLimit,
                    Self::RollingLossLimit => LossHaltReason::RollingLossLimit,
                    Self::MaxDrawdownLimit => LossHaltReason::MaxDrawdownLimit,
                    Self::StaleLossSnapshot => LossHaltReason::StaleLossSnapshot,
                }
            }
        }

        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $source {
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

        impl $source {
            fn from_fact(value: LossSnapshotSource) -> Self {
                match value {
                    LossSnapshotSource::NtLossRuntimeFeed => Self::NtLossRuntimeFeed,
                    LossSnapshotSource::NtPortfolioSnapshot => Self::NtPortfolioSnapshot,
                    LossSnapshotSource::NtAccountSnapshot => Self::NtAccountSnapshot,
                    LossSnapshotSource::NtAccountAndPositionSnapshot => {
                        Self::NtAccountAndPositionSnapshot
                    }
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
                    Self::NtAccountAndPositionSnapshot => {
                        LossSnapshotSource::NtAccountAndPositionSnapshot
                    }
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
        enum $stale {
            MissingSnapshot,
            SourceEmpty,
            FutureDated,
            AgeExceeded,
            MissingRequiredField,
        }

        impl $stale {
            fn from_fact(value: LossSnapshotStaleReason) -> Self {
                match value {
                    LossSnapshotStaleReason::MissingSnapshot => Self::MissingSnapshot,
                    LossSnapshotStaleReason::SourceEmpty => Self::SourceEmpty,
                    LossSnapshotStaleReason::FutureDated => Self::FutureDated,
                    LossSnapshotStaleReason::AgeExceeded => Self::AgeExceeded,
                    LossSnapshotStaleReason::MissingRequiredField => Self::MissingRequiredField,
                }
            }

            fn into_fact(self) -> LossSnapshotStaleReason {
                match self {
                    Self::MissingSnapshot => LossSnapshotStaleReason::MissingSnapshot,
                    Self::SourceEmpty => LossSnapshotStaleReason::SourceEmpty,
                    Self::FutureDated => LossSnapshotStaleReason::FutureDated,
                    Self::AgeExceeded => LossSnapshotStaleReason::AgeExceeded,
                    Self::MissingRequiredField => LossSnapshotStaleReason::MissingRequiredField,
                }
            }
        }
    };
}

define_admission_wire!(
    AdmittedEntryLineV1,
    AdmittedEntryDecisionV1,
    AdmittedEntryOutcomeV1,
    AdmittedEntryLossHaltReasonV1,
    AdmittedEntrySnapshotSourceV1,
    AdmittedEntryStaleReasonV1
);
define_admission_wire!(
    RejectedEntryLineV1,
    RejectedEntryDecisionV1,
    RejectedEntryOutcomeV1,
    RejectedEntryLossHaltReasonV1,
    RejectedEntrySnapshotSourceV1,
    RejectedEntryStaleReasonV1
);
define_admission_wire!(
    RiskReducingExitLineV1,
    RiskReducingExitDecisionV1,
    RiskReducingExitOutcomeV1,
    RiskReducingExitLossHaltReasonV1,
    RiskReducingExitSnapshotSourceV1,
    RiskReducingExitStaleReasonV1
);
define_admission_wire!(
    ForcedReductionLineV1,
    ForcedReductionDecisionV1,
    ForcedReductionOutcomeV1,
    ForcedReductionLossHaltReasonV1,
    ForcedReductionSnapshotSourceV1,
    ForcedReductionStaleReasonV1
);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AdmittedEntryOutcomeV1 {
    Admitted,
}

macro_rules! define_rejection_outcome {
    ($name:ident) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum $name {
            KillSwitchLatched,
            SubmitLifecycleDisallowed,
            LossGovernorHalted,
            NonPositiveNotional,
            NotionalCapExceeded,
            InvalidRiskReducingExitProof,
            CountCapExhausted,
            KillSwitchForcedReductionProofInvalid,
            KillSwitchForcedReductionCapExceeded,
            CapitalAdmission,
        }

        impl $name {
            fn from_reason(reason: AdmissionRejectionReason) -> Self {
                match reason {
                    AdmissionRejectionReason::KillSwitchLatched => Self::KillSwitchLatched,
                    AdmissionRejectionReason::SubmitLifecycleDisallowed => {
                        Self::SubmitLifecycleDisallowed
                    }
                    AdmissionRejectionReason::LossGovernorHalted => Self::LossGovernorHalted,
                    AdmissionRejectionReason::NonPositiveNotional => Self::NonPositiveNotional,
                    AdmissionRejectionReason::NotionalCapExceeded => Self::NotionalCapExceeded,
                    AdmissionRejectionReason::InvalidRiskReducingExitProof => {
                        Self::InvalidRiskReducingExitProof
                    }
                    AdmissionRejectionReason::CountCapExhausted => Self::CountCapExhausted,
                    AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid => {
                        Self::KillSwitchForcedReductionProofInvalid
                    }
                    AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded => {
                        Self::KillSwitchForcedReductionCapExceeded
                    }
                    AdmissionRejectionReason::CapitalAdmission => Self::CapitalAdmission,
                }
            }

            fn into_reason(self) -> AdmissionRejectionReason {
                match self {
                    Self::KillSwitchLatched => AdmissionRejectionReason::KillSwitchLatched,
                    Self::SubmitLifecycleDisallowed => {
                        AdmissionRejectionReason::SubmitLifecycleDisallowed
                    }
                    Self::LossGovernorHalted => AdmissionRejectionReason::LossGovernorHalted,
                    Self::NonPositiveNotional => AdmissionRejectionReason::NonPositiveNotional,
                    Self::NotionalCapExceeded => AdmissionRejectionReason::NotionalCapExceeded,
                    Self::InvalidRiskReducingExitProof => {
                        AdmissionRejectionReason::InvalidRiskReducingExitProof
                    }
                    Self::CountCapExhausted => AdmissionRejectionReason::CountCapExhausted,
                    Self::KillSwitchForcedReductionProofInvalid => {
                        AdmissionRejectionReason::KillSwitchForcedReductionProofInvalid
                    }
                    Self::KillSwitchForcedReductionCapExceeded => {
                        AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded
                    }
                    Self::CapitalAdmission => AdmissionRejectionReason::CapitalAdmission,
                }
            }
        }
    };
}

define_rejection_outcome!(RejectedEntryOutcomeV1);

macro_rules! define_decision_outcome {
    ($name:ident, $rejection:ident) => {
        #[derive(Serialize, Deserialize)]
        #[serde(rename_all = "snake_case", tag = "status", content = "reason")]
        enum $name {
            Admitted,
            Rejected($rejection),
        }

        impl $name {
            fn from_fact(outcome: AdmissionDecisionOutcome) -> Self {
                match outcome {
                    AdmissionDecisionOutcome::Admitted => Self::Admitted,
                    AdmissionDecisionOutcome::Rejected(reason) => {
                        Self::Rejected($rejection::from_reason(reason))
                    }
                }
            }

            fn into_fact(self) -> AdmissionDecisionOutcome {
                match self {
                    Self::Admitted => AdmissionDecisionOutcome::Admitted,
                    Self::Rejected(reason) => {
                        AdmissionDecisionOutcome::Rejected(reason.into_reason())
                    }
                }
            }
        }
    };
}

define_rejection_outcome!(RiskReducingExitRejectionV1);
define_rejection_outcome!(ForcedReductionRejectionV1);
define_decision_outcome!(RiskReducingExitOutcomeV1, RiskReducingExitRejectionV1);
define_decision_outcome!(ForcedReductionOutcomeV1, ForcedReductionRejectionV1);
