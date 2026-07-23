use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;

use super::{
    codec::{decode_current_fact, validate_gate_version},
    facts::{
        AdmittedEntryAdmissionFact, BlockedStrategyInputObservationFact, BookingRecoveryEvent,
        CurrentFact, EntryOrderIntentFact, EntrySkipFact, ExitHoldDecisionFact,
        ExitSubmissionDecisionFact, ForcedReductionAdmissionFact, LossGovernorHaltFact,
        RejectedEntryAdmissionFact, RequoteThrottleObservationFact, ReservationRecoveryEvent,
        RiskReducingExitAdmissionFact, SettlementRecoveryEvent, StartupRecoveryProjections,
        SubmitLinkedStrategyInputSnapshotFact, SubmitReservationFillFact,
        SubmitReservationMetadataFact,
    },
    generated_contract::{
        ConsumerDisposition, KnownConsumer, KnownSink, descriptor_for_identity, disposition_for,
        fact_for_identity, purpose_for_identity, resolve_identity, sink_for_purpose,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShadowPnlEvent {
    SubmitLinkedStrategyInputSnapshot(Box<SubmitLinkedStrategyInputSnapshotFact>),
    EntryOrderIntent(EntryOrderIntentFact),
    AdmittedEntryAdmission(AdmittedEntryAdmissionFact),
}

#[derive(Debug, Clone)]
struct RecordedCurrentFact {
    recorded_at_utc_ns: i64,
    fact: CurrentFact,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BacktestRunGuardEvent {
    BlockedStrategyInputObservation(Box<BlockedStrategyInputObservationFact>),
    SubmitLinkedStrategyInputSnapshot(Box<SubmitLinkedStrategyInputSnapshotFact>),
    EntryOrderIntent(EntryOrderIntentFact),
    AdmittedEntryAdmission(Box<AdmittedEntryAdmissionFact>),
    RejectedEntryAdmission(Box<RejectedEntryAdmissionFact>),
    RiskReducingExitAdmission(Box<RiskReducingExitAdmissionFact>),
    ForcedReductionAdmission(Box<ForcedReductionAdmissionFact>),
    SubmitReservationMetadata(SubmitReservationMetadataFact),
    SubmitReservationFill(SubmitReservationFillFact),
    EntrySkipObservation(Box<EntrySkipFact>),
    ExitSubmissionDecision(Box<ExitSubmissionDecisionFact>),
    ExitHoldDecision(Box<ExitHoldDecisionFact>),
    LossGovernorHalt(LossGovernorHaltFact),
    RequoteThrottleObservation(RequoteThrottleObservationFact),
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedBacktestRunGuardEvent {
    pub recorded_at_utc_ns: i64,
    pub event: BacktestRunGuardEvent,
}

#[cfg(feature = "test-current-evidence-inspection")]
#[doc(hidden)]
pub fn read_current_evidence_facts(path: &Path, max_bytes: u64) -> Result<Vec<CurrentFact>> {
    Ok(read_current_evidence_records(path, max_bytes)?
        .into_iter()
        .map(|record| record.fact)
        .collect())
}

#[cfg(feature = "test-current-evidence-inspection")]
fn read_current_evidence_records(path: &Path, max_bytes: u64) -> Result<Vec<RecordedCurrentFact>> {
    read_consumer_records(path, max_bytes, None)
}

pub fn read_backtest_run_guard_events(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<RecordedBacktestRunGuardEvent>> {
    read_consumer_records(path, max_bytes, Some(KnownConsumer::BacktestRunGuardV1))?
        .into_iter()
        .map(|record| {
            Ok(RecordedBacktestRunGuardEvent {
                recorded_at_utc_ns: record.recorded_at_utc_ns,
                event: into_backtest_run_guard_event(record.fact)?,
            })
        })
        .collect()
}

fn read_consumer_records(
    path: &Path,
    max_bytes: u64,
    consumer: Option<KnownConsumer>,
) -> Result<Vec<RecordedCurrentFact>> {
    let mut file = File::open(path)
        .with_context(|| format!("open current decision evidence `{}`", path.display()))?;
    let lines = read_framed_lines(&mut file, max_bytes, "current decision evidence")?;
    let mut records = Vec::new();
    for (index, line) in lines.into_iter().enumerate() {
        let line_number = index + 1;
        let header = validated_header(&line, line_number, "current decision evidence")?;
        let identity = resolve_identity(&header.kind, header.schema_version).ok_or_else(|| {
            anyhow!(
                "unsupported exact identity at current decision evidence line {line_number}: ({}, {})",
                header.kind,
                header.schema_version
            )
        })?;
        let descriptor = descriptor_for_identity(identity);
        ensure!(
            header.gate_id == descriptor.gate_id,
            "wrong gate_id at current decision evidence line {line_number}"
        );
        let fact_id = fact_for_identity(identity);
        if consumer.is_some_and(|consumer| {
            matches!(
                disposition_for(fact_id, consumer),
                ConsumerDisposition::Irrelevant(_)
            )
        }) {
            continue;
        }
        let fact = decode_current_fact(identity, &line, line_number)?;
        ensure!(
            fact.registered_fact() == fact_id,
            "decoded fact disagrees with registered identity at current decision evidence line {line_number}"
        );
        records.push(RecordedCurrentFact {
            recorded_at_utc_ns: header.recorded_at_utc_ns,
            fact,
        });
    }
    Ok(records)
}

pub fn read_shadow_pnl_events(path: &Path, max_bytes: u64) -> Result<Vec<ShadowPnlEvent>> {
    read_consumer_records(path, max_bytes, Some(KnownConsumer::ShadowPnlV1))?
        .into_iter()
        .map(|record| into_shadow_pnl_event(record.fact))
        .collect()
}

fn into_shadow_pnl_event(fact: CurrentFact) -> Result<ShadowPnlEvent> {
    let registered_fact = fact.registered_fact();
    match fact {
        CurrentFact::SubmitLinkedStrategyInputSnapshot(value) => {
            Ok(ShadowPnlEvent::SubmitLinkedStrategyInputSnapshot(value))
        }
        CurrentFact::EntryOrderIntent(value) => Ok(ShadowPnlEvent::EntryOrderIntent(value)),
        CurrentFact::AdmittedEntryAdmission(value) => {
            Ok(ShadowPnlEvent::AdmittedEntryAdmission(*value))
        }
        CurrentFact::BlockedStrategyInputObservation(_)
        | CurrentFact::RiskReducingExitOrderIntent(_)
        | CurrentFact::RejectedEntryAdmission(_)
        | CurrentFact::RiskReducingExitAdmission(_)
        | CurrentFact::ForcedReductionAdmission(_)
        | CurrentFact::BasketAdmissionGranted(_)
        | CurrentFact::BasketAdmissionRejected(_)
        | CurrentFact::CapitalAdmissionRebuild(_)
        | CurrentFact::SubmitReservationMetadata(_)
        | CurrentFact::SubmitReservationFill(_)
        | CurrentFact::EntrySkipObservation(_)
        | CurrentFact::ExitSubmissionDecision(_)
        | CurrentFact::ExitHoldDecision(_)
        | CurrentFact::ExitEvaluation(_)
        | CurrentFact::LossGovernorHalt(_)
        | CurrentFact::OrderReject(_)
        | CurrentFact::OrderLifecycle(_)
        | CurrentFact::RequoteThrottleObservation(_)
        | CurrentFact::Settlement(_)
        | CurrentFact::TerminalSettlement(_)
        | CurrentFact::VenueTruthCaptureFailure(_)
        | CurrentFact::VenueTruthDivergence(_) => Err(anyhow!(
            "fact {registered_fact:?} is registered as relevant to Shadow PnL but has no typed reducer"
        )),
    }
}

pub(super) fn into_backtest_run_guard_event(fact: CurrentFact) -> Result<BacktestRunGuardEvent> {
    let registered_fact = fact.registered_fact();
    match fact {
        CurrentFact::BlockedStrategyInputObservation(value) => Ok(
            BacktestRunGuardEvent::BlockedStrategyInputObservation(value),
        ),
        CurrentFact::SubmitLinkedStrategyInputSnapshot(value) => Ok(
            BacktestRunGuardEvent::SubmitLinkedStrategyInputSnapshot(value),
        ),
        CurrentFact::EntryOrderIntent(value) => Ok(BacktestRunGuardEvent::EntryOrderIntent(value)),
        CurrentFact::AdmittedEntryAdmission(value) => {
            Ok(BacktestRunGuardEvent::AdmittedEntryAdmission(value))
        }
        CurrentFact::RejectedEntryAdmission(value) => {
            Ok(BacktestRunGuardEvent::RejectedEntryAdmission(value))
        }
        CurrentFact::RiskReducingExitAdmission(value) => {
            Ok(BacktestRunGuardEvent::RiskReducingExitAdmission(value))
        }
        CurrentFact::ForcedReductionAdmission(value) => {
            Ok(BacktestRunGuardEvent::ForcedReductionAdmission(value))
        }
        CurrentFact::SubmitReservationMetadata(value) => {
            Ok(BacktestRunGuardEvent::SubmitReservationMetadata(value))
        }
        CurrentFact::SubmitReservationFill(value) => {
            Ok(BacktestRunGuardEvent::SubmitReservationFill(value))
        }
        CurrentFact::EntrySkipObservation(value) => {
            Ok(BacktestRunGuardEvent::EntrySkipObservation(value))
        }
        CurrentFact::ExitSubmissionDecision(value) => {
            Ok(BacktestRunGuardEvent::ExitSubmissionDecision(value))
        }
        CurrentFact::ExitHoldDecision(value) => Ok(BacktestRunGuardEvent::ExitHoldDecision(value)),
        CurrentFact::LossGovernorHalt(value) => Ok(BacktestRunGuardEvent::LossGovernorHalt(value)),
        CurrentFact::RequoteThrottleObservation(value) => {
            Ok(BacktestRunGuardEvent::RequoteThrottleObservation(value))
        }
        CurrentFact::RiskReducingExitOrderIntent(_)
        | CurrentFact::BasketAdmissionGranted(_)
        | CurrentFact::BasketAdmissionRejected(_)
        | CurrentFact::CapitalAdmissionRebuild(_)
        | CurrentFact::ExitEvaluation(_)
        | CurrentFact::OrderReject(_)
        | CurrentFact::OrderLifecycle(_)
        | CurrentFact::Settlement(_)
        | CurrentFact::TerminalSettlement(_)
        | CurrentFact::VenueTruthCaptureFailure(_)
        | CurrentFact::VenueTruthDivergence(_) => Err(anyhow!(
            "fact {registered_fact:?} is registered as relevant to the backtest run guard but has no typed reducer"
        )),
    }
}

fn validated_header(line: &str, line_number: usize, stream: &str) -> Result<HeaderOnlyLine> {
    let header: HeaderOnlyLine = serde_json::from_str(line)
        .with_context(|| format!("malformed {stream} line {line_number}"))?;
    ensure!(
        header.recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive at {stream} line {line_number}"
    );
    validate_gate_version(&header.gate_version, line_number)?;
    Ok(header)
}

#[derive(Deserialize)]
struct HeaderOnlyLine {
    kind: String,
    schema_version: u32,
    gate_id: String,
    gate_version: String,
    recorded_at_utc_ns: i64,
}

pub(super) struct ValidatedStream {
    pub(super) startup_recovery: StartupRecoveryProjections,
}

pub(super) fn validate_stream(
    file: &mut File,
    expected_sink: KnownSink,
    max_bytes: u64,
) -> Result<ValidatedStream> {
    let stream = match expected_sink {
        KnownSink::Machine => "machine evidence",
        KnownSink::Observation => "observation evidence",
    };
    file.seek(SeekFrom::Start(0))?;
    let lines = read_framed_lines(file, max_bytes, stream)?;
    let mut recovery = StartupRecoveryProjections::default();
    for (index, line) in lines.into_iter().enumerate() {
        let line_number = index + 1;
        let header = validated_header(&line, line_number, stream)?;
        let identity = resolve_identity(&header.kind, header.schema_version).ok_or_else(|| {
            anyhow!(
                "unsupported exact identity at {stream} line {line_number}: ({}, {})",
                header.kind,
                header.schema_version
            )
        })?;
        let descriptor = descriptor_for_identity(identity);
        ensure!(
            header.gate_id == descriptor.gate_id,
            "wrong gate_id at {stream} line {line_number}"
        );
        let purpose = purpose_for_identity(identity);
        let actual_sink = sink_for_purpose(purpose);
        ensure!(
            actual_sink == expected_sink,
            "{} identity in {} stream at line {line_number}",
            sink_name(actual_sink),
            sink_name(expected_sink)
        );
        match expected_sink {
            KnownSink::Machine => {
                apply_startup_recovery_projections(identity, &line, line_number, &mut recovery)?;
            }
            KnownSink::Observation => {
                decode_current_fact(identity, &line, line_number)?;
            }
        }
    }
    if expected_sink == KnownSink::Machine {
        recovery.reservation.validate()?;
    }
    Ok(ValidatedStream {
        startup_recovery: recovery,
    })
}

pub(super) fn apply_startup_recovery_projections(
    identity: super::generated_contract::KnownIdentity,
    line: &str,
    line_number: usize,
    recovery: &mut StartupRecoveryProjections,
) -> Result<()> {
    let fact_id = fact_for_identity(identity);
    let reservation_relevant = consumer_relevant(fact_id, KnownConsumer::ReservationRecoveryV1);
    let settlement_relevant = consumer_relevant(fact_id, KnownConsumer::SettlementRecoveryV1);
    let booking_relevant = consumer_relevant(fact_id, KnownConsumer::BookingRecoveryV1);
    let fact = decode_current_fact(identity, line, line_number)?;
    ensure!(
        fact.registered_fact() == fact_id,
        "decoded fact disagrees with registered identity at machine evidence line {line_number}"
    );
    if !reservation_relevant && !settlement_relevant && !booking_relevant {
        return Ok(());
    }

    if reservation_relevant {
        let event = match &fact {
            CurrentFact::SubmitReservationMetadata(value) => {
                ReservationRecoveryEvent::Metadata(value.clone())
            }
            CurrentFact::SubmitReservationFill(value) => {
                ReservationRecoveryEvent::Fill(value.clone())
            }
            _ => {
                return Err(anyhow!(
                    "fact {fact_id:?} is registered for reservation recovery without a typed reservation event"
                ));
            }
        };
        recovery.reservation.apply(event)?;
    }
    if settlement_relevant {
        let event = match &fact {
            CurrentFact::Settlement(value) => SettlementRecoveryEvent::Settlement(value.clone()),
            CurrentFact::TerminalSettlement(value) => {
                SettlementRecoveryEvent::TerminalSettlement((**value).clone())
            }
            _ => {
                return Err(anyhow!(
                    "fact {fact_id:?} is registered for settlement recovery without a typed settlement event"
                ));
            }
        };
        recovery.settlement.apply(event)?;
    }
    if booking_relevant {
        let event = match &fact {
            CurrentFact::TerminalSettlement(value) => {
                BookingRecoveryEvent::TerminalSettlement((**value).clone())
            }
            _ => {
                return Err(anyhow!(
                    "fact {fact_id:?} is registered for booking recovery without a typed booking event"
                ));
            }
        };
        recovery.booking.apply(event);
    }
    Ok(())
}

const fn consumer_relevant(
    fact: super::generated_contract::KnownFact,
    consumer: KnownConsumer,
) -> bool {
    matches!(
        disposition_for(fact, consumer),
        ConsumerDisposition::Relevant(_)
    )
}

fn sink_name(sink: KnownSink) -> &'static str {
    match sink {
        KnownSink::Machine => "machine",
        KnownSink::Observation => "observation",
    }
}

fn read_framed_lines(reader: &mut impl Read, max_bytes: u64, stream: &str) -> Result<Vec<String>> {
    let mut bytes = String::new();
    let read_limit = max_bytes.saturating_add(1);
    reader
        .take(read_limit)
        .read_to_string(&mut bytes)
        .with_context(|| format!("read {stream}"))?;
    let consumed = bytes.len() as u64;
    ensure!(
        consumed <= max_bytes,
        "{stream} exceeds configured byte cap: {consumed} > {max_bytes}"
    );
    ensure!(
        bytes.is_empty() || bytes.ends_with('\n'),
        "{stream} has a non-newline-terminated final record"
    );
    bytes
        .split_terminator('\n')
        .enumerate()
        .map(|(index, line)| {
            ensure!(!line.trim().is_empty(), "blank {stream} line {}", index + 1);
            ensure!(
                !line.contains('\r'),
                "carriage return in {stream} line {}",
                index + 1
            );
            Ok(line.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::read_framed_lines;

    #[test]
    fn consuming_read_enforces_cap_without_trusting_metadata() {
        let mut source = Cursor::new(b"12345\n");
        let error = read_framed_lines(&mut source, 4, "growing stream")
            .expect_err("bytes consumed beyond the cap must fail");

        assert!(error.to_string().contains("exceeds configured byte cap"));
    }
}
