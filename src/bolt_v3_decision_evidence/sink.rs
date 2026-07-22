use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};

use super::{
    LoadedBoltV3Config,
    generated_contract::{KnownPurpose, KnownSink, sink_for_purpose},
    machine_decision_evidence_path, observation_decision_evidence_path,
    open_decision_evidence_append_file,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordRejection {
    EmptyRecord,
    MissingLineTerminator,
}

#[derive(Debug)]
pub enum RecordError {
    Rejected(RecordRejection),
    AppendFailed(anyhow::Error),
}

impl RecordError {
    pub(super) fn into_anyhow(self) -> anyhow::Error {
        match self {
            Self::Rejected(RecordRejection::EmptyRecord) => {
                anyhow!("decision-evidence record must not be empty")
            }
            Self::Rejected(RecordRejection::MissingLineTerminator) => {
                anyhow!("decision-evidence record must end with a newline")
            }
            Self::AppendFailed(error) => error,
        }
    }
}

#[derive(Debug)]
pub struct EncodedEvidenceRecord {
    bytes: Vec<u8>,
    purpose: KnownPurpose,
    sink: KnownSink,
}

impl EncodedEvidenceRecord {
    pub(crate) fn new(bytes: Vec<u8>, purpose: KnownPurpose) -> Result<Self, RecordError> {
        if bytes.is_empty() {
            return Err(RecordError::Rejected(RecordRejection::EmptyRecord));
        }
        if !bytes.ends_with(b"\n") {
            return Err(RecordError::Rejected(
                RecordRejection::MissingLineTerminator,
            ));
        }
        Ok(Self {
            bytes,
            purpose,
            sink: sink_for_purpose(purpose),
        })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendReceipt {
    purpose: KnownPurpose,
    sink: KnownSink,
    bytes: usize,
}

impl AppendReceipt {
    #[cfg(test)]
    fn sink(self) -> KnownSink {
        self.sink
    }

    #[cfg(test)]
    fn bytes(self) -> usize {
        self.bytes
    }
}

pub trait DecisionEvidenceSink: std::fmt::Debug + Send + Sync {
    fn append(&self, record: EncodedEvidenceRecord) -> Result<AppendReceipt, RecordError>;
    fn drain_shutdown(&self) -> Result<()>;
}

#[derive(Debug)]
pub struct JsonlDecisionEvidenceSink {
    machine_file: Mutex<fs::File>,
    observation_file: Mutex<fs::File>,
}

trait DurableAppendTarget {
    fn write_record(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn sync_record(&mut self) -> io::Result<()>;
}

impl DurableAppendTarget for fs::File {
    fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.write_all(bytes)
    }

    fn sync_record(&mut self) -> io::Result<()> {
        self.sync_data()
    }
}

impl JsonlDecisionEvidenceSink {
    pub(crate) fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        Self::from_paths(
            machine_decision_evidence_path(loaded)?,
            observation_decision_evidence_path(loaded)?,
        )
    }

    pub(crate) fn from_paths(machine_path: PathBuf, observation_path: PathBuf) -> Result<Self> {
        if machine_path == observation_path {
            return Err(anyhow!(
                "machine and observation decision-evidence paths must differ"
            ));
        }
        let machine_file = open_sink_file(&machine_path, "machine")?;
        let observation_file = open_sink_file(&observation_path, "observation")?;
        Ok(Self {
            machine_file: Mutex::new(machine_file),
            observation_file: Mutex::new(observation_file),
        })
    }

    fn append_to<T: DurableAppendTarget>(
        file: &Mutex<T>,
        record: EncodedEvidenceRecord,
    ) -> Result<AppendReceipt, RecordError> {
        let mut file = file.lock().map_err(|_| {
            RecordError::AppendFailed(anyhow!("decision-evidence writer lock is poisoned"))
        })?;
        file.write_record(&record.bytes).map_err(|error| {
            RecordError::AppendFailed(
                anyhow::Error::new(error).context("failed to write decision-evidence record"),
            )
        })?;
        file.sync_record().map_err(|error| {
            RecordError::AppendFailed(
                anyhow::Error::new(error).context("failed to sync decision evidence to disk"),
            )
        })?;
        Ok(AppendReceipt {
            purpose: record.purpose,
            sink: record.sink,
            bytes: record.bytes.len(),
        })
    }
}

impl DecisionEvidenceSink for JsonlDecisionEvidenceSink {
    fn append(&self, record: EncodedEvidenceRecord) -> Result<AppendReceipt, RecordError> {
        match record.sink {
            KnownSink::Machine => Self::append_to(&self.machine_file, record),
            KnownSink::Observation => Self::append_to(&self.observation_file, record),
        }
    }

    fn drain_shutdown(&self) -> Result<()> {
        for (file, label) in [
            (&self.machine_file, "machine"),
            (&self.observation_file, "observation"),
        ] {
            file.lock()
                .map_err(|_| anyhow!("decision-evidence writer lock is poisoned"))?
                .sync_all()
                .with_context(|| format!("failed to drain {label} decision evidence to disk"))?;
        }
        Ok(())
    }
}

fn open_sink_file(path: &Path, label: &str) -> Result<fs::File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create {label} decision-evidence directory `{}`",
                parent.display()
            )
        })?;
    }
    open_decision_evidence_append_file(path).with_context(|| {
        format!(
            "failed to open {label} decision-evidence file `{}`",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct TestAppendTarget {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_sync: bool,
        sync_calls: usize,
    }

    impl DurableAppendTarget for TestAppendTarget {
        fn write_record(&mut self, bytes: &[u8]) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::other("injected write failure"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(())
        }

        fn sync_record(&mut self) -> io::Result<()> {
            self.sync_calls += 1;
            if self.fail_sync {
                return Err(io::Error::other("injected sync failure"));
            }
            Ok(())
        }
    }

    #[test]
    fn routes_machine_and_observation_records_to_separate_files() {
        let temp = tempfile::tempdir().expect("sink tempdir should create");
        let machine_path = temp.path().join("machine.jsonl");
        let observation_path = temp.path().join("observation.jsonl");
        let sink =
            JsonlDecisionEvidenceSink::from_paths(machine_path.clone(), observation_path.clone())
                .expect("separate sink paths should open");

        let machine = EncodedEvidenceRecord::new(
            b"{\"machine\":true}\n".to_vec(),
            KnownPurpose::SubmitReservationMetadata,
        )
        .expect("machine record should validate");
        let observation = EncodedEvidenceRecord::new(
            b"{\"observation\":true}\n".to_vec(),
            KnownPurpose::EntrySkipObservation,
        )
        .expect("observation record should validate");
        let machine_receipt = sink.append(machine).expect("machine append should sync");
        let observation_receipt = sink
            .append(observation)
            .expect("observation append should sync");

        assert_eq!(machine_receipt.sink(), KnownSink::Machine);
        assert_eq!(machine_receipt.bytes(), 17);
        assert_eq!(observation_receipt.sink(), KnownSink::Observation);
        assert_eq!(observation_receipt.bytes(), 21);
        assert_eq!(
            fs::read(machine_path).expect("machine file should read"),
            b"{\"machine\":true}\n"
        );
        assert_eq!(
            fs::read(observation_path).expect("observation file should read"),
            b"{\"observation\":true}\n"
        );
    }

    #[test]
    fn rejects_invalid_record_before_append() {
        assert!(matches!(
            EncodedEvidenceRecord::new(Vec::new(), KnownPurpose::SubmitReservationMetadata),
            Err(RecordError::Rejected(RecordRejection::EmptyRecord))
        ));
        assert!(matches!(
            EncodedEvidenceRecord::new(b"{}".to_vec(), KnownPurpose::EntrySkipObservation),
            Err(RecordError::Rejected(
                RecordRejection::MissingLineTerminator
            ))
        ));
    }

    #[test]
    fn rejects_shared_sink_path() {
        let temp = tempfile::tempdir().expect("sink tempdir should create");
        let shared = temp.path().join("shared.jsonl");
        let error = JsonlDecisionEvidenceSink::from_paths(shared.clone(), shared)
            .expect_err("shared sink path must reject");
        assert!(error.to_string().contains("paths must differ"));
    }

    #[test]
    fn write_failure_returns_no_receipt_and_does_not_sync() {
        let target = Mutex::new(TestAppendTarget {
            fail_write: true,
            ..TestAppendTarget::default()
        });

        let record = EncodedEvidenceRecord::new(
            b"{\"machine\":true}\n".to_vec(),
            KnownPurpose::SubmitReservationMetadata,
        )
        .unwrap();
        let result = JsonlDecisionEvidenceSink::append_to(&target, record);

        assert!(matches!(result, Err(RecordError::AppendFailed(_))));
        let target = target
            .into_inner()
            .expect("test target lock should be healthy");
        assert!(target.bytes.is_empty());
        assert_eq!(target.sync_calls, 0);
    }

    #[test]
    fn sync_failure_returns_no_receipt_after_write_attempt() {
        let target = Mutex::new(TestAppendTarget {
            fail_sync: true,
            ..TestAppendTarget::default()
        });

        let record = EncodedEvidenceRecord::new(
            b"{\"observation\":true}\n".to_vec(),
            KnownPurpose::EntrySkipObservation,
        )
        .unwrap();
        let result = JsonlDecisionEvidenceSink::append_to(&target, record);

        assert!(matches!(result, Err(RecordError::AppendFailed(_))));
        let target = target
            .into_inner()
            .expect("test target lock should be healthy");
        assert_eq!(target.bytes, b"{\"observation\":true}\n");
        assert_eq!(target.sync_calls, 1);
    }

    #[test]
    fn receipt_is_created_only_after_write_and_sync_succeed() {
        let target = Mutex::new(TestAppendTarget::default());

        let record = EncodedEvidenceRecord::new(
            b"{\"observation\":true}\n".to_vec(),
            KnownPurpose::EntrySkipObservation,
        )
        .unwrap();
        let receipt = JsonlDecisionEvidenceSink::append_to(&target, record)
            .expect("successful write and sync should produce a receipt");

        assert_eq!(receipt.sink(), KnownSink::Observation);
        assert_eq!(receipt.bytes(), 21);
        let target = target
            .into_inner()
            .expect("test target lock should be healthy");
        assert_eq!(target.bytes, b"{\"observation\":true}\n");
        assert_eq!(target.sync_calls, 1);
    }
}
