use std::sync::{Arc, Weak};

use super::{
    AdmittedEntryAdmissionFact, AppendReceipt, BasketAdmissionGrantedFact,
    BasketAdmissionRejectedFact, BlockedStrategyInputObservationFact, CapitalAdmissionRebuildFact,
    CommittedAdmission, CommittedSettlement, DecisionEvidenceRecorder, EntryOrderIntentFact,
    EntrySkipFact, ExitEvaluationFact, ExitHoldDecisionFact, ExitSubmissionDecisionFact,
    ForcedReductionAdmissionFact, LossGovernorHaltFact, NonBlockingRecordOutcome,
    ObservationRecordOutcome, OrderLifecycleFact, OrderRejectFact, RecordFailure,
    RejectedEntryAdmissionFact, RequoteThrottleObservationFact, RiskReducingExitAdmissionFact,
    RiskReducingExitOrderIntentFact, SettlementFact, SubmitLinkedStrategyInputSnapshotFact,
    SubmitReservationFillFact, TerminalSettlementFact, VenueTruthCaptureFailureFact,
    VenueTruthDivergenceFact,
};

fn closed() -> RecordFailure {
    RecordFailure::RecorderClosed
}

macro_rules! handle {
    ($name:ident) => {
        #[derive(Debug)]
        pub struct $name {
            recorder: Weak<DecisionEvidenceRecorder>,
            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            // Test-only constructors can own their in-memory sink. Production
            // factories always leave this empty, so handles cannot prolong
            // runtime ownership or the catalog lock.
            _test_owner: Option<Arc<DecisionEvidenceRecorder>>,
        }

        impl $name {
            pub(crate) fn new(recorder: &Arc<DecisionEvidenceRecorder>) -> Self {
                Self {
                    recorder: Arc::downgrade(recorder),
                    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
                    _test_owner: None,
                }
            }

            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            fn new_test_owned(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
                Self {
                    recorder: Arc::downgrade(&recorder),
                    _test_owner: Some(recorder),
                }
            }

            fn recorder(&self) -> Result<Arc<DecisionEvidenceRecorder>, RecordFailure> {
                self.recorder.upgrade().ok_or_else(closed)
            }
        }
    };
}

handle!(BasketAdmissionEvidence);
handle!(OrderExecutionEvidence);
handle!(EdgeTakerEvidence);
handle!(MakerEvidence);

macro_rules! reissuable_handle {
    ($name:ident) => {
        impl $name {
            pub(crate) fn reissue(&self) -> Self {
                Self {
                    recorder: self.recorder.clone(),
                    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
                    _test_owner: self._test_owner.as_ref().map(Arc::clone),
                }
            }
        }
    };
}

reissuable_handle!(OrderExecutionEvidence);
reissuable_handle!(EdgeTakerEvidence);
reissuable_handle!(MakerEvidence);

macro_rules! episode_bounded_handle {
    ($name:ident) => {
        #[derive(Debug)]
        pub struct $name {
            recorder: Weak<DecisionEvidenceRecorder>,
            reject_episode_max_count: usize,
            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            // See the non-episode handle: release builds contain no owner.
            _test_owner: Option<Arc<DecisionEvidenceRecorder>>,
        }

        impl $name {
            pub(crate) fn new(recorder: &Arc<DecisionEvidenceRecorder>) -> Self {
                Self {
                    recorder: Arc::downgrade(recorder),
                    reject_episode_max_count: recorder.reject_episode_max_count(),
                    #[cfg(any(test, feature = "test-current-evidence-inspection"))]
                    _test_owner: None,
                }
            }

            #[cfg(any(test, feature = "test-current-evidence-inspection"))]
            fn new_test_owned(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
                Self {
                    recorder: Arc::downgrade(&recorder),
                    reject_episode_max_count: recorder.reject_episode_max_count(),
                    _test_owner: Some(recorder),
                }
            }

            fn recorder(&self) -> Result<Arc<DecisionEvidenceRecorder>, RecordFailure> {
                self.recorder.upgrade().ok_or_else(closed)
            }

            pub(crate) fn reject_episode_max_count(&self) -> usize {
                self.reject_episode_max_count
            }
        }
    };
}

episode_bounded_handle!(SubmitAdmissionEvidence);
episode_bounded_handle!(OrderRejectObserverEvidence);

#[derive(Debug)]
pub struct StrategyEvidenceHandles {
    edge_taker: EdgeTakerEvidence,
    maker: MakerEvidence,
    order_execution: OrderExecutionEvidence,
}

impl StrategyEvidenceHandles {
    pub(super) fn new(recorder: &Arc<DecisionEvidenceRecorder>) -> Self {
        Self {
            edge_taker: EdgeTakerEvidence::new(recorder),
            maker: MakerEvidence::new(recorder),
            order_execution: OrderExecutionEvidence::new(recorder),
        }
    }

    #[must_use]
    pub(crate) fn edge_taker(&self) -> EdgeTakerEvidence {
        self.edge_taker.reissue()
    }

    #[must_use]
    pub(crate) fn maker(&self) -> MakerEvidence {
        self.maker.reissue()
    }

    #[must_use]
    pub(crate) fn order_execution(&self) -> OrderExecutionEvidence {
        self.order_execution.reissue()
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for SubmitAdmissionEvidence {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        Self::new_test_owned(recorder)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for BasketAdmissionEvidence {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        Self::new_test_owned(recorder)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for OrderExecutionEvidence {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        Self::new_test_owned(recorder)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for OrderRejectObserverEvidence {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        Self::new_test_owned(recorder)
    }
}

#[cfg(any(test, feature = "test-current-evidence-inspection"))]
impl From<Arc<DecisionEvidenceRecorder>> for StrategyEvidenceHandles {
    fn from(recorder: Arc<DecisionEvidenceRecorder>) -> Self {
        Self {
            edge_taker: EdgeTakerEvidence::new_test_owned(Arc::clone(&recorder)),
            maker: MakerEvidence::new_test_owned(Arc::clone(&recorder)),
            order_execution: OrderExecutionEvidence::new_test_owned(recorder),
        }
    }
}

impl SubmitAdmissionEvidence {
    pub fn record_submit_reservation_fill(
        &self,
        fact: SubmitReservationFillFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.recorder()?.record_submit_reservation_fill(fact)
    }

    pub fn record_admitted_entry_admission(
        &self,
        fact: AdmittedEntryAdmissionFact,
    ) -> Result<CommittedAdmission, RecordFailure> {
        self.recorder()?.record_admitted_entry_admission(fact)
    }

    pub fn record_rejected_entry_admission(
        &self,
        fact: RejectedEntryAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_rejected_entry_admission(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_risk_reducing_exit_admission(
        &self,
        fact: RiskReducingExitAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_risk_reducing_exit_admission(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_forced_reduction_admission(
        &self,
        fact: ForcedReductionAdmissionFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_forced_reduction_admission(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_capital_admission_rebuild(
        &self,
        fact: CapitalAdmissionRebuildFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.recorder()?.record_capital_admission_rebuild(fact)
    }

    pub fn record_venue_truth_capture_failure(
        &self,
        fact: VenueTruthCaptureFailureFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_venue_truth_capture_failure(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_venue_truth_divergence(
        &self,
        fact: VenueTruthDivergenceFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_venue_truth_divergence(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_loss_governor_halt(
        &self,
        fact: LossGovernorHaltFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_loss_governor_halt(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_submit_admission_order_reject(
        &self,
        fact: OrderRejectFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_submit_admission_order_reject(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    pub fn record_basket_admission_granted(
        &self,
        fact: BasketAdmissionGrantedFact,
    ) -> Result<CommittedAdmission, RecordFailure> {
        self.recorder()?.record_basket_admission_granted(fact)
    }

    #[cfg(feature = "test-current-evidence-inspection")]
    pub fn record_basket_admission_rejected(
        &self,
        fact: BasketAdmissionRejectedFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_basket_admission_rejected(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }
}

impl BasketAdmissionEvidence {
    pub fn record_basket_admission_granted(
        &self,
        fact: BasketAdmissionGrantedFact,
    ) -> Result<CommittedAdmission, RecordFailure> {
        self.recorder()?.record_basket_admission_granted(fact)
    }

    pub fn record_basket_admission_rejected(
        &self,
        fact: BasketAdmissionRejectedFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_basket_admission_rejected(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }
}

impl OrderExecutionEvidence {
    pub fn record_entry_order_intent(
        &self,
        fact: EntryOrderIntentFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.recorder()?.record_entry_order_intent(fact)
    }

    pub fn record_risk_reducing_exit_order_intent(
        &self,
        fact: RiskReducingExitOrderIntentFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_risk_reducing_exit_order_intent(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }
}

impl EdgeTakerEvidence {
    pub fn record_order_lifecycle(&self, fact: OrderLifecycleFact) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_order_lifecycle(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_entry_skip_observation(&self, fact: EntrySkipFact) -> ObservationRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_entry_skip_observation(fact),
            Err(error) => ObservationRecordOutcome::FailureReported(error),
        }
    }

    pub fn record_blocked_strategy_input_observation(
        &self,
        fact: BlockedStrategyInputObservationFact,
    ) -> ObservationRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_blocked_strategy_input_observation(fact),
            Err(error) => ObservationRecordOutcome::FailureReported(error),
        }
    }

    pub fn record_submit_linked_strategy_input_snapshot(
        &self,
        fact: SubmitLinkedStrategyInputSnapshotFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.recorder()?
            .record_submit_linked_strategy_input_snapshot(fact)
    }

    pub fn record_exit_submission_decision(
        &self,
        fact: ExitSubmissionDecisionFact,
    ) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_exit_submission_decision(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }

    pub fn record_exit_hold_decision(
        &self,
        fact: ExitHoldDecisionFact,
    ) -> ObservationRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_exit_hold_decision(fact),
            Err(error) => ObservationRecordOutcome::FailureReported(error),
        }
    }

    pub fn record_exit_evaluation(&self, fact: ExitEvaluationFact) -> ObservationRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_exit_evaluation(fact),
            Err(error) => ObservationRecordOutcome::FailureReported(error),
        }
    }

    pub fn record_settlement(
        &self,
        fact: SettlementFact,
    ) -> Result<CommittedSettlement, RecordFailure> {
        self.recorder()?.record_settlement(fact)
    }

    pub fn record_terminal_settlement(
        &self,
        fact: TerminalSettlementFact,
    ) -> Result<AppendReceipt, RecordFailure> {
        self.recorder()?.record_terminal_settlement(fact)
    }
}

impl MakerEvidence {
    pub fn record_requote_throttle_observation(
        &self,
        fact: RequoteThrottleObservationFact,
    ) -> ObservationRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_requote_throttle_observation(fact),
            Err(error) => ObservationRecordOutcome::FailureReported(error),
        }
    }
}

impl OrderRejectObserverEvidence {
    pub fn record_observed_order_reject(&self, fact: OrderRejectFact) -> NonBlockingRecordOutcome {
        match self.recorder() {
            Ok(recorder) => recorder.record_observed_order_reject(fact),
            Err(error) => NonBlockingRecordOutcome::Failed(error),
        }
    }
}
