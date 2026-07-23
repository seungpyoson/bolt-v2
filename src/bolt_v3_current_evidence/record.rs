use std::{
    collections::BTreeSet,
    fmt,
    fs::File,
    io::{self, Write},
    sync::{Arc, Mutex},
};

use anyhow::Error;

use super::codec::{
    encode_admitted_entry_admission, encode_basket_admission_granted,
    encode_basket_admission_rejected, encode_blocked_strategy_input_observation,
    encode_capital_admission_rebuild, encode_entry_order_intent, encode_entry_skip_observation,
    encode_exit_evaluation, encode_exit_hold_decision, encode_exit_submission_decision,
    encode_forced_reduction_admission, encode_loss_governor_halt, encode_order_lifecycle,
    encode_order_reject, encode_rejected_entry_admission, encode_replace_admission,
    encode_requote_throttle_observation, encode_reservation_fill, encode_reservation_metadata,
    encode_risk_reducing_exit_admission, encode_risk_reducing_exit_order_intent, encode_settlement,
    encode_submit_linked_strategy_input_snapshot, encode_terminal_settlement,
    encode_venue_truth_capture_failure, encode_venue_truth_divergence,
};
use super::facts::{
    AdmittedEntryAdmissionFact, BasketAdmissionGrantedFact, BasketAdmissionRejectedFact,
    BlockedStrategyInputObservationFact, CapitalAdmissionRebuildFact, EntryOrderIntentFact,
    EntrySkipFact, ExitEvaluationFact, ExitHoldDecisionFact, ExitSubmissionDecisionFact,
    ForcedReductionAdmissionFact, LossGovernorHaltFact, OrderLifecycleFact, OrderRejectFact,
    RejectedEntryAdmissionFact, ReplaceAdmissionFact, RequoteThrottleObservationFact,
    RiskReducingExitAdmissionFact, RiskReducingExitOrderIntentFact, SettlementFact,
    SubmitLinkedStrategyInputSnapshotFact, SubmitReservationFillFact,
    SubmitReservationMetadataFact, TerminalSettlementFact, VenueTruthCaptureFailureFact,
    VenueTruthDivergenceFact,
};
use super::generated_contract::{
    EffectPolicy, KnownProducer, KnownPurpose, KnownSink, effect_policy_for_purpose,
    purpose_for_producer, sink_for_purpose,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendReceipt {
    purpose: KnownPurpose,
    sink: KnownSink,
    bytes: usize,
}

#[must_use = "a committed settlement must drive its post-commit reducers"]
#[derive(Debug)]
pub struct CommittedSettlement {
    fact: SettlementFact,
    _receipt: AppendReceipt,
}

impl CommittedSettlement {
    #[must_use]
    pub fn fact(&self) -> &SettlementFact {
        &self.fact
    }
}

#[derive(Debug)]
pub enum RecordFailure {
    Rejected(Error),
    CommitIndeterminate { phase: CommitPhase, cause: Arc<str> },
    SinkPoisoned { first_cause: PoisonCause },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitPhase {
    Write,
    Sync,
}

impl fmt::Display for CommitPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Write => formatter.write_str("write"),
            Self::Sync => formatter.write_str("sync"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoisonCause {
    CommitIndeterminate { phase: CommitPhase, cause: Arc<str> },
    StartupContentInvalid { cause: Arc<str> },
}

impl fmt::Display for PoisonCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommitIndeterminate { phase, cause } => {
                write!(formatter, "commit indeterminate during {phase}: {cause}")
            }
            Self::StartupContentInvalid { cause } => {
                write!(formatter, "startup content invalid: {cause}")
            }
        }
    }
}

impl fmt::Display for RecordFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(source) => write!(formatter, "evidence record rejected: {source}"),
            Self::CommitIndeterminate { phase, cause } => {
                write!(
                    formatter,
                    "evidence commit indeterminate during {phase}: {cause}"
                )
            }
            Self::SinkPoisoned { first_cause } => {
                write!(formatter, "evidence sink poisoned after {first_cause}")
            }
        }
    }
}

impl std::error::Error for RecordFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Rejected(source) => Some(source.as_ref()),
            Self::CommitIndeterminate { .. } | Self::SinkPoisoned { .. } => None,
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

    #[cfg(test)]
    pub(crate) fn line(&self) -> &[u8] {
        &self.line
    }
}

#[derive(Debug)]
pub(crate) struct DurableSink {
    file: File,
    state: DurableSinkState,
    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    forced_failure: Option<ForcedFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DurableSinkState {
    Healthy,
    Poisoned(PoisonCause),
}

impl DurableSink {
    pub(crate) fn new(file: File) -> Self {
        Self {
            file,
            state: DurableSinkState::Healthy,
            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            forced_failure: None,
        }
    }

    pub(super) fn poisoned(file: File, cause: PoisonCause) -> Self {
        Self {
            file,
            state: DurableSinkState::Poisoned(cause),
            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            forced_failure: None,
        }
    }

    fn append(&mut self, line: &[u8]) -> Result<(), RecordFailure> {
        if let DurableSinkState::Poisoned(first_cause) = &self.state {
            return Err(RecordFailure::SinkPoisoned {
                first_cause: first_cause.clone(),
            });
        }

        let result = (|| -> Result<(), (CommitPhase, io::Error)> {
            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            match self.forced_failure {
                Some(ForcedFailure::Write) => {
                    return Err((
                        CommitPhase::Write,
                        io::Error::other("injected write failure"),
                    ));
                }
                #[cfg(test)]
                Some(ForcedFailure::PartialWrite(bytes)) => {
                    let bytes = bytes.min(line.len());
                    self.file
                        .write_all(&line[..bytes])
                        .map_err(|source| (CommitPhase::Write, source))?;
                    return Err((
                        CommitPhase::Write,
                        io::Error::other("injected partial write failure"),
                    ));
                }
                #[cfg(test)]
                Some(ForcedFailure::Sync) => {}
                None => {}
            }

            self.file
                .write_all(line)
                .map_err(|source| (CommitPhase::Write, source))?;
            #[cfg(test)]
            if self.forced_failure == Some(ForcedFailure::Sync) {
                return Err((CommitPhase::Sync, io::Error::other("injected sync failure")));
            }
            self.file
                .sync_data()
                .map_err(|source| (CommitPhase::Sync, source))
        })();

        match result {
            Ok(()) => Ok(()),
            Err((phase, source)) => {
                let cause: Arc<str> = Arc::from(source.to_string());
                self.state = DurableSinkState::Poisoned(PoisonCause::CommitIndeterminate {
                    phase,
                    cause: Arc::clone(&cause),
                });
                Err(RecordFailure::CommitIndeterminate { phase, cause })
            }
        }
    }
}

#[derive(Debug)]
pub struct DecisionEvidenceRecorder {
    machine: Mutex<DurableSink>,
    observation: Mutex<DurableSink>,
    reject_episode_max_count: usize,
    observation_failure_episodes: Mutex<BTreeSet<KnownPurpose>>,
    #[cfg(test)]
    test_attempts: Mutex<std::collections::BTreeMap<KnownPurpose, usize>>,
    #[cfg(test)]
    test_failure: Mutex<Option<(KnownPurpose, usize)>>,
}

impl DecisionEvidenceRecorder {
    pub(super) fn from_files(
        machine: File,
        observation: File,
        observation_poison: Option<PoisonCause>,
        reject_episode_max_count: usize,
    ) -> Self {
        assert!(reject_episode_max_count > 0);
        let observation = match observation_poison {
            Some(cause) => DurableSink::poisoned(observation, cause),
            None => DurableSink::new(observation),
        };
        Self {
            machine: Mutex::new(DurableSink::new(machine)),
            observation: Mutex::new(observation),
            reject_episode_max_count,
            observation_failure_episodes: Mutex::new(BTreeSet::new()),
            #[cfg(test)]
            test_attempts: Mutex::new(std::collections::BTreeMap::new()),
            #[cfg(test)]
            test_failure: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub(crate) fn recording() -> Self {
        Self::from_files(
            tempfile::tempfile().expect("test machine evidence sink must open"),
            tempfile::tempfile().expect("test observation evidence sink must open"),
            None,
            4096,
        )
    }

    pub(crate) const fn reject_episode_max_count(&self) -> usize {
        self.reject_episode_max_count
    }

    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
    #[doc(hidden)]
    pub fn fail_machine_writes_for_test(&self) {
        self.machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Write);
    }

    #[cfg(test)]
    pub(crate) fn fail_machine_sync_for_test(&self) {
        self.machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Sync);
    }

    #[cfg(test)]
    pub(crate) fn fail_observation_writes(&self) {
        self.observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Write);
    }

    #[cfg(test)]
    pub(crate) fn fail_purpose_on_attempt(&self, purpose: KnownPurpose, attempt: usize) {
        *self
            .test_failure
            .lock()
            .expect("test failure mutex must not be poisoned") = Some((purpose, attempt));
    }

    #[cfg(test)]
    pub(crate) fn attempts_for(&self, purpose: KnownPurpose) -> usize {
        self.test_attempts
            .lock()
            .expect("test attempts mutex must not be poisoned")
            .get(&purpose)
            .copied()
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn recorded_facts(&self) -> anyhow::Result<Vec<super::facts::CurrentFact>> {
        use std::io::{Read, Seek, SeekFrom};

        #[derive(serde::Deserialize)]
        struct Header {
            kind: String,
            schema_version: u32,
        }

        fn decode_sink(
            sink: &Mutex<DurableSink>,
            facts: &mut Vec<super::facts::CurrentFact>,
        ) -> anyhow::Result<()> {
            let sink = sink
                .lock()
                .expect("evidence sink mutex must not be poisoned");
            let mut file = sink.file.try_clone()?;
            file.seek(SeekFrom::Start(0))?;
            let mut contents = String::new();
            file.read_to_string(&mut contents)?;
            for (index, line) in contents.lines().enumerate() {
                let header: Header = serde_json::from_str(line)?;
                let identity = super::generated_contract::resolve_identity(
                    &header.kind,
                    header.schema_version,
                )
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "recorded test evidence has unknown identity ({}, {})",
                        header.kind,
                        header.schema_version
                    )
                })?;
                facts.push(super::codec::decode_current_fact(
                    identity,
                    line,
                    index + 1,
                )?);
            }
            Ok(())
        }

        let mut facts = Vec::new();
        decode_sink(&self.machine, &mut facts)?;
        decode_sink(&self.observation, &mut facts)?;
        Ok(facts)
    }

    #[cfg(test)]
    pub(crate) fn startup_recovery_projections(
        &self,
        max_bytes: Option<u64>,
    ) -> anyhow::Result<(
        super::facts::ReservationRecoveryFacts,
        super::facts::SettlementRecoveryFacts,
        super::facts::BookingRecoveryFacts,
    )> {
        let sink = self
            .machine
            .lock()
            .expect("machine sink mutex must not be poisoned");
        let mut file = sink.file.try_clone()?;
        let projections = super::reader::validate_stream(&mut file, KnownSink::Machine, max_bytes)?
            .startup_recovery;
        Ok((
            projections.reservation,
            projections.settlement,
            projections.booking,
        ))
    }

    pub fn record_submit_reservation_metadata(
        &self,
        command: SubmitReservationMetadataFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::SubmitReservationMetadata,
            encode_reservation_metadata(command)?,
        )
    }

    pub fn record_submit_reservation_fill(
        &self,
        command: SubmitReservationFillFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::SubmitReservationFill,
            encode_reservation_fill(command)?,
        )
    }

    pub fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::OrderExecutionEntryIntent,
            encode_entry_order_intent(fact)?,
        )
    }

    pub fn record_admitted_entry_admission(
        &self,
        fact: AdmittedEntryAdmissionFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::SubmitAdmissionAdmittedEntry,
            encode_admitted_entry_admission(fact)?,
        )
    }

    pub fn record_rejected_entry_admission(
        &self,
        fact: RejectedEntryAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match encode_rejected_entry_admission(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::SubmitAdmissionRejectedEntry, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_risk_reducing_exit_admission(
        &self,
        fact: RiskReducingExitAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match encode_risk_reducing_exit_admission(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::SubmitAdmissionExit, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_replace_admission(
        &self,
        fact: ReplaceAdmissionFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::SubmitAdmissionReplace,
            encode_replace_admission(fact)?,
        )
    }

    pub fn record_forced_reduction_admission(
        &self,
        fact: ForcedReductionAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match encode_forced_reduction_admission(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::SubmitAdmissionForcedReduction, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        match encode_risk_reducing_exit_order_intent(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::OrderExecutionExitIntent, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_basket_admission_granted(
        &self,
        fact: BasketAdmissionGrantedFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::BasketAdmissionGranted,
            encode_basket_admission_granted(fact)?,
        )
    }

    pub fn record_basket_admission_rejected(
        &self,
        fact: BasketAdmissionRejectedFact,
    ) -> NonBlockingRecordOutcome {
        match encode_basket_admission_rejected(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::BasketAdmissionRejected, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_capital_admission_rebuild(
        &self,
        fact: CapitalAdmissionRebuildFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::CapitalAdmissionRebuild,
            encode_capital_admission_rebuild(fact)?,
        )
    }

    pub fn record_order_lifecycle(&self, fact: OrderLifecycleFact) -> NonBlockingRecordOutcome {
        match encode_order_lifecycle(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::EdgeTakerOrderLifecycle, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_requote_throttle_observation(
        &self,
        fact: RequoteThrottleObservationFact,
    ) -> ObservationRecordOutcome {
        match encode_requote_throttle_observation(fact) {
            Ok(record) => self.record_observation(KnownProducer::MakerRequoteThrottle, record),
            Err(error) => {
                self.report_observation_failure(KnownPurpose::RequoteThrottleObservation, error)
            }
        }
    }

    pub fn record_venue_truth_capture_failure(
        &self,
        fact: VenueTruthCaptureFailureFact,
    ) -> NonBlockingRecordOutcome {
        match encode_venue_truth_capture_failure(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::SubmitAdmissionVenueCaptureFailure, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_venue_truth_divergence(
        &self,
        fact: VenueTruthDivergenceFact,
    ) -> NonBlockingRecordOutcome {
        match encode_venue_truth_divergence(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::SubmitAdmissionVenueDivergence, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_loss_governor_halt(
        &self,
        fact: LossGovernorHaltFact,
    ) -> NonBlockingRecordOutcome {
        match encode_loss_governor_halt(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::SubmitAdmissionLossHalt, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_submit_admission_order_reject(
        &self,
        fact: OrderRejectFact,
    ) -> NonBlockingRecordOutcome {
        match encode_order_reject(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::SubmitAdmissionOrderReject, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_observed_order_reject(&self, fact: OrderRejectFact) -> NonBlockingRecordOutcome {
        match encode_order_reject(fact) {
            Ok(record) => self.record_nonblocking(KnownProducer::OrderRejectObserverFeed, record),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_entry_skip_observation(&self, fact: EntrySkipFact) -> ObservationRecordOutcome {
        match encode_entry_skip_observation(fact) {
            Ok(record) => self.record_observation(KnownProducer::EdgeTakerEntrySkip, record),
            Err(error) => {
                self.report_observation_failure(KnownPurpose::EntrySkipObservation, error)
            }
        }
    }

    pub fn record_blocked_strategy_input_observation(
        &self,
        fact: BlockedStrategyInputObservationFact,
    ) -> ObservationRecordOutcome {
        match encode_blocked_strategy_input_observation(fact) {
            Ok(record) => {
                self.record_observation(KnownProducer::EdgeTakerBlockedStrategyInput, record)
            }
            Err(error) => self
                .report_observation_failure(KnownPurpose::BlockedStrategyInputObservation, error),
        }
    }

    pub fn record_submit_linked_strategy_input_snapshot(
        &self,
        fact: SubmitLinkedStrategyInputSnapshotFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::EdgeTakerSubmitStrategyInput,
            encode_submit_linked_strategy_input_snapshot(fact)?,
        )
    }

    pub fn record_exit_submission_decision(
        &self,
        fact: ExitSubmissionDecisionFact,
    ) -> NonBlockingRecordOutcome {
        match encode_exit_submission_decision(fact) {
            Ok(record) => {
                self.record_nonblocking(KnownProducer::EdgeTakerExitSubmitDecision, record)
            }
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_exit_hold_decision(
        &self,
        fact: ExitHoldDecisionFact,
    ) -> ObservationRecordOutcome {
        match encode_exit_hold_decision(fact) {
            Ok(record) => self.record_observation(KnownProducer::EdgeTakerExitHoldDecision, record),
            Err(error) => self.report_observation_failure(KnownPurpose::ExitHoldDecision, error),
        }
    }

    pub fn record_exit_evaluation(&self, fact: ExitEvaluationFact) -> ObservationRecordOutcome {
        match encode_exit_evaluation(fact) {
            Ok(record) => self.record_observation(KnownProducer::EdgeTakerExitEvaluation, record),
            Err(error) => self.report_observation_failure(KnownPurpose::ExitEvaluation, error),
        }
    }

    pub fn record_settlement(
        &self,
        fact: SettlementFact,
    ) -> Result<CommittedSettlement, RecordFailure> {
        let record = encode_settlement(fact.clone())?;
        let receipt = self.record_blocking(KnownProducer::EdgeTakerSettlement, record)?;
        Ok(CommittedSettlement {
            fact,
            _receipt: receipt,
        })
    }

    pub fn record_terminal_settlement(
        &self,
        fact: TerminalSettlementFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.record_blocking(
            KnownProducer::EdgeTakerTerminalSettlement,
            encode_terminal_settlement(fact)?,
        )
    }

    pub(crate) fn record_blocking(
        &self,
        producer: KnownProducer,
        record: EncodedEvidenceRecord,
    ) -> Result<AppendReceipt, RecordFailure> {
        assert_contract_policy(
            producer,
            record.purpose,
            &[
                EffectPolicy::MustPrecedeNewRisk,
                EffectPolicy::ReconciliationFailClosed,
            ],
        );
        self.append(record)
    }

    pub(crate) fn record_nonblocking(
        &self,
        producer: KnownProducer,
        record: EncodedEvidenceRecord,
    ) -> NonBlockingRecordOutcome {
        assert_contract_policy(
            producer,
            record.purpose,
            &[
                EffectPolicy::PreserveResult,
                EffectPolicy::RiskReducingContinues,
            ],
        );
        match self.append(record) {
            Ok(receipt) => NonBlockingRecordOutcome::Appended(receipt),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub(crate) fn record_observation(
        &self,
        producer: KnownProducer,
        record: EncodedEvidenceRecord,
    ) -> ObservationRecordOutcome {
        assert_contract_policy(
            producer,
            record.purpose,
            &[EffectPolicy::ObservationBoundedFailure],
        );
        let purpose = record.purpose;
        match self.append(record) {
            Ok(receipt) => {
                self.observation_failure_episodes
                    .lock()
                    .expect("observation failure episode mutex must not be poisoned")
                    .remove(&purpose);
                ObservationRecordOutcome::Appended(receipt)
            }
            Err(error) => self.report_observation_failure(purpose, error),
        }
    }

    fn report_observation_failure(
        &self,
        purpose: KnownPurpose,
        error: RecordFailure,
    ) -> ObservationRecordOutcome {
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

    fn append(&self, record: EncodedEvidenceRecord) -> Result<AppendReceipt, RecordFailure> {
        #[cfg(test)]
        let inject_failure = {
            let attempt = {
                let mut attempts = self
                    .test_attempts
                    .lock()
                    .expect("test attempts mutex must not be poisoned");
                let attempt = attempts.entry(record.purpose).or_default();
                *attempt += 1;
                *attempt
            };
            *self
                .test_failure
                .lock()
                .expect("test failure mutex must not be poisoned")
                == Some((record.purpose, attempt))
        };
        let sink_kind = sink_for_purpose(record.purpose);
        let sink = match sink_kind {
            KnownSink::Machine => &self.machine,
            KnownSink::Observation => &self.observation,
        };
        let mut sink = sink
            .lock()
            .expect("decision-evidence sink mutex must not be poisoned");
        #[cfg(test)]
        if inject_failure {
            sink.forced_failure = Some(ForcedFailure::Write);
        }
        sink.append(&record.line)?;
        Ok(AppendReceipt {
            purpose: record.purpose,
            sink: sink_kind,
            bytes: record.line.len(),
        })
    }
}

fn assert_contract_policy(
    producer: KnownProducer,
    purpose: KnownPurpose,
    permitted: &[EffectPolicy],
) {
    assert_eq!(purpose_for_producer(producer), purpose);
    assert!(permitted.contains(&effect_policy_for_purpose(purpose)));
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ForcedFailure {
    Write,
    #[cfg(test)]
    PartialWrite(usize),
    #[cfg(test)]
    Sync,
}

#[cfg(test)]
mod tests {
    use std::{fs, fs::OpenOptions, path::PathBuf};

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
        DecisionEvidenceRecorder::from_files(machine, observation, None, 4096)
    }

    fn recorder_with_paths() -> (
        tempfile::TempDir,
        DecisionEvidenceRecorder,
        PathBuf,
        PathBuf,
    ) {
        let directory = tempfile::tempdir().expect("tempdir must exist");
        let machine_path = directory.path().join("machine.jsonl");
        let observation_path = directory.path().join("observation.jsonl");
        let machine = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&machine_path)
            .expect("machine sink must open");
        let observation = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&observation_path)
            .expect("observation sink must open");
        (
            directory,
            DecisionEvidenceRecorder::from_files(machine, observation, None, 4096),
            machine_path,
            observation_path,
        )
    }

    fn record(purpose: KnownPurpose) -> EncodedEvidenceRecord {
        EncodedEvidenceRecord::try_new(purpose, b"{}\n".to_vec()).expect("test record must encode")
    }

    #[test]
    fn receipt_exists_only_after_write_and_sync_succeed() {
        let (_directory, recorder, machine_path, observation_path) = recorder_with_paths();
        let receipt = recorder
            .record_blocking(
                KnownProducer::OrderExecutionEntryIntent,
                record(KnownPurpose::EntryOrderIntent),
            )
            .expect("healthy sink must append");
        assert_eq!(receipt.purpose, KnownPurpose::EntryOrderIntent);
        assert_eq!(receipt.sink, KnownSink::Machine);
        assert_eq!(receipt.bytes, 3);

        assert_eq!(fs::read(&machine_path).unwrap(), b"{}\n");

        let observation_receipt = recorder.record_observation(
            KnownProducer::EdgeTakerBlockedStrategyInput,
            record(KnownPurpose::BlockedStrategyInputObservation),
        );
        assert!(matches!(
            observation_receipt,
            ObservationRecordOutcome::Appended(_)
        ));
        assert_eq!(fs::read(&observation_path).unwrap(), b"{}\n");
    }

    #[test]
    fn partial_write_is_commit_indeterminate_and_poison_refuses_later_io() {
        let (_directory, recorder, machine_path, observation_path) = recorder_with_paths();

        recorder
            .machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::PartialWrite(1));
        assert!(matches!(
            recorder.record_blocking(
                KnownProducer::OrderExecutionEntryIntent,
                record(KnownPurpose::EntryOrderIntent),
            ),
            Err(RecordFailure::CommitIndeterminate {
                phase: CommitPhase::Write,
                ..
            })
        ));
        let retained = fs::read(&machine_path).unwrap();
        assert_eq!(retained, b"{");

        assert!(matches!(
            recorder.record_blocking(
                KnownProducer::OrderExecutionEntryIntent,
                record(KnownPurpose::EntryOrderIntent),
            ),
            Err(RecordFailure::SinkPoisoned { .. })
        ));
        assert_eq!(fs::read(&machine_path).unwrap(), retained);

        assert!(matches!(
            recorder.record_observation(
                KnownProducer::EdgeTakerBlockedStrategyInput,
                record(KnownPurpose::BlockedStrategyInputObservation),
            ),
            ObservationRecordOutcome::Appended(_)
        ));
        assert_eq!(fs::read(&observation_path).unwrap(), b"{}\n");
    }

    #[test]
    fn sync_failure_is_commit_indeterminate_and_poison_refuses_later_io() {
        let (_directory, recorder, machine_path, _observation_path) = recorder_with_paths();
        recorder
            .machine
            .lock()
            .expect("machine sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Sync);

        assert!(matches!(
            recorder.record_blocking(
                KnownProducer::OrderExecutionEntryIntent,
                record(KnownPurpose::EntryOrderIntent),
            ),
            Err(RecordFailure::CommitIndeterminate {
                phase: CommitPhase::Sync,
                ..
            })
        ));
        let retained = fs::read(&machine_path).unwrap();
        assert_eq!(retained, b"{}\n");

        assert!(matches!(
            recorder.record_blocking(
                KnownProducer::OrderExecutionEntryIntent,
                record(KnownPurpose::EntryOrderIntent),
            ),
            Err(RecordFailure::SinkPoisoned { .. })
        ));
        assert_eq!(fs::read(&machine_path).unwrap(), retained);
    }

    #[test]
    fn observation_poison_reports_once_per_purpose_and_never_resumes() {
        let recorder = recorder();
        recorder
            .observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = Some(ForcedFailure::Write);

        assert!(matches!(
            recorder.record_observation(
                KnownProducer::EdgeTakerBlockedStrategyInput,
                record(KnownPurpose::BlockedStrategyInputObservation),
            ),
            ObservationRecordOutcome::FailureReported(_)
        ));
        assert!(matches!(
            recorder.record_observation(
                KnownProducer::EdgeTakerBlockedStrategyInput,
                record(KnownPurpose::BlockedStrategyInputObservation),
            ),
            ObservationRecordOutcome::FailureSuppressed
        ));
        assert!(matches!(
            recorder.record_observation(
                KnownProducer::EdgeTakerEntrySkip,
                record(KnownPurpose::EntrySkipObservation),
            ),
            ObservationRecordOutcome::FailureReported(_)
        ));

        recorder
            .observation
            .lock()
            .expect("observation sink mutex must not be poisoned")
            .forced_failure = None;
        assert!(matches!(
            recorder.record_observation(
                KnownProducer::EdgeTakerBlockedStrategyInput,
                record(KnownPurpose::BlockedStrategyInputObservation),
            ),
            ObservationRecordOutcome::FailureSuppressed
        ));
    }

    #[test]
    fn malformed_encoded_record_is_rejected_before_any_sink_call() {
        assert!(matches!(
            EncodedEvidenceRecord::try_new(KnownPurpose::EntryOrderIntent, b"{}".to_vec()),
            Err(RecordFailure::Rejected(_))
        ));

        let recorder = recorder();
        assert!(
            recorder
                .record_blocking(
                    KnownProducer::OrderExecutionEntryIntent,
                    record(KnownPurpose::EntryOrderIntent),
                )
                .is_ok()
        );
    }

    #[test]
    #[should_panic(expected = "assertion `left == right` failed")]
    fn producer_cannot_emit_another_purposes_identity() {
        let recorder = recorder();
        let _ = recorder.record_blocking(
            KnownProducer::SubmitReservationMetadata,
            record(KnownPurpose::EntryOrderIntent),
        );
    }

    #[test]
    #[should_panic(expected = "assertion failed: permitted.contains")]
    fn effect_policy_cannot_cross_the_caller_outcome_boundary() {
        let recorder = recorder();
        let _ = recorder.record_nonblocking(
            KnownProducer::OrderExecutionEntryIntent,
            record(KnownPurpose::EntryOrderIntent),
        );
    }
}
