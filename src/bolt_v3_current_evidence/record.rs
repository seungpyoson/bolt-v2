use std::{
    collections::BTreeSet,
    fmt,
    fs::File,
    io::{self, Write},
    sync::Mutex,
};

use anyhow::Error;

use super::codec::{
    encode_reservation_fill, encode_reservation_metadata, encode_settlement,
    encode_settlement_booking_error, encode_terminal_settlement,
};
use super::facts::{
    SettlementBookingErrorFact, SettlementFact, SubmitReservationFillFact,
    SubmitReservationMetadataFact, TerminalSettlementFact,
};
use super::generated_contract::{KnownPurpose, KnownSink, sink_for_purpose};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendReceipt {
    purpose: KnownPurpose,
    sink: KnownSink,
    bytes: usize,
}

#[derive(Debug)]
pub enum RecordFailure {
    Rejected(Error),
    AppendFailed(Error),
}

impl fmt::Display for RecordFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(source) => write!(formatter, "evidence record rejected: {source}"),
            Self::AppendFailed(source) => write!(formatter, "evidence append failed: {source}"),
        }
    }
}

impl std::error::Error for RecordFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rejected(source) | Self::AppendFailed(source) => Some(source.as_ref()),
        }
    }
}

#[derive(Debug)]
pub enum NonBlockingRecordOutcome {
    Appended(AppendReceipt),
    Failed(RecordFailure),
}

#[derive(Debug)]
pub enum ObservationRecordOutcome {
    Appended(AppendReceipt),
    FailureReported(RecordFailure),
    FailureSuppressed,
}

#[derive(Debug)]
pub(crate) struct EncodedEvidenceRecord {
    purpose: KnownPurpose,
    line: Vec<u8>,
}

impl EncodedEvidenceRecord {
    pub(crate) fn try_new(purpose: KnownPurpose, line: Vec<u8>) -> Result<Self, RecordFailure> {
        if line.is_empty() || line.last() != Some(&b'\n') || line[..line.len() - 1].contains(&b'\n')
        {
            return Err(RecordFailure::Rejected(anyhow::anyhow!(
                "encoded evidence must be exactly one newline-terminated JSONL record"
            )));
        }
        Ok(Self { purpose, line })
    }
}

#[derive(Debug)]
pub(crate) struct DurableSink {
    file: File,
    #[cfg(test)]
    forced_failure: Option<ForcedFailure>,
}

impl DurableSink {
    pub(crate) fn new(file: File) -> Self {
        Self {
            file,
            #[cfg(test)]
            forced_failure: None,
        }
    }

    fn append(&mut self, line: &[u8]) -> io::Result<()> {
        #[cfg(test)]
        if self.forced_failure == Some(ForcedFailure::Write) {
            return Err(io::Error::other("injected write failure"));
        }
        self.file.write_all(line)?;
        #[cfg(test)]
        if self.forced_failure == Some(ForcedFailure::Sync) {
            return Err(io::Error::other("injected sync failure"));
        }
        self.file.sync_data()
    }
}

#[derive(Debug)]
pub struct DecisionEvidenceRecorder {
    machine: Mutex<DurableSink>,
    observation: Mutex<DurableSink>,
    observation_failure_episodes: Mutex<BTreeSet<KnownPurpose>>,
}

impl DecisionEvidenceRecorder {
    pub(crate) fn new(machine: File, observation: File) -> Self {
        Self {
            machine: Mutex::new(DurableSink::new(machine)),
            observation: Mutex::new(DurableSink::new(observation)),
            observation_failure_episodes: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn record_submit_reservation_metadata(
        &self,
        command: SubmitReservationMetadataFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(encode_reservation_metadata(command)?)
    }

    pub fn record_submit_reservation_fill(
        &self,
        command: SubmitReservationFillFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(encode_reservation_fill(command)?)
    }

    pub fn record_settlement(&self, fact: SettlementFact) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(encode_settlement(fact)?)
    }

    pub fn record_settlement_booking_error(
        &self,
        fact: SettlementBookingErrorFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(encode_settlement_booking_error(fact)?)
    }

    pub fn record_terminal_settlement(
        &self,
        fact: TerminalSettlementFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(encode_terminal_settlement(fact)?)
    }

    pub(crate) fn record_blocking(
        &self,
        record: EncodedEvidenceRecord,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.append(record)
    }

    pub(crate) fn record_nonblocking(
        &self,
        record: EncodedEvidenceRecord,
    ) -> NonBlockingRecordOutcome {
        match self.append(record) {
            Ok(receipt) => NonBlockingRecordOutcome::Appended(receipt),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub(crate) fn record_observation(
        &self,
        record: EncodedEvidenceRecord,
    ) -> ObservationRecordOutcome {
        let purpose = record.purpose;
        match self.append(record) {
            Ok(receipt) => {
                self.observation_failure_episodes
                    .lock()
                    .expect("observation failure episode mutex must not be poisoned")
                    .remove(&purpose);
                ObservationRecordOutcome::Appended(receipt)
            }
            Err(error) => {
                let first_failure = self
                    .observation_failure_episodes
                    .lock()
                    .expect("observation failure episode mutex must not be poisoned")
                    .insert(purpose);
                if first_failure {
                    ObservationRecordOutcome::FailureReported(error)
                } else {
                    ObservationRecordOutcome::FailureSuppressed
                }
            }
        }
    }

    fn append(&self, record: EncodedEvidenceRecord) -> Result<AppendReceipt, RecordFailure> {
        let sink_kind = sink_for_purpose(record.purpose);
        let sink = match sink_kind {
            KnownSink::Machine => &self.machine,
            KnownSink::Observation => &self.observation,
        };
        sink.lock()
            .expect("decision-evidence sink mutex must not be poisoned")
            .append(&record.line)
            .map_err(|source| RecordFailure::AppendFailed(source.into()))?;
        Ok(AppendReceipt {
            purpose: record.purpose,
            sink: sink_kind,
            bytes: record.line.len(),
        })
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForcedFailure {
    Write,
    Sync,
}

#[cfg(test)]
mod tests {
    use std::fs::OpenOptions;

    use super::*;

    fn recorder() -> DecisionEvidenceRecorder {
        let directory = tempfile::tempdir().expect("tempdir must exist");
        let machine = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("machine.jsonl"))
            .expect("machine sink must open");
        let observation = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.path().join("observation.jsonl"))
            .expect("observation sink must open");
        DecisionEvidenceRecorder::new(machine, observation)
    }

    fn record(purpose: KnownPurpose) -> EncodedEvidenceRecord {
        EncodedEvidenceRecord::try_new(purpose, b"{}\n".to_vec()).expect("test record must encode")
    }

    #[test]
    fn receipt_exists_only_after_write_and_sync_succeed() {
        let recorder = recorder();
        let receipt = recorder
            .record_blocking(record(KnownPurpose::EntryOrderIntent))
            .expect("healthy sink must append");
        assert_eq!(receipt.purpose, KnownPurpose::EntryOrderIntent);
        assert_eq!(receipt.sink, KnownSink::Machine);
        assert_eq!(receipt.bytes, 3);

        recorder
            .machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Write);
        assert!(matches!(
            recorder.record_blocking(record(KnownPurpose::EntryOrderIntent)),
            Err(RecordFailure::AppendFailed(_))
        ));

        recorder
            .machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Sync);
        assert!(matches!(
            recorder.record_blocking(record(KnownPurpose::EntryOrderIntent)),
            Err(RecordFailure::AppendFailed(_))
        ));
    }

    #[test]
    fn observation_failures_report_once_per_purpose_until_success() {
        let recorder = recorder();
        recorder
            .observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Write);

        assert!(matches!(
            recorder.record_observation(record(KnownPurpose::BlockedStrategyInputObservation)),
            ObservationRecordOutcome::FailureReported(_)
        ));
        assert!(matches!(
            recorder.record_observation(record(KnownPurpose::BlockedStrategyInputObservation)),
            ObservationRecordOutcome::FailureSuppressed
        ));
        assert!(matches!(
            recorder.record_observation(record(KnownPurpose::EntrySkipObservation)),
            ObservationRecordOutcome::FailureReported(_)
        ));

        recorder
            .observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = None;
        assert!(matches!(
            recorder.record_observation(record(KnownPurpose::BlockedStrategyInputObservation)),
            ObservationRecordOutcome::Appended(_)
        ));

        recorder
            .observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Sync);
        assert!(matches!(
            recorder.record_observation(record(KnownPurpose::BlockedStrategyInputObservation)),
            ObservationRecordOutcome::FailureReported(_)
        ));
    }

    #[test]
    fn malformed_encoded_record_is_rejected_before_any_sink_call() {
        assert!(matches!(
            EncodedEvidenceRecord::try_new(KnownPurpose::EntryOrderIntent, b"{}".to_vec()),
            Err(RecordFailure::Rejected(_))
        ));
    }
}
