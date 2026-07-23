use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::Deserialize;

use super::{
    codec::{decode_current_fact, decode_startup_recovery_fact},
    facts::{
        AdmittedEntryAdmissionFact, CurrentFact, EntryOrderIntentFact, StartupRecoveryFacts,
        SubmitLinkedStrategyInputSnapshotFact,
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
pub struct RecordedCurrentFact {
    pub recorded_at_utc_ns: i64,
    pub fact: CurrentFact,
}

pub fn read_current_evidence_facts(path: &Path, max_bytes: u64) -> Result<Vec<CurrentFact>> {
    Ok(read_current_evidence_records(path, max_bytes)?
        .into_iter()
        .map(|record| record.fact)
        .collect())
}

pub fn read_current_evidence_records(
    path: &Path,
    max_bytes: u64,
) -> Result<Vec<RecordedCurrentFact>> {
    let mut file = File::open(path)
        .with_context(|| format!("open current decision evidence `{}`", path.display()))?;
    let len = file.metadata()?.len();
    let lines = read_framed_lines(&mut file, len, Some(max_bytes), "current decision evidence")?;
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
        records.push(RecordedCurrentFact {
            recorded_at_utc_ns: header.recorded_at_utc_ns,
            fact: decode_current_fact(identity, &line, line_number)?,
        });
    }
    Ok(records)
}

pub fn read_shadow_pnl_events(path: &Path) -> Result<Vec<ShadowPnlEvent>> {
    let mut file = File::open(path)
        .with_context(|| format!("open current decision evidence `{}`", path.display()))?;
    let len = file.metadata()?.len();
    let lines = read_framed_lines(&mut file, len, None, "current decision evidence")?;
    let mut events = Vec::new();
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
        if matches!(
            disposition_for(fact_id, KnownConsumer::ShadowPnlV1),
            ConsumerDisposition::Irrelevant(_)
        ) {
            continue;
        }
        let fact = decode_current_fact(identity, &line, line_number)?;
        ensure!(
            fact.registered_fact() == fact_id,
            "decoded fact disagrees with registered identity at current decision evidence line {line_number}"
        );
        events.push(into_shadow_pnl_event(fact)?);
    }
    Ok(events)
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
        | CurrentFact::ReplaceAdmission(_)
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

fn validated_header(line: &str, line_number: usize, stream: &str) -> Result<HeaderOnlyLine> {
    let header: HeaderOnlyLine = serde_json::from_str(line)
        .with_context(|| format!("malformed {stream} line {line_number}"))?;
    ensure!(
        header.recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive at {stream} line {line_number}"
    );
    ensure!(
        !header.gate_version.trim().is_empty(),
        "gate_version must be non-empty at {stream} line {line_number}"
    );
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
    pub(super) startup_recovery: StartupRecoveryFacts,
}

pub(super) fn validate_stream(
    file: &mut File,
    expected_sink: KnownSink,
    max_bytes: Option<u64>,
) -> Result<ValidatedStream> {
    let stream = match expected_sink {
        KnownSink::Machine => "machine evidence",
        KnownSink::Observation => "observation evidence",
    };
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let lines = read_framed_lines(file, len, max_bytes, stream)?;
    let mut facts = StartupRecoveryFacts::default();
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
                if let Some(fact) = decode_startup_recovery_fact(identity, &line, line_number)? {
                    facts.apply(fact)?;
                }
            }
            KnownSink::Observation => {
                decode_current_fact(identity, &line, line_number)?;
            }
        }
    }
    if expected_sink == KnownSink::Machine {
        facts.validate()?;
    }
    Ok(ValidatedStream {
        startup_recovery: facts,
    })
}

fn sink_name(sink: KnownSink) -> &'static str {
    match sink {
        KnownSink::Machine => "machine",
        KnownSink::Observation => "observation",
    }
}

fn read_framed_lines(
    reader: &mut impl Read,
    len: u64,
    max_bytes: Option<u64>,
    stream: &str,
) -> Result<Vec<String>> {
    if let Some(max_bytes) = max_bytes {
        ensure!(
            len <= max_bytes,
            "{stream} exceeds configured byte cap: {len} > {max_bytes}"
        );
    }
    let mut bytes = String::new();
    reader
        .read_to_string(&mut bytes)
        .with_context(|| format!("read {stream}"))?;
    ensure!(
        bytes.is_empty() || bytes.ends_with('\n'),
        "{stream} has a non-newline-terminated final record"
    );
    bytes
        .split_terminator('\n')
        .enumerate()
        .map(|(index, line)| {
            ensure!(!line.trim().is_empty(), "blank {stream} line {}", index + 1);
            Ok(line.to_string())
        })
        .collect()
}
