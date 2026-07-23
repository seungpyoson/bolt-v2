mod admission;
mod basket_admission;
mod entry_skip;
mod exit;
mod lifecycle;
mod loss;
mod order_intent;
mod order_reject;
mod requote;
mod reservation;
mod settlement;
mod strategy_input;
mod venue_truth;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    facts::{
        AdmittedEntryAdmissionFact, BasketAdmissionGrantedFact, BasketAdmissionRejectedFact,
        BlockedStrategyInputObservationFact, CapitalAdmissionRebuildFact, CurrentFact,
        EntryOrderIntentFact, EntrySkipFact, ExitEvaluationFact, ExitHoldDecisionFact,
        ExitSubmissionDecisionFact, ForcedReductionAdmissionFact, LossGovernorHaltFact,
        OrderLifecycleFact, OrderRejectFact, RejectedEntryAdmissionFact,
        RequoteThrottleObservationFact, RiskReducingExitAdmissionFact,
        RiskReducingExitOrderIntentFact, SettlementFact, SubmitLinkedStrategyInputSnapshotFact,
        SubmitReservationFillFact, SubmitReservationMetadataFact, TerminalSettlementFact,
        VenueTruthCaptureFailureFact, VenueTruthDivergenceFact,
    },
    generated_contract::{
        IdentityDescriptor, KnownIdentity, KnownPurpose, current_identity_for_purpose,
        descriptor_for_identity, identities,
    },
    record::{EncodedEvidenceRecord, RecordFailure},
};

pub(crate) struct CurrentCodecs;

pub(crate) trait CodecFor<I> {
    type Input;
    type Fact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure>;

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact>;
}

impl CodecFor<identities::SubmitReservationMetadataV1> for CurrentCodecs {
    type Input = SubmitReservationMetadataFact;
    type Fact = SubmitReservationMetadataFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        reservation::encode_metadata(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        reservation::decode_metadata(line, line_number)
    }
}

impl CodecFor<identities::SubmitReservationFillV1> for CurrentCodecs {
    type Input = SubmitReservationFillFact;
    type Fact = SubmitReservationFillFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        reservation::encode_fill(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        reservation::decode_fill(line, line_number)
    }
}

impl CodecFor<identities::SettlementV1> for CurrentCodecs {
    type Input = SettlementFact;
    type Fact = SettlementFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        settlement::encode_settlement(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        settlement::decode_settlement(line, line_number)
    }
}

impl CodecFor<identities::TerminalSettlementV1> for CurrentCodecs {
    type Input = TerminalSettlementFact;
    type Fact = TerminalSettlementFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        settlement::encode_terminal(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        settlement::decode_terminal(line, line_number)
    }
}

impl CodecFor<identities::EntryOrderIntentV1> for CurrentCodecs {
    type Input = EntryOrderIntentFact;
    type Fact = EntryOrderIntentFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        order_intent::encode_entry(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        order_intent::decode_entry(line, line_number)
    }
}

impl CodecFor<identities::RiskReducingExitOrderIntentV1> for CurrentCodecs {
    type Input = RiskReducingExitOrderIntentFact;
    type Fact = RiskReducingExitOrderIntentFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        order_intent::encode_risk_reducing_exit(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        order_intent::decode_risk_reducing_exit(line, line_number)
    }
}

impl CodecFor<identities::BasketAdmissionGrantedV1> for CurrentCodecs {
    type Input = BasketAdmissionGrantedFact;
    type Fact = BasketAdmissionGrantedFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        basket_admission::encode_granted(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        basket_admission::decode_granted(line, line_number)
    }
}

impl CodecFor<identities::BasketAdmissionRejectedV1> for CurrentCodecs {
    type Input = BasketAdmissionRejectedFact;
    type Fact = BasketAdmissionRejectedFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        basket_admission::encode_rejected(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        basket_admission::decode_rejected(line, line_number)
    }
}

impl CodecFor<identities::CapitalAdmissionRebuildV1> for CurrentCodecs {
    type Input = CapitalAdmissionRebuildFact;
    type Fact = CapitalAdmissionRebuildFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        admission::encode_capital_rebuild(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        admission::decode_capital_rebuild(line, line_number)
    }
}

impl CodecFor<identities::OrderLifecycleV1> for CurrentCodecs {
    type Input = OrderLifecycleFact;
    type Fact = OrderLifecycleFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        lifecycle::encode(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        lifecycle::decode_fact(line, line_number)
    }
}

impl CodecFor<identities::RequoteThrottleObservationV1> for CurrentCodecs {
    type Input = RequoteThrottleObservationFact;
    type Fact = RequoteThrottleObservationFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        requote::encode(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        requote::decode_fact(line, line_number)
    }
}

impl CodecFor<identities::VenueTruthCaptureFailureV1> for CurrentCodecs {
    type Input = VenueTruthCaptureFailureFact;
    type Fact = VenueTruthCaptureFailureFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        venue_truth::encode_capture_failure(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        venue_truth::decode_capture_failure(line, line_number)
    }
}

impl CodecFor<identities::VenueTruthDivergenceV1> for CurrentCodecs {
    type Input = VenueTruthDivergenceFact;
    type Fact = VenueTruthDivergenceFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        venue_truth::encode_divergence(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        venue_truth::decode_divergence(line, line_number)
    }
}

impl CodecFor<identities::LossGovernorHaltV1> for CurrentCodecs {
    type Input = LossGovernorHaltFact;
    type Fact = LossGovernorHaltFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        loss::encode(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        loss::decode_fact(line, line_number)
    }
}

impl CodecFor<identities::OrderRejectV1> for CurrentCodecs {
    type Input = OrderRejectFact;
    type Fact = OrderRejectFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        order_reject::encode(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        order_reject::decode_fact(line, line_number)
    }
}

impl CodecFor<identities::EntrySkipObservationV1> for CurrentCodecs {
    type Input = EntrySkipFact;
    type Fact = EntrySkipFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        entry_skip::encode(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        entry_skip::decode_fact(line, line_number)
    }
}

impl CodecFor<identities::BlockedStrategyInputObservationV1> for CurrentCodecs {
    type Input = BlockedStrategyInputObservationFact;
    type Fact = BlockedStrategyInputObservationFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        strategy_input::encode_blocked(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        strategy_input::decode_blocked(line, line_number)
    }
}

impl CodecFor<identities::SubmitLinkedStrategyInputSnapshotV1> for CurrentCodecs {
    type Input = SubmitLinkedStrategyInputSnapshotFact;
    type Fact = SubmitLinkedStrategyInputSnapshotFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        strategy_input::encode_submit(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        strategy_input::decode_submit(line, line_number)
    }
}

impl CodecFor<identities::ExitSubmissionDecisionV1> for CurrentCodecs {
    type Input = ExitSubmissionDecisionFact;
    type Fact = ExitSubmissionDecisionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        exit::encode_submission(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        exit::decode_submission(line, line_number)
    }
}

impl CodecFor<identities::ExitHoldDecisionV1> for CurrentCodecs {
    type Input = ExitHoldDecisionFact;
    type Fact = ExitHoldDecisionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        exit::encode_hold(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        exit::decode_hold(line, line_number)
    }
}

impl CodecFor<identities::ExitEvaluationV1> for CurrentCodecs {
    type Input = ExitEvaluationFact;
    type Fact = ExitEvaluationFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        exit::encode_evaluation(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        exit::decode_evaluation(line, line_number)
    }
}

impl CodecFor<identities::AdmittedEntryAdmissionV1> for CurrentCodecs {
    type Input = AdmittedEntryAdmissionFact;
    type Fact = AdmittedEntryAdmissionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        admission::encode_admitted_entry(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        admission::decode_admitted_entry(line, line_number)
    }
}

impl CodecFor<identities::RejectedEntryAdmissionV1> for CurrentCodecs {
    type Input = RejectedEntryAdmissionFact;
    type Fact = RejectedEntryAdmissionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        admission::encode_rejected_entry(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        admission::decode_rejected_entry(line, line_number)
    }
}

impl CodecFor<identities::RiskReducingExitAdmissionV1> for CurrentCodecs {
    type Input = RiskReducingExitAdmissionFact;
    type Fact = RiskReducingExitAdmissionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        admission::encode_risk_reducing_exit(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        admission::decode_risk_reducing_exit(line, line_number)
    }
}

impl CodecFor<identities::ForcedReductionAdmissionV1> for CurrentCodecs {
    type Input = ForcedReductionAdmissionFact;
    type Fact = ForcedReductionAdmissionFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        admission::encode_forced_reduction(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        admission::decode_forced_reduction(line, line_number)
    }
}

pub(crate) fn encode_entry_order_intent(
    fact: EntryOrderIntentFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::EntryOrderIntentV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_admitted_entry_admission(
    fact: AdmittedEntryAdmissionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::AdmittedEntryAdmissionV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_rejected_entry_admission(
    fact: RejectedEntryAdmissionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::RejectedEntryAdmissionV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_risk_reducing_exit_admission(
    fact: RiskReducingExitAdmissionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::RiskReducingExitAdmissionV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_forced_reduction_admission(
    fact: ForcedReductionAdmissionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::ForcedReductionAdmissionV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_risk_reducing_exit_order_intent(
    fact: RiskReducingExitOrderIntentFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::RiskReducingExitOrderIntentV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_basket_admission_granted(
    fact: BasketAdmissionGrantedFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::BasketAdmissionGrantedV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_basket_admission_rejected(
    fact: BasketAdmissionRejectedFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::BasketAdmissionRejectedV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_capital_admission_rebuild(
    fact: CapitalAdmissionRebuildFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::CapitalAdmissionRebuildV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_order_lifecycle(
    fact: OrderLifecycleFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::OrderLifecycleV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_requote_throttle_observation(
    fact: RequoteThrottleObservationFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::RequoteThrottleObservationV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_venue_truth_capture_failure(
    fact: VenueTruthCaptureFailureFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::VenueTruthCaptureFailureV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_venue_truth_divergence(
    fact: VenueTruthDivergenceFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::VenueTruthDivergenceV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_loss_governor_halt(
    fact: LossGovernorHaltFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::LossGovernorHaltV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_order_reject(
    fact: OrderRejectFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::OrderRejectV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_entry_skip_observation(
    fact: EntrySkipFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::EntrySkipObservationV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_blocked_strategy_input_observation(
    fact: BlockedStrategyInputObservationFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::BlockedStrategyInputObservationV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_submit_linked_strategy_input_snapshot(
    fact: SubmitLinkedStrategyInputSnapshotFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::SubmitLinkedStrategyInputSnapshotV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_exit_submission_decision(
    fact: ExitSubmissionDecisionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::ExitSubmissionDecisionV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_exit_hold_decision(
    fact: ExitHoldDecisionFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::ExitHoldDecisionV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_exit_evaluation(
    fact: ExitEvaluationFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::ExitEvaluationV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_reservation_metadata(
    fact: SubmitReservationMetadataFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::SubmitReservationMetadataV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_reservation_fill(
    fact: SubmitReservationFillFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::SubmitReservationFillV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_settlement(
    fact: SettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::SettlementV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn encode_terminal_settlement(
    fact: TerminalSettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::TerminalSettlementV1>>::encode(&fact, current_utc_ns()?)
}

pub(super) fn decode_current_fact(
    identity: KnownIdentity,
    line: &str,
    line_number: usize,
) -> Result<CurrentFact> {
    let header: GateVersionHeader = decode(line, line_number)?;
    validate_gate_version(&header.gate_version, line_number)?;
    Ok(match identity {
        KnownIdentity::BlockedStrategyInputObservationV1 => {
            CurrentFact::BlockedStrategyInputObservation(Box::new(<CurrentCodecs as CodecFor<
                identities::BlockedStrategyInputObservationV1,
            >>::decode(
                line, line_number
            )?))
        }
        KnownIdentity::SubmitLinkedStrategyInputSnapshotV1 => {
            CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(<CurrentCodecs as CodecFor<
                identities::SubmitLinkedStrategyInputSnapshotV1,
            >>::decode(
                line, line_number
            )?))
        }
        KnownIdentity::EntryOrderIntentV1 => {
            CurrentFact::EntryOrderIntent(<CurrentCodecs as CodecFor<
                identities::EntryOrderIntentV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::RiskReducingExitOrderIntentV1 => {
            CurrentFact::RiskReducingExitOrderIntent(<CurrentCodecs as CodecFor<
                identities::RiskReducingExitOrderIntentV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::AdmittedEntryAdmissionV1 => {
            CurrentFact::AdmittedEntryAdmission(Box::new(<CurrentCodecs as CodecFor<
                identities::AdmittedEntryAdmissionV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::RejectedEntryAdmissionV1 => {
            CurrentFact::RejectedEntryAdmission(Box::new(<CurrentCodecs as CodecFor<
                identities::RejectedEntryAdmissionV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::RiskReducingExitAdmissionV1 => {
            CurrentFact::RiskReducingExitAdmission(Box::new(<CurrentCodecs as CodecFor<
                identities::RiskReducingExitAdmissionV1,
            >>::decode(
                line, line_number
            )?))
        }
        KnownIdentity::ForcedReductionAdmissionV1 => {
            CurrentFact::ForcedReductionAdmission(Box::new(<CurrentCodecs as CodecFor<
                identities::ForcedReductionAdmissionV1,
            >>::decode(
                line, line_number
            )?))
        }
        KnownIdentity::BasketAdmissionGrantedV1 => {
            CurrentFact::BasketAdmissionGranted(<CurrentCodecs as CodecFor<
                identities::BasketAdmissionGrantedV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::BasketAdmissionRejectedV1 => {
            CurrentFact::BasketAdmissionRejected(<CurrentCodecs as CodecFor<
                identities::BasketAdmissionRejectedV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::CapitalAdmissionRebuildV1 => {
            CurrentFact::CapitalAdmissionRebuild(<CurrentCodecs as CodecFor<
                identities::CapitalAdmissionRebuildV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::SubmitReservationMetadataV1 => {
            CurrentFact::SubmitReservationMetadata(<CurrentCodecs as CodecFor<
                identities::SubmitReservationMetadataV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::SubmitReservationFillV1 => {
            CurrentFact::SubmitReservationFill(<CurrentCodecs as CodecFor<
                identities::SubmitReservationFillV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::EntrySkipObservationV1 => {
            CurrentFact::EntrySkipObservation(Box::new(<CurrentCodecs as CodecFor<
                identities::EntrySkipObservationV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::ExitSubmissionDecisionV1 => {
            CurrentFact::ExitSubmissionDecision(Box::new(<CurrentCodecs as CodecFor<
                identities::ExitSubmissionDecisionV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::ExitHoldDecisionV1 => {
            CurrentFact::ExitHoldDecision(Box::new(<CurrentCodecs as CodecFor<
                identities::ExitHoldDecisionV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::ExitEvaluationV1 => {
            CurrentFact::ExitEvaluation(Box::new(<CurrentCodecs as CodecFor<
                identities::ExitEvaluationV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::LossGovernorHaltV1 => {
            CurrentFact::LossGovernorHalt(<CurrentCodecs as CodecFor<
                identities::LossGovernorHaltV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::OrderRejectV1 => {
            CurrentFact::OrderReject(Box::new(<CurrentCodecs as CodecFor<
                identities::OrderRejectV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::OrderLifecycleV1 => {
            CurrentFact::OrderLifecycle(<CurrentCodecs as CodecFor<
                identities::OrderLifecycleV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::RequoteThrottleObservationV1 => {
            CurrentFact::RequoteThrottleObservation(<CurrentCodecs as CodecFor<
                identities::RequoteThrottleObservationV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::SettlementV1 => {
            CurrentFact::Settlement(
                <CurrentCodecs as CodecFor<identities::SettlementV1>>::decode(line, line_number)?,
            )
        }
        KnownIdentity::TerminalSettlementV1 => {
            CurrentFact::TerminalSettlement(Box::new(<CurrentCodecs as CodecFor<
                identities::TerminalSettlementV1,
            >>::decode(line, line_number)?))
        }
        KnownIdentity::VenueTruthCaptureFailureV1 => {
            CurrentFact::VenueTruthCaptureFailure(<CurrentCodecs as CodecFor<
                identities::VenueTruthCaptureFailureV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::VenueTruthDivergenceV1 => {
            CurrentFact::VenueTruthDivergence(<CurrentCodecs as CodecFor<
                identities::VenueTruthDivergenceV1,
            >>::decode(line, line_number)?)
        }
    })
}

fn current_line_descriptor(purpose: KnownPurpose) -> IdentityDescriptor {
    descriptor_for_identity(current_identity_for_purpose(purpose))
}

fn current_utc_ns() -> Result<i64, RecordFailure> {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            RecordFailure::Rejected(anyhow::anyhow!(
                "current UTC timestamp is outside the supported positive i64 domain"
            ))
        })
}

fn encode_line(
    purpose: KnownPurpose,
    line: &impl Serialize,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    let mut bytes = serde_json::to_vec(line).map_err(|error| {
        RecordFailure::Rejected(anyhow::anyhow!(
            "current evidence serialization failed: {error}"
        ))
    })?;
    bytes.push(b'\n');
    EncodedEvidenceRecord::try_new(purpose, bytes)
}

fn validate_nonempty<'a>(
    label: &str,
    values: impl IntoIterator<Item = &'a str>,
    observed_at_ns: u64,
) -> Result<(), RecordFailure> {
    if values.into_iter().any(|value| value.trim().is_empty()) || observed_at_ns == 0 {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "{label} fields must be nonempty and observed_at_ns must be positive"
        )));
    }
    Ok(())
}

fn validate_recorded_at(recorded_at_utc_ns: i64) -> Result<(), RecordFailure> {
    if recorded_at_utc_ns <= 0 {
        return Err(RecordFailure::Rejected(anyhow::anyhow!(
            "recorded_at_utc_ns must be positive"
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
struct GateVersionHeader {
    gate_version: String,
}

pub(super) fn validate_gate_version(gate_version: &str, line_number: usize) -> Result<()> {
    ensure!(
        !gate_version.trim().is_empty(),
        "gate_version must be non-empty at current evidence line {line_number}"
    );
    Ok(())
}

fn decode<'a, T: Deserialize<'a>>(line: &'a str, line_number: usize) -> Result<T> {
    serde_json::from_str(line).with_context(|| {
        format!("malformed current payload at current evidence line {line_number}")
    })
}

fn validate_envelope(
    identity: KnownIdentity,
    kind: &str,
    schema_version: u32,
    gate_id: &str,
    recorded_at_utc_ns: i64,
    line_number: usize,
) -> Result<()> {
    let descriptor = descriptor_for_identity(identity);
    ensure!(
        kind == descriptor.kind && schema_version == descriptor.schema_version,
        "identity mismatch at current evidence line {line_number}"
    );
    ensure!(
        gate_id == descriptor.gate_id,
        "wrong gate_id at current evidence line {line_number}"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive at current evidence line {line_number}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::facts::*;
    use super::*;

    macro_rules! unit_wire_coverage {
        ($type:ty, [$($variant:ident => $wire:literal),+ $(,)?]) => {
            impl $type {
                fn wire_coverage_values() -> Vec<(Self, &'static str)> {
                    let values = vec![$((Self::$variant, $wire)),+];
                    for (value, _) in &values {
                        match value {
                            $(Self::$variant => {}),+
                        }
                    }
                    values
                }
            }
        };
    }

    macro_rules! payload_wire_coverage {
        (
            $type:ty,
            [$($value:expr => $pattern:pat => $wire:literal),+ $(,)?]
        ) => {
            impl $type {
                fn wire_coverage_values() -> Vec<(Self, &'static str)> {
                    let values = vec![$(($value, $wire)),+];
                    for (value, _) in &values {
                        match value {
                            $($pattern => {}),+
                        }
                    }
                    values
                }
            }
        };
    }

    unit_wire_coverage!(
        OrderIntentClampNotEvaluatedReason,
        [
            NoVenueTruth => "no_venue_truth",
            ForeignInstrument => "foreign_instrument",
            NonSellOrderSide => "non_sell_order_side"
        ]
    );
    payload_wire_coverage!(
        OrderIntentClampOutcome,
        [
            Self::WithinBounds => Self::WithinBounds => "within_bounds",
            Self::Clamped { original_quantity: "2".to_string() } =>
                Self::Clamped { .. } => "clamped",
            Self::Rejected => Self::Rejected => "rejected",
            Self::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            } => Self::NotEvaluated { .. } => "not_evaluated"
        ]
    );
    unit_wire_coverage!(
        BasketAdmissionRejectionReason,
        [
            BasketNotionalCapExceeded => "basket_notional_cap_exceeded",
            MaxOpenBasketCapExceeded => "max_open_basket_cap_exceeded",
            StaleScannerEvidence => "stale_scanner_evidence",
            StaleSubmitRecheck => "stale_submit_recheck",
            NonPositiveCandidateCost => "non_positive_candidate_cost",
            NonPositiveEdge => "non_positive_edge",
            EdgeThreshold => "edge_threshold",
            MissingGroupingProof => "missing_grouping_proof",
            MissingSettlementRules => "missing_settlement_rules",
            RetryBudgetExceeded => "retry_budget_exceeded",
            SubmitSlots => "submit_slots"
        ]
    );
    unit_wire_coverage!(
        CapitalAdmissionRejectionReason,
        [
            MissingEvidence => "missing_evidence",
            StaleRequest => "stale_request",
            PoolMismatch => "pool_mismatch",
            OverBudget => "over_budget",
            InvalidRequest => "invalid_request",
            CollateralGroupMismatch => "collateral_group_mismatch",
            DuplicateReservation => "duplicate_reservation",
            UnknownReservation => "unknown_reservation",
            UnknownRelease => "unknown_release",
            ReconciliationRequired => "reconciliation_required"
        ]
    );
    payload_wire_coverage!(
        CapitalAdmissionRebuildOutcome,
        [
            Self::Accepted => Self::Accepted => "accepted",
            Self::Rejected(CapitalAdmissionRejectionReason::MissingEvidence) =>
                Self::Rejected(_) => "rejected"
        ]
    );
    unit_wire_coverage!(
        RequoteActionCostClass,
        [
            FreshSubmit => "fresh_submit",
            CancelResubmit => "cancel_resubmit",
            Cancel => "cancel"
        ]
    );
    unit_wire_coverage!(
        RequoteThrottleBound,
        [
            SubmitCommandWindow => "submit_command_window",
            RestCallWindow => "rest_call_window",
            MinInterval => "min_interval",
            WindowCap => "window_cap",
            OutOfOrderTs => "out_of_order_ts",
            Overflow => "overflow"
        ]
    );
    unit_wire_coverage!(
        RequoteThrottleBlockReason,
        [RequoteBudgetExhausted => "requote_budget_exhausted"]
    );
    unit_wire_coverage!(
        VenueTruthDivergenceAlarmClass,
        [
            TrueDivergence => "true_divergence",
            OrderingViolation => "ordering_violation",
            SilentChannel => "silent_channel"
        ]
    );
    unit_wire_coverage!(
        StaleLossReason,
        [
            MissingSnapshot => "missing_snapshot",
            SourceEmpty => "source_empty",
            FutureDated => "future_dated",
            AgeExceeded => "age_exceeded",
            MissingRequiredField => "missing_required_field"
        ]
    );
    unit_wire_coverage!(
        LossHaltReason,
        [
            PerTradeLossLimit => "per_trade_loss_limit",
            DailyLossLimit => "daily_loss_limit",
            RollingLossLimit => "rolling_loss_limit",
            MaxDrawdownLimit => "max_drawdown_limit",
            StaleLossSnapshot => "stale_loss_snapshot"
        ]
    );
    unit_wire_coverage!(
        LossSnapshotSource,
        [
            NtLossRuntimeFeed => "nt_loss_runtime_feed",
            NtPortfolioSnapshot => "nt_portfolio_snapshot",
            NtAccountSnapshot => "nt_account_snapshot",
            NtAccountAndPositionSnapshot => "nt_account_and_position_snapshot",
            NtPositionEvent => "nt_position_event",
            NtPositionChanged => "nt_position_changed",
            NtPositionClosed => "nt_position_closed",
            NtPositionAdjusted => "nt_position_adjusted",
            NtCapitalAdmissionState => "nt_capital_admission_state",
            BoltLossSnapshot => "bolt_loss_snapshot",
            LossGovernor => "loss_governor",
            Unknown => "unknown",
            Other => "other"
        ]
    );
    unit_wire_coverage!(
        LossSnapshotStaleReason,
        [
            MissingSnapshot => "missing_snapshot",
            SourceEmpty => "source_empty",
            FutureDated => "future_dated",
            AgeExceeded => "age_exceeded",
            MissingRequiredField => "missing_required_field"
        ]
    );
    unit_wire_coverage!(
        AdmissionRejectionReason,
        [
            KillSwitchLatched => "kill_switch_latched",
            LossGovernorHalted => "loss_governor_halted",
            NonPositiveNotional => "non_positive_notional",
            NotionalCapExceeded => "notional_cap_exceeded",
            InvalidRiskReducingExitProof => "invalid_risk_reducing_exit_proof",
            CountCapExhausted => "count_cap_exhausted",
            KillSwitchForcedReductionProofInvalid =>
                "kill_switch_forced_reduction_proof_invalid",
            KillSwitchForcedReductionCapExceeded =>
                "kill_switch_forced_reduction_cap_exceeded",
            CapitalAdmission => "capital_admission"
        ]
    );
    payload_wire_coverage!(
        AdmissionDecisionOutcome,
        [
            Self::Admitted => Self::Admitted => "admitted",
            Self::Rejected(AdmissionRejectionReason::NotionalCapExceeded) =>
                Self::Rejected(_) => "rejected"
        ]
    );
    unit_wire_coverage!(
        OrderRejectSource,
        [
            SubmitAdmission => "submit_admission",
            Venue => "venue",
            NtExecution => "nt_execution",
            Internal => "internal"
        ]
    );
    unit_wire_coverage!(
        OrderRejectReason,
        [
            AdmissionRejected => "admission_rejected",
            PrecisionRejected => "precision_rejected",
            MinSizeRejected => "min_size_rejected",
            MinNotionalRejected => "min_notional_rejected",
            InsufficientBalance => "insufficient_balance",
            DuplicateClientOrderId => "duplicate_client_order_id",
            Other => "other"
        ]
    );
    unit_wire_coverage!(
        EntrySkipReason,
        [
            StrategyCoreNotRegistered => "strategy_core_not_registered",
            EntryGateBlocked => "entry_gate_blocked",
            EntryPricingBlocked => "entry_pricing_blocked",
            NoSideSelected => "no_side_selected",
            SizedNotionalNotPositive => "sized_notional_not_positive",
            InstrumentIdMissing => "instrument_id_missing",
            InstrumentMissingFromCache => "instrument_missing_from_cache",
            EntryPriceMissing => "entry_price_missing",
            QuantityRoundingFailed => "quantity_rounding_failed",
            LimitNotionalExceedsSizedNotional => "limit_notional_exceeds_sized_notional",
            EntryQuoteNotionalBelowVenueMinimum => "entry_quote_notional_below_venue_minimum",
            EntryQuoteNotionalMinimumUnmodeled => "entry_quote_notional_minimum_unmodeled",
            QuantityNotPositive => "quantity_not_positive",
            PositionContractInvalid => "position_contract_invalid",
            EntryPositionContractUnsupported => "entry_position_contract_unsupported",
            HistoricalEntryFeeUnavailable => "historical_entry_fee_unavailable",
            OnePositionInvariantViolation => "one_position_invariant_violation",
            EntryMalformedRejected => "entry_malformed_rejected",
            EntryBalanceRejected => "entry_balance_rejected",
            EntryUnfillableRejectedUnchangedBook =>
                "entry_unfillable_rejected_unchanged_book"
        ]
    );
    unit_wire_coverage!(
        ForcedFlatReason,
        [
            Freeze => "freeze",
            StaleReference => "stale_reference",
            ThinBook => "thin_book",
            MetadataMismatch => "metadata_mismatch",
            FastVenueIncoherent => "fast_venue_incoherent"
        ]
    );
    unit_wire_coverage!(
        ExposureOccupancy,
        [
            PendingEntry => "pending_entry",
            EntryReconcilePending => "entry_reconcile_pending",
            ManagedPosition => "managed_position",
            ExitPending => "exit_pending",
            UnsupportedObserved => "unsupported_observed",
            BlindRecovery => "blind_recovery"
        ]
    );
    payload_wire_coverage!(
        EntryBlockReason,
        [
            Self::PhaseNotActive => Self::PhaseNotActive => "phase_not_active",
            Self::MetadataMismatch => Self::MetadataMismatch => "metadata_mismatch",
            Self::ActiveBookNotPriced => Self::ActiveBookNotPriced => "active_book_not_priced",
            Self::BookCrossed => Self::BookCrossed => "book_crossed",
            Self::IntervalOpenMissing => Self::IntervalOpenMissing => "interval_open_missing",
            Self::WarmupIncomplete => Self::WarmupIncomplete => "warmup_incomplete",
            Self::FeesNotReady => Self::FeesNotReady => "fees_not_ready",
            Self::RecoveryMode => Self::RecoveryMode => "recovery_mode",
            Self::MarketCoolingDown => Self::MarketCoolingDown => "market_cooling_down",
            Self::SpotSpikeCooldown => Self::SpotSpikeCooldown => "spot_spike_cooldown",
            Self::ForcedFlat(ForcedFlatReason::Freeze) =>
                Self::ForcedFlat(_) => "forced_flat",
            Self::OnePositionInvariant(ExposureOccupancy::PendingEntry) =>
                Self::OnePositionInvariant(_) => "one_position_invariant"
        ]
    );
    unit_wire_coverage!(
        BinaryOutcomeEdgeBlockReason,
        [
            MissingOrderBook => "missing_order_book",
            InsufficientDepth => "insufficient_depth",
            InvalidProbability => "invalid_probability",
            InvalidCost => "invalid_cost",
            UnsupportedOrderShape => "unsupported_order_shape",
            EdgeBelowThreshold => "edge_below_threshold",
            SpreadOrSlippageWipedEdge => "spread_or_slippage_wiped_edge",
            FeeUnavailable => "fee_unavailable"
        ]
    );
    payload_wire_coverage!(
        EntryPricingBlockReason,
        [
            Self::SpotPriceMissing => Self::SpotPriceMissing => "spot_price_missing",
            Self::ReferenceCurrentPriceStale =>
                Self::ReferenceCurrentPriceStale => "reference_current_price_stale",
            Self::StrikePriceMissing => Self::StrikePriceMissing => "strike_price_missing",
            Self::SecondsToExpiryMissing =>
                Self::SecondsToExpiryMissing => "seconds_to_expiry_missing",
            Self::RealizedVolNotReady => Self::RealizedVolNotReady => "realized_vol_not_ready",
            Self::ThetaScalerUnavailable =>
                Self::ThetaScalerUnavailable => "theta_scaler_unavailable",
            Self::UncertaintyBandUnavailable =>
                Self::UncertaintyBandUnavailable => "uncertainty_band_unavailable",
            Self::FairProbabilityUnavailable =>
                Self::FairProbabilityUnavailable => "fair_probability_unavailable",
            Self::FeeUnavailable(OutcomeSide::Up) =>
                Self::FeeUnavailable(_) => "fee_unavailable",
            Self::ExecutableEntryCostUnavailable(OutcomeSide::Up) =>
                Self::ExecutableEntryCostUnavailable(_) => "executable_entry_cost_unavailable",
            Self::ExecutableEdgeUnavailable(
                OutcomeSide::Up,
                BinaryOutcomeEdgeBlockReason::MissingOrderBook,
            ) => Self::ExecutableEdgeUnavailable(_, _) => "executable_edge_unavailable",
            Self::SizedNotionalUnsupported(OutcomeSide::Up) =>
                Self::SizedNotionalUnsupported(_) => "sized_notional_unsupported"
        ]
    );
    unit_wire_coverage!(
        RealizedVolPricingComponent,
        [
            Measured => "measured",
            NoiseRobust => "noise_robust",
            Continuous => "continuous",
            Forecast => "forecast"
        ]
    );
    unit_wire_coverage!(
        RealizedVolAggregation,
        [
            UpperQuantile => "upper_quantile",
            Median => "median",
            TrimmedMean => "trimmed_mean",
            MedianWithUpperQuantileGuard => "median_with_upper_quantile_guard"
        ]
    );
    unit_wire_coverage!(
        RealizedVolSourceClass,
        [
            SpotQuote => "spot_quote",
            Trade => "trade",
            Mark => "mark",
            Index => "index"
        ]
    );
    unit_wire_coverage!(
        RealizedVolSampleKind,
        [
            Midpoint => "midpoint",
            Trade => "trade",
            Mark => "mark",
            Index => "index"
        ]
    );
    unit_wire_coverage!(
        RealizedVolSourceStatus,
        [
            Ready => "ready",
            Blocked => "blocked",
            DiagnosticOnly => "diagnostic_only",
            Waiting => "waiting"
        ]
    );
    unit_wire_coverage!(
        RealizedVolSourceRejectReason,
        [
            DisabledSource => "disabled_source",
            InvalidPrice => "invalid_price",
            SourceClassMismatch => "source_class_mismatch",
            SampleKindMismatch => "sample_kind_mismatch",
            EventTimeRegression => "event_time_regression",
            DuplicateTimestamp => "duplicate_timestamp",
            StaleSameEventUpdate => "stale_same_event_update",
            ReceiveBeforeEvent => "receive_before_event",
            EventReceiveLagExceeded => "event_receive_lag_exceeded"
        ]
    );
    unit_wire_coverage!(
        RealizedVolBlockReason,
        [
            InvalidConfig => "invalid_config",
            QuorumNotReady => "quorum_not_ready",
            SourceStale => "source_stale",
            CoverageBelowMinimum => "coverage_below_minimum",
            InterSampleGapExceeded => "inter_sample_gap_exceeded",
            SourceClassMismatch => "source_class_mismatch",
            SampleKindMismatch => "sample_kind_mismatch",
            CrossSourceDispersion => "cross_source_dispersion",
            AnnualizationBasisInvalid => "annualization_basis_invalid",
            NotWarm => "not_warm"
        ]
    );
    unit_wire_coverage!(
        RvGateResult,
        [
            Accepted => "accepted",
            MissingSnapshot => "missing_snapshot",
            MissingEvaluationEventTime => "missing_evaluation_event_time",
            RejectedFutureDated => "rejected_future_dated",
            RejectedStale => "rejected_stale",
            RejectedNotReady => "rejected_not_ready"
        ]
    );
    unit_wire_coverage!(
        StrategyInputMarketSelectionOutcome,
        [Current => "current", Next => "next"]
    );
    unit_wire_coverage!(
        ExitTriggerSource,
        [
            SignalQuote => "signal_quote",
            ReferenceUpdate => "reference_update",
            SelectionUpdate => "selection_update",
            BookDelta => "book_delta",
            Unknown => "unknown",
            Other => "other"
        ]
    );
    unit_wire_coverage!(
        ExitBlockedReason,
        [
            NoOpenPosition => "no_open_position",
            ExitAlreadyPending => "exit_already_pending",
            EntryOrderStillWorking => "entry_order_still_working",
            ExitDecisionUnavailable => "exit_decision_unavailable",
            ExitHold => "exit_hold",
            PositionIntervalEnded => "position_interval_ended",
            PositionIntervalUnknown => "position_interval_unknown",
            OpenPositionMissing => "open_position_missing",
            ExitOrderConfigInvalid => "exit_order_config_invalid",
            ExitQuoteQuantityUnsupported => "exit_quote_quantity_unsupported",
            ExitPriceMissing => "exit_price_missing",
            ExitQuantityNotPositive => "exit_quantity_not_positive"
        ]
    );
    unit_wire_coverage!(
        ExitSubmissionOutcome,
        [Exit => "exit", ExitFailClosed => "exit_fail_closed"]
    );
    unit_wire_coverage!(
        ExitHoldOutcome,
        [Hold => "hold", Blocked => "blocked"]
    );
    payload_wire_coverage!(
        ExitEvaluationDecision,
        [
            Self::Submission {
                outcome: ExitSubmissionOutcome::Exit,
            } => Self::Submission { .. } => "submit",
            Self::Hold {
                outcome: ExitHoldOutcome::Hold,
                blocked_reason: None,
            } => Self::Hold { .. } => "hold"
        ]
    );
    unit_wire_coverage!(OutcomeSide, [Up => "up", Down => "down"]);
    unit_wire_coverage!(
        SettlementBookingErrorReason,
        [
            ResolutionFeedMissing => "resolution_feed_missing",
            SettlementAlreadyBooked => "settlement_already_booked",
            SettlementInputInvalid => "settlement_input_invalid",
            SettlementBlocked => "settlement_blocked"
        ]
    );
    unit_wire_coverage!(
        OrderLifecycleTransition,
        [
            BoundaryReclassification => "boundary_reclassification",
            EntryFillMaterialized => "entry_fill_materialized",
            EntryReconcilePending => "entry_reconcile_pending",
            PositionTruthRematerialized => "position_truth_rematerialized",
            PositionClosed => "position_closed",
            ResidualRemanaged => "residual_remanaged",
            RestartOpenOrderAdopted => "restart_open_order_adopted",
            RestartOpenOrderRecoveryBlocked => "restart_open_order_recovery_blocked",
            SettlementEvidenceRecoveryBlocked => "settlement_evidence_recovery_blocked",
            SettlementBookingTerminal => "settlement_booking_terminal",
            OrderDenied => "order_denied",
            OrderRejected => "order_rejected",
            OrderCanceled => "order_canceled",
            OrderExpired => "order_expired",
            OrderFilled => "order_filled",
            ReconcileQueryFailed => "reconcile_query_failed"
        ]
    );
    unit_wire_coverage!(
        OrderLifecycleOutcome,
        [
            PendingEntry => "pending_entry",
            Managed => "managed",
            ExitPending => "exit_pending",
            EntryReconcilePending => "entry_reconcile_pending",
            UnsupportedObserved => "unsupported_observed",
            BlindRecovery => "blind_recovery",
            Flat => "flat"
        ]
    );

    macro_rules! frozen_corpus {
        ($file:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/current_evidence/positive/",
                $file
            ))
        };
    }

    fn positive_corpus(identity: KnownIdentity) -> &'static str {
        match identity {
            KnownIdentity::BlockedStrategyInputObservationV1 => {
                frozen_corpus!("blocked_strategy_input_observation.jsonl")
            }
            KnownIdentity::SubmitLinkedStrategyInputSnapshotV1 => {
                frozen_corpus!("submit_linked_strategy_input_snapshot.jsonl")
            }
            KnownIdentity::EntryOrderIntentV1 => frozen_corpus!("entry_order_intent.jsonl"),
            KnownIdentity::RiskReducingExitOrderIntentV1 => {
                frozen_corpus!("risk_reducing_exit_order_intent.jsonl")
            }
            KnownIdentity::AdmittedEntryAdmissionV1 => {
                frozen_corpus!("admitted_entry_admission.jsonl")
            }
            KnownIdentity::RejectedEntryAdmissionV1 => {
                frozen_corpus!("rejected_entry_admission.jsonl")
            }
            KnownIdentity::RiskReducingExitAdmissionV1 => {
                frozen_corpus!("risk_reducing_exit_admission.jsonl")
            }
            KnownIdentity::ForcedReductionAdmissionV1 => {
                frozen_corpus!("forced_reduction_admission.jsonl")
            }
            KnownIdentity::BasketAdmissionGrantedV1 => {
                frozen_corpus!("basket_admission_granted.jsonl")
            }
            KnownIdentity::BasketAdmissionRejectedV1 => {
                frozen_corpus!("basket_admission_rejected.jsonl")
            }
            KnownIdentity::CapitalAdmissionRebuildV1 => {
                frozen_corpus!("capital_admission_rebuild.jsonl")
            }
            KnownIdentity::SubmitReservationMetadataV1 => {
                frozen_corpus!("submit_reservation_metadata.jsonl")
            }
            KnownIdentity::SubmitReservationFillV1 => {
                frozen_corpus!("submit_reservation_fill.jsonl")
            }
            KnownIdentity::EntrySkipObservationV1 => {
                frozen_corpus!("entry_skip_observation.jsonl")
            }
            KnownIdentity::ExitSubmissionDecisionV1 => {
                frozen_corpus!("exit_submission_decision.jsonl")
            }
            KnownIdentity::ExitHoldDecisionV1 => frozen_corpus!("exit_hold_decision.jsonl"),
            KnownIdentity::ExitEvaluationV1 => frozen_corpus!("exit_evaluation.jsonl"),
            KnownIdentity::LossGovernorHaltV1 => frozen_corpus!("loss_governor_halt.jsonl"),
            KnownIdentity::OrderRejectV1 => frozen_corpus!("order_reject.jsonl"),
            KnownIdentity::OrderLifecycleV1 => frozen_corpus!("order_lifecycle.jsonl"),
            KnownIdentity::RequoteThrottleObservationV1 => {
                frozen_corpus!("requote_throttle_observation.jsonl")
            }
            KnownIdentity::SettlementV1 => frozen_corpus!("settlement.jsonl"),
            KnownIdentity::TerminalSettlementV1 => frozen_corpus!("terminal_settlement.jsonl"),
            KnownIdentity::VenueTruthCaptureFailureV1 => {
                frozen_corpus!("venue_truth_capture_failure.jsonl")
            }
            KnownIdentity::VenueTruthDivergenceV1 => {
                frozen_corpus!("venue_truth_divergence.jsonl")
            }
        }
    }

    macro_rules! accepted_noncanonical_fixture {
        ($file:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/bolt_v3/current_evidence/accepted_noncanonical/",
                $file
            ))
        };
    }

    fn accepted_noncanonical_corpus(identity: KnownIdentity) -> Option<&'static str> {
        match identity {
            KnownIdentity::BlockedStrategyInputObservationV1 => Some(
                accepted_noncanonical_fixture!("blocked_strategy_input_observation.jsonl"),
            ),
            KnownIdentity::SubmitLinkedStrategyInputSnapshotV1 => Some(
                accepted_noncanonical_fixture!("submit_linked_strategy_input_snapshot.jsonl"),
            ),
            KnownIdentity::EntryOrderIntentV1 => {
                Some(accepted_noncanonical_fixture!("entry_order_intent.jsonl"))
            }
            KnownIdentity::RiskReducingExitOrderIntentV1 => Some(accepted_noncanonical_fixture!(
                "risk_reducing_exit_order_intent.jsonl"
            )),
            KnownIdentity::AdmittedEntryAdmissionV1 => Some(accepted_noncanonical_fixture!(
                "admitted_entry_admission.jsonl"
            )),
            KnownIdentity::RejectedEntryAdmissionV1 => Some(accepted_noncanonical_fixture!(
                "rejected_entry_admission.jsonl"
            )),
            KnownIdentity::RiskReducingExitAdmissionV1 => Some(accepted_noncanonical_fixture!(
                "risk_reducing_exit_admission.jsonl"
            )),
            KnownIdentity::ForcedReductionAdmissionV1 => Some(accepted_noncanonical_fixture!(
                "forced_reduction_admission.jsonl"
            )),
            KnownIdentity::EntrySkipObservationV1 => Some(accepted_noncanonical_fixture!(
                "entry_skip_observation.jsonl"
            )),
            KnownIdentity::ExitSubmissionDecisionV1 => Some(accepted_noncanonical_fixture!(
                "exit_submission_decision.jsonl"
            )),
            KnownIdentity::ExitHoldDecisionV1 => {
                Some(accepted_noncanonical_fixture!("exit_hold_decision.jsonl"))
            }
            KnownIdentity::ExitEvaluationV1 => {
                Some(accepted_noncanonical_fixture!("exit_evaluation.jsonl"))
            }
            KnownIdentity::LossGovernorHaltV1 => {
                Some(accepted_noncanonical_fixture!("loss_governor_halt.jsonl"))
            }
            KnownIdentity::OrderRejectV1 => {
                Some(accepted_noncanonical_fixture!("order_reject.jsonl"))
            }
            KnownIdentity::OrderLifecycleV1 => {
                Some(accepted_noncanonical_fixture!("order_lifecycle.jsonl"))
            }
            KnownIdentity::RequoteThrottleObservationV1 => Some(accepted_noncanonical_fixture!(
                "requote_throttle_observation.jsonl"
            )),
            KnownIdentity::TerminalSettlementV1 => {
                Some(accepted_noncanonical_fixture!("terminal_settlement.jsonl"))
            }
            KnownIdentity::BasketAdmissionGrantedV1
            | KnownIdentity::BasketAdmissionRejectedV1
            | KnownIdentity::CapitalAdmissionRebuildV1
            | KnownIdentity::SubmitReservationMetadataV1
            | KnownIdentity::SubmitReservationFillV1
            | KnownIdentity::SettlementV1
            | KnownIdentity::VenueTruthCaptureFailureV1
            | KnownIdentity::VenueTruthDivergenceV1 => None,
        }
    }

    fn reencode_current_fact(
        fact: CurrentFact,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        match fact {
            CurrentFact::BlockedStrategyInputObservation(value) => {
                <CurrentCodecs as CodecFor<identities::BlockedStrategyInputObservationV1>>::encode(
                    value.as_ref(),
                    recorded_at_utc_ns,
                )
            }
            CurrentFact::SubmitLinkedStrategyInputSnapshot(value) => {
                <CurrentCodecs as CodecFor<identities::SubmitLinkedStrategyInputSnapshotV1>>::encode(
                    value.as_ref(),
                    recorded_at_utc_ns,
                )
            }
            CurrentFact::EntryOrderIntent(value) => <CurrentCodecs as CodecFor<
                identities::EntryOrderIntentV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::RiskReducingExitOrderIntent(value) => <CurrentCodecs as CodecFor<
                identities::RiskReducingExitOrderIntentV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::AdmittedEntryAdmission(value) => <CurrentCodecs as CodecFor<
                identities::AdmittedEntryAdmissionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::RejectedEntryAdmission(value) => <CurrentCodecs as CodecFor<
                identities::RejectedEntryAdmissionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::RiskReducingExitAdmission(value) => <CurrentCodecs as CodecFor<
                identities::RiskReducingExitAdmissionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::ForcedReductionAdmission(value) => <CurrentCodecs as CodecFor<
                identities::ForcedReductionAdmissionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::BasketAdmissionGranted(value) => <CurrentCodecs as CodecFor<
                identities::BasketAdmissionGrantedV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::BasketAdmissionRejected(value) => <CurrentCodecs as CodecFor<
                identities::BasketAdmissionRejectedV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::CapitalAdmissionRebuild(value) => <CurrentCodecs as CodecFor<
                identities::CapitalAdmissionRebuildV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::SubmitReservationMetadata(value) => <CurrentCodecs as CodecFor<
                identities::SubmitReservationMetadataV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::SubmitReservationFill(value) => <CurrentCodecs as CodecFor<
                identities::SubmitReservationFillV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::EntrySkipObservation(value) => <CurrentCodecs as CodecFor<
                identities::EntrySkipObservationV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::ExitSubmissionDecision(value) => <CurrentCodecs as CodecFor<
                identities::ExitSubmissionDecisionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::ExitHoldDecision(value) => <CurrentCodecs as CodecFor<
                identities::ExitHoldDecisionV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::ExitEvaluation(value) => <CurrentCodecs as CodecFor<
                identities::ExitEvaluationV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::LossGovernorHalt(value) => <CurrentCodecs as CodecFor<
                identities::LossGovernorHaltV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::OrderReject(value) => <CurrentCodecs as CodecFor<
                identities::OrderRejectV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::OrderLifecycle(value) => <CurrentCodecs as CodecFor<
                identities::OrderLifecycleV1,
            >>::encode(&value, recorded_at_utc_ns),
            CurrentFact::RequoteThrottleObservation(value) => <CurrentCodecs as CodecFor<
                identities::RequoteThrottleObservationV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::Settlement(value) => <CurrentCodecs as CodecFor<
                identities::SettlementV1,
            >>::encode(&value, recorded_at_utc_ns),
            CurrentFact::TerminalSettlement(value) => <CurrentCodecs as CodecFor<
                identities::TerminalSettlementV1,
            >>::encode(
                value.as_ref(), recorded_at_utc_ns
            ),
            CurrentFact::VenueTruthCaptureFailure(value) => <CurrentCodecs as CodecFor<
                identities::VenueTruthCaptureFailureV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
            CurrentFact::VenueTruthDivergence(value) => <CurrentCodecs as CodecFor<
                identities::VenueTruthDivergenceV1,
            >>::encode(
                &value, recorded_at_utc_ns
            ),
        }
    }

    fn rv_source_diagnostics() -> Vec<RealizedVolatilitySourceDiagnosticFact> {
        let source_classes = RealizedVolSourceClass::wire_coverage_values();
        let sample_kinds = RealizedVolSampleKind::wire_coverage_values();
        let statuses = RealizedVolSourceStatus::wire_coverage_values();
        let count = source_classes
            .len()
            .max(sample_kinds.len())
            .max(statuses.len());
        (0..count)
            .map(|index| RealizedVolatilitySourceDiagnosticFact {
                source_id: format!("source-{index}"),
                source_class: source_classes[index % source_classes.len()].0,
                sample_kind: sample_kinds[index % sample_kinds.len()].0,
                enabled: true,
                counts_toward_quorum: true,
                status: statuses[index % statuses.len()].0,
                annualized_realized_volatility_decimal: Some("0.2".to_string()),
                measured_annualized_realized_volatility_decimal: Some("0.19".to_string()),
                noise_robust_annualized_realized_volatility_decimal: Some("0.18".to_string()),
                continuous_annualized_realized_volatility_decimal: Some("0.17".to_string()),
                jump_annualized_realized_volatility_decimal: Some("0.01".to_string()),
                first_sample_ts_ms: Some(1),
                last_sample_ts_ms: Some(2),
                raw_sample_count: 2,
                grid_sample_count: 2,
                coverage_ratio: "1".to_string(),
                max_inter_sample_gap_ms: Some(1),
                last_rejected_reason: Some(
                    RealizedVolSourceRejectReason::wire_coverage_values()
                        [index % RealizedVolSourceRejectReason::wire_coverage_values().len()]
                    .0,
                ),
                last_rejected_event_ts_ms: Some(1),
                last_rejected_recv_ts_ms: Some(2),
                rejection_counters: RealizedVolSourceRejectReason::wire_coverage_values()
                    .into_iter()
                    .map(|(reason, _)| (reason, 1))
                    .collect(),
                block_reason: Some(
                    RealizedVolBlockReason::wire_coverage_values()
                        [index % RealizedVolBlockReason::wire_coverage_values().len()]
                    .0,
                ),
            })
            .collect()
    }

    fn rv_snapshot(
        pricing_component: RealizedVolPricingComponent,
        aggregation: RealizedVolAggregation,
    ) -> EntryRealizedVolatilitySnapshotFact {
        EntryRealizedVolatilitySnapshotFact {
            surface_id: "surface-coverage".to_string(),
            as_of_ms: Some(2),
            annualized_decimal: Some("0.2".to_string()),
            measured_annualized_decimal: Some("0.19".to_string()),
            noise_robust_annualized_decimal: Some("0.18".to_string()),
            continuous_annualized_decimal: Some("0.17".to_string()),
            jump_annualized_decimal: Some("0.01".to_string()),
            forecast_annualized_decimal: Some("0.21".to_string()),
            pricing_component,
            seconds_per_annum: "31536000".to_string(),
            aggregation,
            sources_used: vec!["source-0".to_string()],
            source_diagnostics: rv_source_diagnostics(),
            unknown_source_rejections: [("unknown-source".to_string(), 1)].into_iter().collect(),
            blockers: RealizedVolBlockReason::wire_coverage_values()
                .into_iter()
                .map(|(reason, _)| reason)
                .collect(),
            config_fingerprint: "coverage-config".to_string(),
        }
    }

    fn complete_rv_snapshot() -> EntryRealizedVolatilitySnapshotFact {
        rv_snapshot(
            RealizedVolPricingComponent::Measured,
            RealizedVolAggregation::UpperQuantile,
        )
    }

    fn null_optional_rv_snapshot() -> EntryRealizedVolatilitySnapshotFact {
        EntryRealizedVolatilitySnapshotFact {
            surface_id: "surface-null-optionals".to_string(),
            as_of_ms: None,
            annualized_decimal: None,
            measured_annualized_decimal: None,
            noise_robust_annualized_decimal: None,
            continuous_annualized_decimal: None,
            jump_annualized_decimal: None,
            forecast_annualized_decimal: None,
            pricing_component: RealizedVolPricingComponent::Measured,
            seconds_per_annum: "31536000".to_string(),
            aggregation: RealizedVolAggregation::UpperQuantile,
            sources_used: vec![],
            source_diagnostics: vec![RealizedVolatilitySourceDiagnosticFact {
                source_id: "source-null-optionals".to_string(),
                source_class: RealizedVolSourceClass::SpotQuote,
                sample_kind: RealizedVolSampleKind::Midpoint,
                enabled: true,
                counts_toward_quorum: true,
                status: RealizedVolSourceStatus::Waiting,
                annualized_realized_volatility_decimal: None,
                measured_annualized_realized_volatility_decimal: None,
                noise_robust_annualized_realized_volatility_decimal: None,
                continuous_annualized_realized_volatility_decimal: None,
                jump_annualized_realized_volatility_decimal: None,
                first_sample_ts_ms: None,
                last_sample_ts_ms: None,
                raw_sample_count: 0,
                grid_sample_count: 0,
                coverage_ratio: "0".to_string(),
                max_inter_sample_gap_ms: None,
                last_rejected_reason: None,
                last_rejected_event_ts_ms: None,
                last_rejected_recv_ts_ms: None,
                rejection_counters: std::collections::BTreeMap::new(),
                block_reason: None,
            }],
            unknown_source_rejections: std::collections::BTreeMap::new(),
            blockers: vec![],
            config_fingerprint: "coverage-config".to_string(),
        }
    }

    fn entry_block_coverage_values() -> Vec<EntryBlockReason> {
        let mut values = EntryBlockReason::wire_coverage_values()
            .into_iter()
            .map(|(value, _)| value)
            .collect::<Vec<_>>();
        values.extend(
            ForcedFlatReason::wire_coverage_values()
                .into_iter()
                .map(|(reason, _)| EntryBlockReason::ForcedFlat(reason)),
        );
        values.extend(
            ExposureOccupancy::wire_coverage_values()
                .into_iter()
                .map(|(occupancy, _)| EntryBlockReason::OnePositionInvariant(occupancy)),
        );
        values
    }

    fn entry_pricing_coverage_values() -> Vec<EntryPricingBlockReason> {
        let mut values = EntryPricingBlockReason::wire_coverage_values()
            .into_iter()
            .map(|(value, _)| value)
            .collect::<Vec<_>>();
        for (side, _) in OutcomeSide::wire_coverage_values() {
            values.push(EntryPricingBlockReason::FeeUnavailable(side));
            values.push(EntryPricingBlockReason::ExecutableEntryCostUnavailable(
                side,
            ));
            values.push(EntryPricingBlockReason::SizedNotionalUnsupported(side));
            for (reason, _) in BinaryOutcomeEdgeBlockReason::wire_coverage_values() {
                values.push(EntryPricingBlockReason::ExecutableEdgeUnavailable(
                    side, reason,
                ));
            }
        }
        values
    }

    fn wire_coverage_facts(identity: KnownIdentity, baseline: CurrentFact) -> Vec<CurrentFact> {
        let mut cases = vec![baseline.clone()];
        match baseline {
            CurrentFact::BlockedStrategyInputObservation(value) => {
                for (outcome, _) in StrategyInputMarketSelectionOutcome::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.details.market_selection_outcome = outcome;
                    cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(case)));
                }
                for (gate_result, _) in RvGateResult::wire_coverage_values() {
                    let mut absent = value.as_ref().clone();
                    absent.details.realized_volatility = StrategyInputRvState::Absent {
                        gate_result,
                        receive_watermark_ms: Some(2),
                    };
                    cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(
                        absent,
                    )));

                    let mut present = value.as_ref().clone();
                    present.details.realized_volatility = StrategyInputRvState::Present {
                        selected_annualized_decimal: Some("0.2".to_string()),
                        gate_result,
                        receive_watermark_ms: Some(2),
                        snapshot: Box::new(complete_rv_snapshot()),
                    };
                    cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(
                        present,
                    )));
                }
                for (pricing_component, _) in RealizedVolPricingComponent::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.details.realized_volatility = StrategyInputRvState::Present {
                        selected_annualized_decimal: Some("0.2".to_string()),
                        gate_result: RvGateResult::Accepted,
                        receive_watermark_ms: Some(2),
                        snapshot: Box::new(rv_snapshot(
                            pricing_component,
                            RealizedVolAggregation::UpperQuantile,
                        )),
                    };
                    cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(case)));
                }
                for (aggregation, _) in RealizedVolAggregation::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.details.realized_volatility = StrategyInputRvState::Present {
                        selected_annualized_decimal: Some("0.2".to_string()),
                        gate_result: RvGateResult::Accepted,
                        receive_watermark_ms: Some(2),
                        snapshot: Box::new(rv_snapshot(
                            RealizedVolPricingComponent::Measured,
                            aggregation,
                        )),
                    };
                    cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(case)));
                }
                let mut null_rv_optionals = value.as_ref().clone();
                null_rv_optionals.details.realized_volatility = StrategyInputRvState::Present {
                    selected_annualized_decimal: None,
                    gate_result: RvGateResult::MissingSnapshot,
                    receive_watermark_ms: None,
                    snapshot: Box::new(null_optional_rv_snapshot()),
                };
                cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(
                    null_rv_optionals,
                )));
                let mut blockers = value.as_ref().clone();
                blockers.details.gate_blocked_by = entry_block_coverage_values();
                blockers.details.pricing_blocked_by = entry_pricing_coverage_values();
                blockers.details.selected_side = Some("up".to_string());
                cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(
                    blockers,
                )));

                let mut absent_numbers = value.as_ref().clone();
                clear_strategy_input_optionals(&mut absent_numbers.details);
                absent_numbers.details.price_to_beat_value = None;
                absent_numbers.details.spot_price = None;
                absent_numbers.details.theta_scaled_min_edge_bps = None;
                absent_numbers.details.fair_probability_up = None;
                absent_numbers.details.uncertainty_band_probability = None;
                absent_numbers.details.expected_edge_basis_points = None;
                absent_numbers.details.worst_case_edge_basis_points = None;
                absent_numbers.details.fee_rate_basis_points = None;
                cases.push(CurrentFact::BlockedStrategyInputObservation(Box::new(
                    absent_numbers,
                )));
            }
            CurrentFact::SubmitLinkedStrategyInputSnapshot(value) => {
                for (outcome, _) in StrategyInputMarketSelectionOutcome::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.details.market_selection_outcome = outcome;
                    cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                        case,
                    )));
                }
                for (gate_result, _) in RvGateResult::wire_coverage_values() {
                    let mut absent = value.as_ref().clone();
                    absent.details.realized_volatility = StrategyInputRvState::Absent {
                        gate_result,
                        receive_watermark_ms: Some(2),
                    };
                    cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                        absent,
                    )));

                    let mut present = value.as_ref().clone();
                    present.details.realized_volatility = StrategyInputRvState::Present {
                        selected_annualized_decimal: Some("0.2".to_string()),
                        gate_result,
                        receive_watermark_ms: Some(2),
                        snapshot: Box::new(complete_rv_snapshot()),
                    };
                    cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                        present,
                    )));
                }
                let mut null_rv_optionals = value.as_ref().clone();
                null_rv_optionals.details.realized_volatility = StrategyInputRvState::Present {
                    selected_annualized_decimal: None,
                    gate_result: RvGateResult::MissingSnapshot,
                    receive_watermark_ms: None,
                    snapshot: Box::new(null_optional_rv_snapshot()),
                };
                cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                    null_rv_optionals,
                )));
                let mut blockers = value.as_ref().clone();
                blockers.details.gate_blocked_by = entry_block_coverage_values();
                blockers.details.pricing_blocked_by = entry_pricing_coverage_values();
                blockers.details.selected_side = Some("up".to_string());
                cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                    blockers,
                )));
                let mut absent_optionals = value.as_ref().clone();
                clear_strategy_input_optionals(&mut absent_optionals.details);
                cases.push(CurrentFact::SubmitLinkedStrategyInputSnapshot(Box::new(
                    absent_optionals,
                )));
            }
            CurrentFact::EntryOrderIntent(value) => {
                for (outcome, _) in OrderIntentClampOutcome::wire_coverage_values() {
                    let mut case = value.clone();
                    case.details.clamp_outcome = Some(outcome);
                    cases.push(CurrentFact::EntryOrderIntent(case));
                }
                for (reason, _) in OrderIntentClampNotEvaluatedReason::wire_coverage_values() {
                    let mut case = value.clone();
                    case.details.clamp_outcome =
                        Some(OrderIntentClampOutcome::NotEvaluated { reason });
                    cases.push(CurrentFact::EntryOrderIntent(case));
                }
                for details in order_intent_optional_cases(&value.details) {
                    cases.push(CurrentFact::EntryOrderIntent(EntryOrderIntentFact {
                        details,
                    }));
                }
            }
            CurrentFact::RiskReducingExitOrderIntent(value) => {
                for (outcome, _) in OrderIntentClampOutcome::wire_coverage_values() {
                    let mut case = value.clone();
                    case.details.clamp_outcome = Some(outcome);
                    cases.push(CurrentFact::RiskReducingExitOrderIntent(case));
                }
                for details in order_intent_optional_cases(&value.details) {
                    cases.push(CurrentFact::RiskReducingExitOrderIntent(
                        RiskReducingExitOrderIntentFact { details },
                    ));
                }
                for (reason, _) in OrderIntentClampNotEvaluatedReason::wire_coverage_values() {
                    let mut case = value.clone();
                    case.details.clamp_outcome =
                        Some(OrderIntentClampOutcome::NotEvaluated { reason });
                    cases.push(CurrentFact::RiskReducingExitOrderIntent(case));
                }
            }
            CurrentFact::BasketAdmissionRejected(value) => {
                for (reason, _) in BasketAdmissionRejectionReason::wire_coverage_values() {
                    let mut case = value.clone();
                    case.reason = reason;
                    cases.push(CurrentFact::BasketAdmissionRejected(case));
                }
            }
            CurrentFact::CapitalAdmissionRebuild(value) => {
                for (reason, _) in CapitalAdmissionRejectionReason::wire_coverage_values() {
                    let mut case = value.clone();
                    case.outcome = CapitalAdmissionRebuildOutcome::Rejected(reason);
                    cases.push(CurrentFact::CapitalAdmissionRebuild(case));
                }
            }
            CurrentFact::RequoteThrottleObservation(value) => {
                for (action_cost_class, _) in RequoteActionCostClass::wire_coverage_values() {
                    let mut case = value.clone();
                    case.action_cost_class = action_cost_class;
                    cases.push(CurrentFact::RequoteThrottleObservation(case));
                }
                for (bound_by, _) in RequoteThrottleBound::wire_coverage_values() {
                    let mut case = value.clone();
                    case.bound_by = bound_by;
                    cases.push(CurrentFact::RequoteThrottleObservation(case));
                }
                let mut null_market = value.clone();
                null_market.market_id = None;
                cases.push(CurrentFact::RequoteThrottleObservation(null_market));
                let mut present_market = value.clone();
                present_market.market_id = Some("market-coverage".to_string());
                cases.push(CurrentFact::RequoteThrottleObservation(present_market));
            }
            CurrentFact::VenueTruthDivergence(value) => {
                for (alarm_class, _) in VenueTruthDivergenceAlarmClass::wire_coverage_values() {
                    let mut case = value.clone();
                    case.alarm_class = alarm_class;
                    cases.push(CurrentFact::VenueTruthDivergence(case));
                }
            }
            CurrentFact::LossGovernorHalt(value) => {
                for (stale_reason, _) in StaleLossReason::wire_coverage_values() {
                    let mut case = value.clone();
                    case.stale_reason = stale_reason;
                    cases.push(CurrentFact::LossGovernorHalt(case));
                }
                let mut nulls = value.clone();
                nulls.snapshot_present = false;
                nulls.snapshot_observed_at_ns = None;
                nulls.snapshot_age_ns = None;
                nulls.snapshot_source = None;
                nulls.has_per_trade_pnl = false;
                nulls.has_daily_pnl = false;
                nulls.has_rolling_pnl = false;
                nulls.has_current_equity = false;
                nulls.has_peak_equity = false;
                nulls.last_account_state_ts_ns = None;
                nulls.last_portfolio_snapshot_ts_ns = None;
                nulls.last_position_event_ts_ns = None;
                cases.push(CurrentFact::LossGovernorHalt(nulls));

                let mut present = value.clone();
                present.snapshot_present = true;
                present.snapshot_observed_at_ns = Some(10);
                present.snapshot_age_ns = Some(2);
                present.snapshot_source = Some("nt_account_snapshot".to_string());
                present.last_account_state_ts_ns = Some(10);
                present.last_portfolio_snapshot_ts_ns = Some(10);
                present.last_position_event_ts_ns = Some(10);
                cases.push(CurrentFact::LossGovernorHalt(present));
            }
            CurrentFact::AdmittedEntryAdmission(value) => {
                cases.extend(
                    admission_detail_cases(&value.details)
                        .into_iter()
                        .map(|details| {
                            CurrentFact::AdmittedEntryAdmission(Box::new(
                                AdmittedEntryAdmissionFact { details },
                            ))
                        }),
                );
            }
            CurrentFact::RejectedEntryAdmission(value) => {
                cases.extend(
                    admission_detail_cases(&value.details)
                        .into_iter()
                        .map(|details| {
                            CurrentFact::RejectedEntryAdmission(Box::new(
                                RejectedEntryAdmissionFact {
                                    details,
                                    reason: value.reason,
                                },
                            ))
                        }),
                );
                for (reason, _) in AdmissionRejectionReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.reason = reason;
                    cases.push(CurrentFact::RejectedEntryAdmission(Box::new(case)));
                }
            }
            CurrentFact::RiskReducingExitAdmission(value) => {
                cases.extend(admission_outcome_cases(
                    value.details.clone(),
                    |details, outcome| {
                        CurrentFact::RiskReducingExitAdmission(Box::new(
                            RiskReducingExitAdmissionFact { details, outcome },
                        ))
                    },
                ));
            }
            CurrentFact::ForcedReductionAdmission(value) => {
                cases.extend(admission_outcome_cases(
                    value.details.clone(),
                    |details, outcome| {
                        CurrentFact::ForcedReductionAdmission(Box::new(
                            ForcedReductionAdmissionFact { details, outcome },
                        ))
                    },
                ));
            }
            CurrentFact::OrderReject(value) => {
                for (reject_source, _) in OrderRejectSource::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.reject_source = reject_source;
                    if reject_source == OrderRejectSource::SubmitAdmission {
                        case.reject_reason = OrderRejectReason::AdmissionRejected;
                        case.admission_outcome = Some(AdmissionDecisionOutcome::Rejected(
                            AdmissionRejectionReason::NotionalCapExceeded,
                        ));
                    } else {
                        case.reject_reason = OrderRejectReason::Other;
                        case.admission_outcome = None;
                    }
                    cases.push(CurrentFact::OrderReject(Box::new(case)));
                }
                for (reject_reason, _) in OrderRejectReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.reject_reason = reject_reason;
                    if reject_reason == OrderRejectReason::AdmissionRejected {
                        case.reject_source = OrderRejectSource::SubmitAdmission;
                        case.admission_outcome = Some(AdmissionDecisionOutcome::Rejected(
                            AdmissionRejectionReason::NotionalCapExceeded,
                        ));
                    } else {
                        case.admission_outcome = None;
                    }
                    cases.push(CurrentFact::OrderReject(Box::new(case)));
                }
                for (reason, _) in AdmissionRejectionReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.reject_source = OrderRejectSource::SubmitAdmission;
                    case.reject_reason = OrderRejectReason::AdmissionRejected;
                    case.admission_outcome = Some(AdmissionDecisionOutcome::Rejected(reason));
                    cases.push(CurrentFact::OrderReject(Box::new(case)));
                }
                cases.extend(
                    order_reject_optional_cases(value.as_ref())
                        .into_iter()
                        .map(|case| CurrentFact::OrderReject(Box::new(case))),
                );
            }
            CurrentFact::EntrySkipObservation(value) => {
                for (reason, _) in EntrySkipReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.reason_category = reason;
                    case.submission_blocked_reason = Some(reason);
                    cases.push(CurrentFact::EntrySkipObservation(Box::new(case)));
                }
                let mut blockers = value.as_ref().clone();
                blockers.gate_blocked_by = entry_block_coverage_values();
                blockers.pricing_blocked_by = entry_pricing_coverage_values();
                blockers.realized_vol_snapshot = Some(complete_rv_snapshot());
                cases.push(CurrentFact::EntrySkipObservation(Box::new(blockers)));

                for (gate_result, _) in RvGateResult::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.realized_vol_gate_result = Some(gate_result);
                    cases.push(CurrentFact::EntrySkipObservation(Box::new(case)));
                }
                for (side, _) in OutcomeSide::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.selected_side = Some(side);
                    cases.push(CurrentFact::EntrySkipObservation(Box::new(case)));
                }
                for (pricing_component, _) in RealizedVolPricingComponent::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.realized_vol_snapshot = Some(rv_snapshot(
                        pricing_component,
                        RealizedVolAggregation::UpperQuantile,
                    ));
                    cases.push(CurrentFact::EntrySkipObservation(Box::new(case)));
                }
                for (aggregation, _) in RealizedVolAggregation::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.realized_vol_snapshot = Some(rv_snapshot(
                        RealizedVolPricingComponent::Measured,
                        aggregation,
                    ));
                    cases.push(CurrentFact::EntrySkipObservation(Box::new(case)));
                }
                let mut null_rv_optionals = value.as_ref().clone();
                null_rv_optionals.realized_vol_snapshot = Some(null_optional_rv_snapshot());
                cases.push(CurrentFact::EntrySkipObservation(Box::new(
                    null_rv_optionals,
                )));
                cases.extend(
                    entry_skip_optional_cases(value.as_ref())
                        .into_iter()
                        .map(|case| CurrentFact::EntrySkipObservation(Box::new(case))),
                );
            }
            CurrentFact::ExitSubmissionDecision(value) => {
                cases.extend(
                    exit_detail_cases(&value.details)
                        .into_iter()
                        .map(|details| {
                            CurrentFact::ExitSubmissionDecision(Box::new(
                                ExitSubmissionDecisionFact {
                                    details,
                                    outcome: value.outcome,
                                    submission: value.submission.clone(),
                                },
                            ))
                        }),
                );
                for (outcome, _) in ExitSubmissionOutcome::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.outcome = outcome;
                    cases.push(CurrentFact::ExitSubmissionDecision(Box::new(case)));
                }
            }
            CurrentFact::ExitHoldDecision(value) => {
                cases.extend(
                    exit_detail_cases(&value.details)
                        .into_iter()
                        .map(|details| {
                            CurrentFact::ExitHoldDecision(Box::new(ExitHoldDecisionFact {
                                details,
                                outcome: ExitHoldOutcome::Hold,
                                blocked_reason: None,
                            }))
                        }),
                );
                for (blocked_reason, _) in ExitBlockedReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.outcome = ExitHoldOutcome::Blocked;
                    case.blocked_reason = Some(blocked_reason);
                    cases.push(CurrentFact::ExitHoldDecision(Box::new(case)));
                }
            }
            CurrentFact::ExitEvaluation(value) => {
                for (exit_trigger_source, _) in ExitTriggerSource::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.exit_trigger_source = exit_trigger_source;
                    cases.push(CurrentFact::ExitEvaluation(Box::new(case)));
                }
                for (outcome, _) in ExitSubmissionOutcome::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.decision = ExitEvaluationDecision::Submission { outcome };
                    cases.push(CurrentFact::ExitEvaluation(Box::new(case)));
                }
                for (blocked_reason, _) in ExitBlockedReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.decision = ExitEvaluationDecision::Hold {
                        outcome: ExitHoldOutcome::Blocked,
                        blocked_reason: Some(blocked_reason),
                    };
                    cases.push(CurrentFact::ExitEvaluation(Box::new(case)));
                }
                for (gate_result, _) in RvGateResult::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.rv_gate_result = gate_result;
                    cases.push(CurrentFact::ExitEvaluation(Box::new(case)));
                }
                let mut blockers = value.as_ref().clone();
                blockers.rv_blockers = RealizedVolBlockReason::wire_coverage_values()
                    .into_iter()
                    .map(|(reason, _)| reason)
                    .collect();
                blockers.forced_flat_reasons = ForcedFlatReason::wire_coverage_values()
                    .into_iter()
                    .map(|(_, wire)| wire.to_string())
                    .collect();
                cases.push(CurrentFact::ExitEvaluation(Box::new(blockers)));
                cases.extend(
                    exit_evaluation_optional_cases(value.as_ref())
                        .into_iter()
                        .map(|case| CurrentFact::ExitEvaluation(Box::new(case))),
                );
            }
            CurrentFact::Settlement(value) => {
                for (outcome_side, _) in OutcomeSide::wire_coverage_values() {
                    let mut case = value.clone();
                    case.outcome_side = outcome_side;
                    cases.push(CurrentFact::Settlement(case));
                }
            }
            CurrentFact::OrderLifecycle(value) => {
                for (transition, _) in OrderLifecycleTransition::wire_coverage_values() {
                    let mut case = value.clone();
                    case.transition = transition;
                    cases.push(CurrentFact::OrderLifecycle(case));
                }
                for (outcome, _) in OrderLifecycleOutcome::wire_coverage_values() {
                    let mut case = value.clone();
                    case.outcome = outcome;
                    cases.push(CurrentFact::OrderLifecycle(case));
                }
                cases.extend(
                    order_lifecycle_optional_cases(&value)
                        .into_iter()
                        .map(CurrentFact::OrderLifecycle),
                );
            }
            CurrentFact::TerminalSettlement(value) => {
                for (transition, _) in OrderLifecycleTransition::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.lifecycle.transition = transition;
                    cases.push(CurrentFact::TerminalSettlement(Box::new(case)));
                }
                for (outcome, _) in OrderLifecycleOutcome::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.lifecycle.outcome = outcome;
                    cases.push(CurrentFact::TerminalSettlement(Box::new(case)));
                }
                for (reason, _) in SettlementBookingErrorReason::wire_coverage_values() {
                    let mut case = value.as_ref().clone();
                    case.booking_error
                        .as_mut()
                        .expect("terminal baseline has booking error")
                        .reason = reason;
                    cases.push(CurrentFact::TerminalSettlement(Box::new(case)));
                }
                for lifecycle in order_lifecycle_optional_cases(&value.lifecycle) {
                    let mut case = value.as_ref().clone();
                    case.lifecycle = lifecycle;
                    cases.push(CurrentFact::TerminalSettlement(Box::new(case)));
                }
                let mut without_booking_error = value.as_ref().clone();
                without_booking_error.booking_error = None;
                cases.push(CurrentFact::TerminalSettlement(Box::new(
                    without_booking_error,
                )));
                if let Some(booking_error) = value.booking_error.as_ref() {
                    for booking_error in settlement_booking_error_optional_cases(booking_error) {
                        let mut case = value.as_ref().clone();
                        case.booking_error = Some(booking_error);
                        cases.push(CurrentFact::TerminalSettlement(Box::new(case)));
                    }
                }
            }
            CurrentFact::BasketAdmissionGranted(_)
            | CurrentFact::SubmitReservationMetadata(_)
            | CurrentFact::SubmitReservationFill(_)
            | CurrentFact::VenueTruthCaptureFailure(_) => {}
        }
        assert!(
            !cases.is_empty(),
            "{identity:?} must have at least one canonical wire case"
        );
        cases
    }

    fn admission_detail_cases(baseline: &AdmissionDetails) -> Vec<AdmissionDetails> {
        let mut cases = Vec::new();
        for (reason, _) in LossHaltReason::wire_coverage_values() {
            let mut case = baseline.clone();
            case.loss_halt_reasons = vec![reason];
            cases.push(case);
        }
        for (source, _) in LossSnapshotSource::wire_coverage_values() {
            let mut case = baseline.clone();
            case.snapshot_source = Some(source);
            cases.push(case);
        }
        for (reason, _) in LossSnapshotStaleReason::wire_coverage_values() {
            let mut case = baseline.clone();
            case.stale_reason = Some(reason);
            cases.push(case);
        }
        let mut nulls = baseline.clone();
        nulls.snapshot_observed_at_ns = None;
        nulls.snapshot_age_ns = None;
        nulls.max_snapshot_age_ns = None;
        nulls.snapshot_source = None;
        nulls.last_account_state_observed_at_ns = None;
        nulls.last_portfolio_snapshot_observed_at_ns = None;
        nulls.last_position_event_observed_at_ns = None;
        nulls.stale_reason = None;
        nulls.loss_snapshot_observed_at_ns = None;
        nulls.loss_eval_now_ns = None;
        cases.push(nulls);

        let mut present = baseline.clone();
        present.snapshot_observed_at_ns = Some(10);
        present.snapshot_age_ns = Some(2);
        present.max_snapshot_age_ns = Some(5);
        present.snapshot_source = Some(LossSnapshotSource::NtAccountSnapshot);
        present.last_account_state_observed_at_ns = Some(10);
        present.last_portfolio_snapshot_observed_at_ns = Some(10);
        present.last_position_event_observed_at_ns = Some(10);
        present.stale_reason = Some(LossSnapshotStaleReason::AgeExceeded);
        present.loss_snapshot_observed_at_ns = Some(10);
        present.loss_eval_now_ns = Some(12);
        cases.push(present);
        cases
    }

    fn order_intent_optional_cases(baseline: &OrderIntentDetails) -> Vec<OrderIntentDetails> {
        let mut nulls = baseline.clone();
        nulls.clamp_outcome = None;
        nulls.order_fields.price = None;
        nulls.order_fields.trigger_price = None;
        nulls.order_fields.activation_price = None;
        nulls.order_fields.trigger_type = None;
        nulls.order_fields.trigger_instrument_id = None;
        nulls.order_fields.trailing_offset = None;
        nulls.order_fields.trailing_offset_type = None;
        nulls.order_fields.expire_time_unix_nanos = None;

        let mut present = baseline.clone();
        present.clamp_outcome = Some(OrderIntentClampOutcome::WithinBounds);
        present.order_fields.price = Some("0.4".to_string());
        present.order_fields.trigger_price = Some("0.5".to_string());
        present.order_fields.activation_price = Some("0.5".to_string());
        present.order_fields.trigger_type = Some("last_price".to_string());
        present.order_fields.trigger_instrument_id = Some("YES-USD.POLYMARKET".to_string());
        present.order_fields.trailing_offset = Some("0.01".to_string());
        present.order_fields.trailing_offset_type = Some("price".to_string());
        present.order_fields.expire_time_unix_nanos = Some("1000000000".to_string());
        vec![nulls, present]
    }

    fn admission_outcome_cases(
        details: AdmissionDetails,
        wrap: impl Fn(AdmissionDetails, AdmissionDecisionOutcome) -> CurrentFact,
    ) -> Vec<CurrentFact> {
        let mut cases = vec![wrap(details.clone(), AdmissionDecisionOutcome::Admitted)];
        for (reason, _) in AdmissionRejectionReason::wire_coverage_values() {
            cases.push(wrap(
                details.clone(),
                AdmissionDecisionOutcome::Rejected(reason),
            ));
        }
        cases.extend(
            admission_detail_cases(&details)
                .into_iter()
                .map(|details| wrap(details, AdmissionDecisionOutcome::Admitted)),
        );
        cases
    }

    fn exit_detail_cases(baseline: &ExitDecisionDetails) -> Vec<ExitDecisionDetails> {
        let mut cases = Vec::new();
        for (exit_trigger_source, _) in ExitTriggerSource::wire_coverage_values() {
            let mut case = baseline.clone();
            case.exit_trigger_source = exit_trigger_source;
            cases.push(case);
        }
        for (position_outcome_side, _) in OutcomeSide::wire_coverage_values() {
            let mut case = baseline.clone();
            case.position_outcome_side = Some(position_outcome_side);
            cases.push(case);
        }
        for (rv_gate_result, _) in RvGateResult::wire_coverage_values() {
            let mut case = baseline.clone();
            case.rv_gate_result = rv_gate_result;
            cases.push(case);
        }
        let mut blockers = baseline.clone();
        blockers.forced_flat_reasons = ForcedFlatReason::wire_coverage_values()
            .into_iter()
            .map(|(reason, _)| reason)
            .collect();
        blockers.rv_snapshot_blockers = RealizedVolBlockReason::wire_coverage_values()
            .into_iter()
            .map(|(reason, _)| reason)
            .collect();
        blockers.rv_source_diagnostics = rv_source_diagnostics();
        cases.push(blockers);

        let mut nulls = baseline.clone();
        nulls.market_id = None;
        nulls.position_id = None;
        nulls.position_instrument_id = None;
        nulls.position_outcome_side = None;
        nulls.spot_price = None;
        nulls.spot_venue_name = None;
        nulls.reference_current_price = None;
        nulls.interval_open = None;
        nulls.fair_probability_up = None;
        nulls.fair_probability_down = None;
        nulls.uncertainty_band_probability = None;
        nulls.up_fee_bps = None;
        nulls.down_fee_bps = None;
        nulls.hold_ev_bps = None;
        nulls.exit_ev_bps = None;
        nulls.realized_vol = None;
        nulls.realized_vol_source_venue = None;
        nulls.realized_vol_source_ts_ms = None;
        nulls.trigger_ts_init_ms = None;
        nulls.rv_snapshot_as_of_ms = None;
        nulls.rv_snapshot_has_ready_realized_vol = None;
        nulls.rv_snapshot_receive_watermark_ms = None;
        nulls.rv_max_source_age_ms = None;
        nulls.rv_future_dating_delta_ms = None;
        nulls.seconds_to_market_end = None;
        nulls.stale_reference_after_ms = None;
        nulls.last_reference_ts_ms = None;
        nulls.min_liquidity_required = None;
        nulls.liquidity_available = None;
        nulls.rv_source_diagnostics = null_optional_rv_snapshot().source_diagnostics;
        cases.push(nulls);

        let mut present = baseline.clone();
        present.market_id = Some("market-coverage".to_string());
        present.position_id = Some("position-coverage".to_string());
        present.position_instrument_id = Some("YES-USD.POLYMARKET".to_string());
        present.position_outcome_side = Some(OutcomeSide::Up);
        present.spot_price = Some("100".to_string());
        present.spot_venue_name = Some("venue".to_string());
        present.reference_current_price = Some("100".to_string());
        present.interval_open = Some("100".to_string());
        present.fair_probability_up = Some("0.5".to_string());
        present.fair_probability_down = Some("0.5".to_string());
        present.uncertainty_band_probability = Some("0.01".to_string());
        present.up_fee_bps = Some("1".to_string());
        present.down_fee_bps = Some("1".to_string());
        present.hold_ev_bps = Some("1".to_string());
        present.exit_ev_bps = Some("2".to_string());
        present.realized_vol = Some("0.2".to_string());
        present.realized_vol_source_venue = Some("venue".to_string());
        present.realized_vol_source_ts_ms = Some(2);
        present.trigger_ts_init_ms = Some(34);
        present.rv_snapshot_as_of_ms = Some(2);
        present.rv_snapshot_has_ready_realized_vol = Some(true);
        present.rv_snapshot_receive_watermark_ms = Some(2);
        present.rv_max_source_age_ms = Some(1_000);
        present.rv_future_dating_delta_ms = Some(1);
        present.seconds_to_market_end = Some(60);
        present.stale_reference_after_ms = Some(5_000);
        present.last_reference_ts_ms = Some(34);
        present.min_liquidity_required = Some("1".to_string());
        present.liquidity_available = Some("2".to_string());
        cases.push(present);
        cases
    }

    fn exit_evaluation_optional_cases(baseline: &ExitEvaluationFact) -> Vec<ExitEvaluationFact> {
        let mut nulls = baseline.clone();
        nulls.position_id = None;
        nulls.market_id = None;
        nulls.instrument_id = None;
        nulls.client_order_id = None;
        nulls.trigger_ts_event_ms = None;
        nulls.trigger_ts_init_ms = None;
        nulls.rv_as_of_ms = None;
        nulls.rv_snapshot_receive_watermark_ms = None;
        nulls.rv_max_source_age_ms = None;
        nulls.rv_as_of_minus_now_ms = None;
        nulls.spot_price = None;
        nulls.spot_venue_name = None;
        nulls.reference_current_price = None;
        nulls.interval_open = None;
        nulls.fair_probability_up = None;
        nulls.fair_probability_down = None;
        nulls.uncertainty_band_probability = None;
        nulls.up_fee_bps = None;
        nulls.down_fee_bps = None;
        nulls.hold_ev_bps = None;
        nulls.exit_ev_bps = None;

        let mut present = baseline.clone();
        present.position_id = Some("position-coverage".to_string());
        present.market_id = Some("market-coverage".to_string());
        present.instrument_id = Some("YES-USD.POLYMARKET".to_string());
        present.client_order_id = Some("client-order-coverage".to_string());
        present.trigger_ts_event_ms = Some(34);
        present.trigger_ts_init_ms = Some(34);
        present.rv_as_of_ms = Some(33);
        present.rv_snapshot_receive_watermark_ms = Some(33);
        present.rv_max_source_age_ms = Some(1_000);
        present.rv_as_of_minus_now_ms = Some(-1);
        present.spot_price = Some("100".to_string());
        present.spot_venue_name = Some("venue".to_string());
        present.reference_current_price = Some("100".to_string());
        present.interval_open = Some("100".to_string());
        present.fair_probability_up = Some("0.5".to_string());
        present.fair_probability_down = Some("0.5".to_string());
        present.uncertainty_band_probability = Some("0.01".to_string());
        present.up_fee_bps = Some("1".to_string());
        present.down_fee_bps = Some("1".to_string());
        present.hold_ev_bps = Some("1".to_string());
        present.exit_ev_bps = Some("2".to_string());

        vec![nulls, present]
    }

    fn order_reject_optional_cases(baseline: &OrderRejectFact) -> Vec<OrderRejectFact> {
        let mut nulls = baseline.clone();
        nulls.reject_source = OrderRejectSource::Venue;
        nulls.reject_reason = OrderRejectReason::Other;
        nulls.admission_outcome = None;
        nulls.raw_reason_text = None;
        nulls.order_side = None;
        nulls.raw_price = None;
        nulls.raw_quantity = None;
        nulls.raw_maker_amount = None;
        nulls.raw_taker_amount = None;
        nulls.normalized_price = None;
        nulls.normalized_quantity = None;
        nulls.normalized_maker_amount = None;
        nulls.normalized_taker_amount = None;
        nulls.venue_price_precision = None;
        nulls.venue_size_precision = None;
        nulls.venue_min_notional = None;
        nulls.prior_client_order_id = None;
        nulls.backoff_cooldown_state = None;

        let mut present = nulls.clone();
        present.raw_reason_text = Some("venue rejected order".to_string());
        present.order_side = Some("buy".to_string());
        present.raw_price = Some("0.5".to_string());
        present.raw_quantity = Some("2".to_string());
        present.raw_maker_amount = Some("1".to_string());
        present.raw_taker_amount = Some("2".to_string());
        present.normalized_price = Some("0.5".to_string());
        present.normalized_quantity = Some("2".to_string());
        present.normalized_maker_amount = Some("1".to_string());
        present.normalized_taker_amount = Some("2".to_string());
        present.venue_price_precision = Some(2);
        present.venue_size_precision = Some(2);
        present.venue_min_notional = Some("1".to_string());
        present.prior_client_order_id = Some("prior-client-order".to_string());
        present.backoff_cooldown_state = Some("active".to_string());

        vec![nulls, present]
    }

    fn order_lifecycle_optional_cases(baseline: &OrderLifecycleFact) -> Vec<OrderLifecycleFact> {
        let mut nulls = baseline.clone();
        nulls.market_id = None;
        nulls.instrument_id = None;
        nulls.position_id = None;
        nulls.client_order_id = None;
        nulls.prior_client_order_id = None;
        nulls.raw_reason_text = None;
        nulls.order_side = None;
        nulls.filled_quantity = None;
        nulls.residual_quantity = None;
        nulls.ts_event_ns = None;

        let mut present = baseline.clone();
        present.market_id = Some("market-coverage".to_string());
        present.instrument_id = Some("YES-USD.POLYMARKET".to_string());
        present.position_id = Some("position-coverage".to_string());
        present.client_order_id = Some("client-order-coverage".to_string());
        present.prior_client_order_id = Some("prior-client-order-coverage".to_string());
        present.raw_reason_text = Some("lifecycle transition".to_string());
        present.order_side = Some("buy".to_string());
        present.filled_quantity = Some("1".to_string());
        present.residual_quantity = Some("1".to_string());
        present.ts_event_ns = Some(10);

        vec![nulls, present]
    }

    fn settlement_booking_error_optional_cases(
        baseline: &SettlementBookingErrorFact,
    ) -> Vec<SettlementBookingErrorFact> {
        let mut nulls = baseline.clone();
        nulls.market_id = None;
        nulls.position_id = None;
        nulls.instrument_id = None;
        nulls.resolution_instrument_id = None;

        let mut present = baseline.clone();
        present.market_id = Some("market-coverage".to_string());
        present.position_id = Some("position-coverage".to_string());
        present.instrument_id = Some("YES-USD.POLYMARKET".to_string());
        present.resolution_instrument_id = Some("BTC-USD.CHAINLINK".to_string());

        vec![nulls, present]
    }

    fn entry_skip_optional_cases(baseline: &EntrySkipFact) -> Vec<EntrySkipFact> {
        let mut nulls = baseline.clone();
        nulls.market_id = None;
        nulls.seconds_to_market_end = None;
        nulls.spot_price = None;
        nulls.reference_current_price = None;
        nulls.realized_vol = None;
        nulls.realized_vol_source_venue = None;
        nulls.realized_vol_source_ts_ms = None;
        nulls.realized_vol_gate_result = None;
        nulls.realized_vol_receive_watermark_ms = None;
        nulls.realized_vol_snapshot = None;
        nulls.fair_probability_up = None;
        nulls.fair_probability_down = None;
        nulls.selected_side = None;
        nulls.sized_notional = None;
        nulls.sized_worst_case_ev_bps = None;
        nulls.sized_edge_cents_per_share = None;
        nulls.theta_scaled_min_edge_bps = None;
        nulls.up_fee_bps = None;
        nulls.down_fee_bps = None;
        nulls.submission_blocked_reason = None;
        nulls.stale_reference_after_ms = None;
        nulls.last_reference_ts_ms = None;
        nulls.min_liquidity_required = None;
        nulls.liquidity_available = None;

        let mut present = baseline.clone();
        present.market_id = Some("market-coverage".to_string());
        present.seconds_to_market_end = Some(60);
        present.spot_price = Some("100".to_string());
        present.reference_current_price = Some("100".to_string());
        present.realized_vol = Some("0.2".to_string());
        present.realized_vol_source_venue = Some("venue".to_string());
        present.realized_vol_source_ts_ms = Some(2);
        present.realized_vol_gate_result = Some(RvGateResult::Accepted);
        present.realized_vol_receive_watermark_ms = Some(2);
        present.realized_vol_snapshot = Some(complete_rv_snapshot());
        present.fair_probability_up = Some("0.5".to_string());
        present.fair_probability_down = Some("0.5".to_string());
        present.selected_side = Some(OutcomeSide::Up);
        present.sized_notional = Some("10".to_string());
        present.sized_worst_case_ev_bps = Some("5".to_string());
        present.sized_edge_cents_per_share = Some("0.01".to_string());
        present.theta_scaled_min_edge_bps = Some("5".to_string());
        present.up_fee_bps = Some("1".to_string());
        present.down_fee_bps = Some("1".to_string());
        present.submission_blocked_reason = Some(EntrySkipReason::EntryPricingBlocked);
        present.stale_reference_after_ms = Some(5_000);
        present.last_reference_ts_ms = Some(1);
        present.min_liquidity_required = Some("1".to_string());
        present.liquidity_available = Some("2".to_string());
        vec![nulls, present]
    }

    fn clear_strategy_input_optionals<PurposeNumeric>(
        details: &mut StrategyInputDetails<PurposeNumeric>,
    ) {
        details.market_id = None;
        details.polymarket_condition_id = None;
        details.polymarket_market_slug = None;
        details.polymarket_question_id = None;
        details.up_instrument_id = None;
        details.down_instrument_id = None;
        details.market_selection_timestamp_ms = None;
        details.selected_market_observed_timestamp_ms = None;
        details.polymarket_market_start_timestamp_ms = None;
        details.polymarket_market_end_timestamp_ms = None;
        details.reference_current_price = None;
        details.reference_current_price_source_id = None;
        details.reference_current_price_failed_over = None;
        details.up_worst_case_edge_basis_points = None;
        details.down_worst_case_edge_basis_points = None;
        details.fast_venue_name = None;
        details.fast_venue_age_ms = None;
        details.fast_venue_jitter_ms = None;
        details.lead_agreement_corr = None;
        details.selected_side = None;
        details.realized_volatility = StrategyInputRvState::Absent {
            gate_result: RvGateResult::MissingSnapshot,
            receive_watermark_ms: None,
        };
    }

    fn mutation_is_rejected(identity: KnownIdentity, value: &serde_json::Value) {
        let line = serde_json::to_string(value).expect("mutated fixture must remain JSON");
        assert!(
            decode_current_fact(identity, &line, 1).is_err(),
            "{identity:?} accepted malformed fixture: {line}"
        );
    }

    fn positive_value(identity: KnownIdentity) -> serde_json::Value {
        serde_json::from_str(
            positive_corpus(identity)
                .lines()
                .next()
                .expect("positive corpus must contain a baseline record"),
        )
        .expect("positive corpus line must decode as JSON")
    }

    #[derive(Clone, Debug)]
    enum JsonPathStep {
        Field(String),
        Index(usize),
    }

    fn collect_object_field_paths(
        value: &serde_json::Value,
        prefix: &mut Vec<JsonPathStep>,
        paths: &mut Vec<Vec<JsonPathStep>>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                for (field, child) in object {
                    prefix.push(JsonPathStep::Field(field.clone()));
                    paths.push(prefix.clone());
                    collect_object_field_paths(child, prefix, paths);
                    prefix.pop();
                }
            }
            serde_json::Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    prefix.push(JsonPathStep::Index(index));
                    collect_object_field_paths(child, prefix, paths);
                    prefix.pop();
                }
            }
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_) => {}
        }
    }

    fn value_at_path_mut<'a>(
        mut value: &'a mut serde_json::Value,
        path: &[JsonPathStep],
    ) -> &'a mut serde_json::Value {
        for step in path {
            value = match step {
                JsonPathStep::Field(field) => value
                    .as_object_mut()
                    .and_then(|object| object.get_mut(field))
                    .expect("fixture path must name an object field"),
                JsonPathStep::Index(index) => value
                    .as_array_mut()
                    .and_then(|array| array.get_mut(*index))
                    .expect("fixture path must name an array element"),
            };
        }
        value
    }

    fn remove_field_at_path(value: &mut serde_json::Value, path: &[JsonPathStep]) {
        let (last, parent_path) = path.split_last().expect("field path must be nonempty");
        let JsonPathStep::Field(field) = last else {
            unreachable!("collected paths always end at an object field")
        };
        value_at_path_mut(value, parent_path)
            .as_object_mut()
            .expect("fixture field parent must be an object")
            .remove(field)
            .expect("fixture field must exist");
    }

    fn normalized_coverage_path(path: &[JsonPathStep]) -> String {
        let normalized = path
            .iter()
            .map(|step| match step {
                JsonPathStep::Field(field) => JsonPathStep::Field(field.clone()),
                JsonPathStep::Index(_) => JsonPathStep::Index(usize::MAX),
            })
            .collect::<Vec<_>>();
        format!("{normalized:?}")
    }

    fn incompatible_json_type(value: &serde_json::Value) -> serde_json::Value {
        if value.is_object() {
            serde_json::Value::Array(Vec::new())
        } else {
            serde_json::json!({})
        }
    }

    fn metadata() -> SubmitReservationMetadataFact {
        SubmitReservationMetadataFact {
            client_order_id: "client-1".to_string(),
            submit_reservation_id: "reservation-1".to_string(),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: "binary".to_string(),
            collateral_currency: "USDC".to_string(),
            capital_pool_id: "pool-1".to_string(),
            collateral_group_id: "group-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            side: "buy".to_string(),
            submitted_quantity: "1".to_string(),
            liability_factor: "1".to_string(),
            additive_liability: "0".to_string(),
            reserved_liability: "1".to_string(),
            observed_at_ns: 1,
            source: "submit_admission".to_string(),
        }
    }

    fn settlement() -> SettlementFact {
        SettlementFact {
            strategy_id: "strategy-1".to_string(),
            settlement_key: "settlement-1".to_string(),
            market_id: "market-1".to_string(),
            position_id: "position-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            product_id: "product-1".to_string(),
            outcome_side: super::super::facts::OutcomeSide::Up,
            entry_order_side: "buy".to_string(),
            quantity: "1".to_string(),
            entry_price: "0.4".to_string(),
            family_key: "family-1".to_string(),
            strike_price: "100".to_string(),
            resolution_instrument_id: "BTC-USD".to_string(),
            resolution_ts_event_ns: 2,
            reference_close_price: "101".to_string(),
            payout_per_share: "1".to_string(),
            terminal_value: "1".to_string(),
            realized_pnl: "0.6".to_string(),
            settlement_currency: "USDC".to_string(),
        }
    }

    fn booking_error() -> SettlementBookingErrorFact {
        SettlementBookingErrorFact {
            strategy_id: "strategy-1".to_string(),
            settlement_key: "settlement-1".to_string(),
            market_id: Some("market-1".to_string()),
            position_id: Some("position-1".to_string()),
            instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            resolution_instrument_id: Some("BTC-USD".to_string()),
            reason: super::super::facts::SettlementBookingErrorReason::SettlementBlocked,
            detail: "blocked".to_string(),
            observed_at_ns: 3,
        }
    }

    fn terminal() -> TerminalSettlementFact {
        TerminalSettlementFact {
            settlement_key: "settlement-1".to_string(),
            booking_error: Some(booking_error()),
            lifecycle: super::super::facts::OrderLifecycleFact {
                strategy_id: "strategy-1".to_string(),
                transition:
                    super::super::facts::OrderLifecycleTransition::SettlementBookingTerminal,
                outcome: super::super::facts::OrderLifecycleOutcome::Flat,
                source: "settlement_booking".to_string(),
                market_id: Some("market-1".to_string()),
                instrument_id: Some("YES-USD.POLYMARKET".to_string()),
                position_id: Some("position-1".to_string()),
                client_order_id: None,
                prior_client_order_id: None,
                raw_reason_text: Some("terminal".to_string()),
                order_side: Some("buy".to_string()),
                filled_quantity: Some("1".to_string()),
                residual_quantity: Some("0".to_string()),
                ts_event_ns: Some(4),
            },
        }
    }

    fn order_intent_details() -> OrderIntentDetails {
        OrderIntentDetails {
            strategy_id: "strategy-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            client_order_id: "client-1".to_string(),
            order_side: "buy".to_string(),
            price: "0.4".to_string(),
            quantity: "2".to_string(),
            clamp_outcome: Some(OrderIntentClampOutcome::NotEvaluated {
                reason: OrderIntentClampNotEvaluatedReason::NoVenueTruth,
            }),
            order_fields: OrderIntentOrderFields {
                order_type: "limit".to_string(),
                time_in_force: "gtc".to_string(),
                price: Some("0.4".to_string()),
                trigger_price: None,
                activation_price: None,
                trigger_type: None,
                trigger_instrument_id: None,
                trailing_offset: None,
                trailing_offset_type: None,
                expire_time_unix_nanos: None,
                is_post_only: true,
                is_reduce_only: false,
                is_quote_quantity: false,
            },
        }
    }

    fn basket_details() -> super::super::facts::BasketAdmissionDetails {
        super::super::facts::BasketAdmissionDetails {
            strategy_id: "strategy-1".to_string(),
            execution_client_id: "execution-1".to_string(),
            basket_id: "basket-1".to_string(),
            group_id: "group-1".to_string(),
            leg_instrument_ids: vec![
                "YES-USD.POLYMARKET".to_string(),
                "NO-USD.POLYMARKET".to_string(),
            ],
            total_notional: "10".to_string(),
            leg_order_count: 2,
        }
    }

    fn capital_rebuild() -> CapitalAdmissionRebuildFact {
        CapitalAdmissionRebuildFact {
            observed_at_ns: 5,
            source: "venue_reconciliation".to_string(),
            observed_open_order_count: 2,
            all_open_orders_attributed: true,
            outcome: super::super::facts::CapitalAdmissionRebuildOutcome::Accepted,
            attempted_reservation_count: 2,
            recovered_reservation_count: 2,
            live_reserved_liability: "10".to_string(),
        }
    }

    fn requote_observation() -> RequoteThrottleObservationFact {
        RequoteThrottleObservationFact {
            strategy_id: "strategy-1".to_string(),
            family_key: "family-1".to_string(),
            market_id: Some("market-1".to_string()),
            leg: "up".to_string(),
            now_ms: 6,
            observed_at_ns: 7,
            action_cost_class: super::super::facts::RequoteActionCostClass::CancelResubmit,
            block_reason: super::super::facts::RequoteThrottleBlockReason::RequoteBudgetExhausted,
            bound_by: super::super::facts::RequoteThrottleBound::RestCallWindow,
            submit_commands_in_window: 2,
            submit_command_cap: 3,
            submit_window_ms: 1_000,
            rest_cost_in_window: 4,
            rest_cap_per_minute: 5,
            rest_window_ms: 60_000,
            min_interval_ms: 100,
        }
    }

    fn venue_capture_failure() -> VenueTruthCaptureFailureFact {
        VenueTruthCaptureFailureFact {
            source: "venue_truth".to_string(),
            observed_at_ns: 8,
            endpoint: "venue_snapshot".to_string(),
            error_class: "timeout".to_string(),
            captures_missed: 1,
        }
    }

    fn venue_divergence() -> VenueTruthDivergenceFact {
        VenueTruthDivergenceFact {
            source: "venue_truth".to_string(),
            observed_at_ns: 9,
            account_id: "POLYMARKET-001".to_string(),
            field: "open_orders".to_string(),
            venue_value: "2".to_string(),
            prior_accepted_value: "1".to_string(),
            missing_explanation: "unexplained_open_order_delta".to_string(),
            alarm_class: super::super::facts::VenueTruthDivergenceAlarmClass::TrueDivergence,
        }
    }

    fn loss_halt() -> LossGovernorHaltFact {
        LossGovernorHaltFact {
            snapshot_present: true,
            snapshot_observed_at_ns: Some(10),
            admission_now_ns: 12,
            snapshot_age_ns: Some(2),
            max_snapshot_age_ns: 1,
            snapshot_source: Some("nt_account_snapshot".to_string()),
            has_per_trade_pnl: true,
            has_daily_pnl: true,
            has_rolling_pnl: true,
            has_current_equity: true,
            has_peak_equity: true,
            last_account_state_ts_ns: Some(10),
            last_portfolio_snapshot_ts_ns: Some(10),
            last_position_event_ts_ns: Some(10),
            account_state_count: 1,
            portfolio_snapshot_count: 1,
            position_event_count: 1,
            stale_reason: super::super::facts::StaleLossReason::AgeExceeded,
            stable_halt_key: "age_exceeded:nt_account_snapshot".to_string(),
            retry_count: 1,
            elapsed_since_first_halt_ns: 0,
        }
    }

    fn admission_details() -> AdmissionDetails {
        AdmissionDetails {
            strategy_id: "strategy-1".to_string(),
            execution_client_id: "execution-1".to_string(),
            client_order_id: "client-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            notional: "10".to_string(),
            loss_halt_reasons: vec![LossHaltReason::DailyLossLimit],
            snapshot_present: true,
            snapshot_observed_at_ns: Some(10),
            admission_now_ns: 12,
            snapshot_age_ns: Some(2),
            max_snapshot_age_ns: Some(5),
            snapshot_source: Some(LossSnapshotSource::NtAccountSnapshot),
            per_trade_pnl_present: true,
            daily_pnl_present: true,
            rolling_pnl_present: true,
            current_equity_present: true,
            peak_equity_present: true,
            last_account_state_observed_at_ns: Some(10),
            last_portfolio_snapshot_observed_at_ns: Some(10),
            last_position_event_observed_at_ns: Some(10),
            stale_reason: None,
            loss_snapshot_observed_at_ns: Some(10),
            loss_eval_now_ns: Some(12),
        }
    }

    fn order_reject() -> OrderRejectFact {
        OrderRejectFact {
            reject_source: OrderRejectSource::Venue,
            reject_reason: OrderRejectReason::MinNotionalRejected,
            admission_outcome: None,
            raw_reason_text: Some("minimum notional rejected".to_string()),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            order_side: Some("buy".to_string()),
            raw_price: Some("0.4".to_string()),
            raw_quantity: Some("1".to_string()),
            raw_maker_amount: None,
            raw_taker_amount: None,
            normalized_price: Some("0.4".to_string()),
            normalized_quantity: Some("1".to_string()),
            normalized_maker_amount: None,
            normalized_taker_amount: None,
            venue_price_precision: Some(2),
            venue_size_precision: Some(2),
            venue_min_notional: Some("1".to_string()),
            prior_client_order_id: None,
            client_order_id: "client-1".to_string(),
            retry_count: 1,
            backoff_cooldown_state: None,
            stable_episode_key: "YES-USD.POLYMARKET/venue/min_notional_rejected".to_string(),
            elapsed_ns: 0,
        }
    }

    fn entry_skip() -> EntrySkipFact {
        EntrySkipFact {
            strategy_id: "strategy-1".to_string(),
            now_ms: 30,
            reason_category: super::super::facts::EntrySkipReason::EntryPricingBlocked,
            gate_blocked_by: vec![super::super::facts::EntryBlockReason::WarmupIncomplete],
            pricing_blocked_by: vec![
                super::super::facts::EntryPricingBlockReason::RealizedVolNotReady,
            ],
            market_id: Some("market-1".to_string()),
            phase: "Active".to_string(),
            seconds_to_market_end: Some(60),
            spot_price: Some("100".to_string()),
            reference_current_price: Some("100".to_string()),
            fast_venue_available: true,
            reference_current_price_available: true,
            realized_vol: None,
            realized_vol_source_venue: None,
            realized_vol_source_ts_ms: None,
            realized_vol_gate_result: Some(RvGateResult::MissingSnapshot),
            realized_vol_receive_watermark_ms: None,
            realized_vol_snapshot: None,
            fair_probability_up: None,
            fair_probability_down: None,
            selected_side: None,
            sized_notional: None,
            sized_worst_case_ev_bps: None,
            sized_edge_cents_per_share: None,
            theta_scaled_min_edge_bps: None,
            up_fee_bps: None,
            down_fee_bps: None,
            submission_blocked_reason: Some(
                super::super::facts::EntrySkipReason::EntryPricingBlocked,
            ),
            stale_reference_after_ms: Some(5_000),
            last_reference_ts_ms: Some(29),
            min_liquidity_required: Some("1".to_string()),
            liquidity_available: Some("0".to_string()),
            frozen: false,
            metadata_matches_selection: true,
            fast_venue_incoherent: false,
        }
    }

    fn strategy_input_details<PurposeNumeric>(
        purpose_numeric: impl Fn(&str) -> PurposeNumeric,
    ) -> StrategyInputDetails<PurposeNumeric> {
        StrategyInputDetails {
            strategy_id: "strategy-1".to_string(),
            configured_target_id: "target-1".to_string(),
            market_selection_ruleset_id: "ruleset-1".to_string(),
            market_selection_outcome: StrategyInputMarketSelectionOutcome::Current,
            market_id: Some("market-1".to_string()),
            polymarket_condition_id: Some("condition-1".to_string()),
            polymarket_market_slug: Some("market-slug".to_string()),
            polymarket_question_id: Some("question-1".to_string()),
            up_instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            down_instrument_id: Some("NO-USD.POLYMARKET".to_string()),
            market_selection_timestamp_ms: Some(31),
            selected_market_observed_timestamp_ms: Some(31),
            polymarket_market_start_timestamp_ms: Some(1),
            polymarket_market_end_timestamp_ms: Some(60_000),
            price_to_beat_source: "chainlink".to_string(),
            price_to_beat_value: purpose_numeric("100"),
            reference_quote_ts_event: 31,
            spot_price: purpose_numeric("100"),
            fast_venue_available: true,
            reference_current_price: Some("100".to_string()),
            reference_current_price_available: true,
            reference_current_price_source_id: Some("chainlink".to_string()),
            reference_current_price_failed_over: Some(false),
            realized_volatility: StrategyInputRvState::Absent {
                gate_result: RvGateResult::MissingSnapshot,
                receive_watermark_ms: None,
            },
            seconds_to_market_end: 60,
            pricing_kurtosis: "3".to_string(),
            theta_decay_factor: "1".to_string(),
            theta_scaled_min_edge_bps: purpose_numeric("10"),
            fair_probability_up: purpose_numeric("0.5"),
            uncertainty_band_probability: purpose_numeric("0.01"),
            expected_edge_basis_points: purpose_numeric("20"),
            worst_case_edge_basis_points: purpose_numeric("10"),
            up_worst_case_edge_basis_points: Some("10".to_string()),
            down_worst_case_edge_basis_points: Some("9".to_string()),
            gate_blocked_by: vec![],
            pricing_blocked_by: vec![
                super::super::facts::EntryPricingBlockReason::RealizedVolNotReady,
            ],
            fast_venue_name: Some("binance".to_string()),
            fast_venue_age_ms: Some(1),
            fast_venue_jitter_ms: Some(1),
            fast_venue_incoherent: false,
            lead_agreement_corr: Some("1".to_string()),
            fee_rate_basis_points: purpose_numeric("0"),
            selected_side: None,
        }
    }

    fn exit_decision_details() -> ExitDecisionDetails {
        ExitDecisionDetails {
            strategy_id: "strategy-1".to_string(),
            market_id: Some("market-1".to_string()),
            position_id: Some("position-1".to_string()),
            position_instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            position_outcome_side: Some(super::super::facts::OutcomeSide::Up),
            forced_flat_reasons: vec![],
            spot_price: Some("100".to_string()),
            spot_venue_name: Some("binance".to_string()),
            fast_venue_available: true,
            reference_current_price: Some("100".to_string()),
            reference_current_price_available: true,
            interval_open: Some("100".to_string()),
            fair_probability_up: Some("0.5".to_string()),
            fair_probability_down: Some("0.5".to_string()),
            uncertainty_band_probability: Some("0.01".to_string()),
            up_fee_bps: Some("0".to_string()),
            down_fee_bps: Some("0".to_string()),
            hold_ev_bps: Some("1".to_string()),
            exit_ev_bps: Some("2".to_string()),
            realized_vol: None,
            realized_vol_source_venue: None,
            realized_vol_source_ts_ms: None,
            exit_eval_now_ms: 34,
            exit_trigger_source: ExitTriggerSource::SignalQuote,
            trigger_ts_event_ms: 34,
            trigger_ts_init_ms: Some(34),
            rv_surface_id: "surface-1".to_string(),
            rv_snapshot_as_of_ms: None,
            rv_snapshot_ready: false,
            rv_snapshot_has_ready_realized_vol: Some(false),
            rv_snapshot_receive_watermark_ms: None,
            rv_max_source_age_ms: Some(1_000),
            rv_snapshot_blockers: vec![super::super::facts::RealizedVolBlockReason::NotWarm],
            rv_source_diagnostics: vec![],
            rv_gate_result: RvGateResult::MissingSnapshot,
            rv_future_dating_delta_ms: None,
            exit_hysteresis_bps: "1".to_string(),
            seconds_to_market_end: Some(60),
            ts_ms: 34,
            stale_reference_after_ms: Some(5_000),
            last_reference_ts_ms: Some(34),
            min_liquidity_required: Some("1".to_string()),
            liquidity_available: Some("2".to_string()),
            frozen: false,
            metadata_matches_selection: true,
            fast_venue_incoherent: false,
        }
    }

    #[test]
    fn frozen_wire_domain_declarations_are_exhaustive_and_unique() {
        fn assert_unique<T>(domain: &str, values: Vec<(T, &'static str)>) {
            let mut spellings = std::collections::BTreeSet::new();
            for (_, spelling) in values {
                assert!(
                    spellings.insert(spelling),
                    "{domain} repeats frozen wire spelling {spelling}"
                );
            }
            assert!(!spellings.is_empty(), "{domain} must not be empty");
        }

        macro_rules! assert_domain {
            ($type:ty) => {
                assert_unique(stringify!($type), <$type>::wire_coverage_values())
            };
        }

        assert_domain!(OrderIntentClampNotEvaluatedReason);
        assert_domain!(OrderIntentClampOutcome);
        assert_domain!(BasketAdmissionRejectionReason);
        assert_domain!(CapitalAdmissionRejectionReason);
        assert_domain!(CapitalAdmissionRebuildOutcome);
        assert_domain!(RequoteActionCostClass);
        assert_domain!(RequoteThrottleBound);
        assert_domain!(RequoteThrottleBlockReason);
        assert_domain!(VenueTruthDivergenceAlarmClass);
        assert_domain!(StaleLossReason);
        assert_domain!(LossHaltReason);
        assert_domain!(LossSnapshotSource);
        assert_domain!(LossSnapshotStaleReason);
        assert_domain!(AdmissionRejectionReason);
        assert_domain!(AdmissionDecisionOutcome);
        assert_domain!(OrderRejectSource);
        assert_domain!(OrderRejectReason);
        assert_domain!(EntrySkipReason);
        assert_domain!(ForcedFlatReason);
        assert_domain!(ExposureOccupancy);
        assert_domain!(EntryBlockReason);
        assert_domain!(BinaryOutcomeEdgeBlockReason);
        assert_domain!(EntryPricingBlockReason);
        assert_domain!(RealizedVolPricingComponent);
        assert_domain!(RealizedVolAggregation);
        assert_domain!(RealizedVolSourceClass);
        assert_domain!(RealizedVolSampleKind);
        assert_domain!(RealizedVolSourceStatus);
        assert_domain!(RealizedVolSourceRejectReason);
        assert_domain!(RealizedVolBlockReason);
        assert_domain!(RvGateResult);
        assert_domain!(StrategyInputMarketSelectionOutcome);
        assert_domain!(ExitTriggerSource);
        assert_domain!(ExitBlockedReason);
        assert_domain!(ExitSubmissionOutcome);
        assert_domain!(ExitHoldOutcome);
        assert_domain!(ExitEvaluationDecision);
        assert_domain!(OutcomeSide);
        assert_domain!(SettlementBookingErrorReason);
        assert_domain!(OrderLifecycleTransition);
        assert_domain!(OrderLifecycleOutcome);
    }

    #[test]
    fn current_identity_corpus_is_complete_byte_exact_and_strict() {
        use super::super::generated_contract::{ALL_IDENTITIES, resolve_identity};

        let mut corpus_mismatches = Vec::new();
        for identity in ALL_IDENTITIES.iter().copied() {
            let fixture = positive_corpus(identity);
            assert!(
                fixture.ends_with('\n') && !fixture.starts_with('\n'),
                "{identity:?} corpus must contain newline-terminated JSONL records"
            );
            let mut fixture_lines = fixture.lines();
            let line = fixture_lines
                .next()
                .expect("positive corpus must contain a baseline record");
            let value: serde_json::Value =
                serde_json::from_str(line).expect("positive fixture must be JSON");
            let object = value
                .as_object()
                .expect("positive fixture must have an object envelope");
            let kind = object
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .expect("positive fixture must declare kind");
            let schema_version = object
                .get("schema_version")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
                .expect("positive fixture must declare a u32 schema version");
            let recorded_at_utc_ns = object
                .get("recorded_at_utc_ns")
                .and_then(serde_json::Value::as_i64)
                .expect("positive fixture must declare a recorded timestamp");
            assert_eq!(resolve_identity(kind, schema_version), Some(identity));

            let fact = decode_current_fact(identity, line, 1)
                .expect("positive fixture must decode through the sole dispatch path");
            let coverage_facts = wire_coverage_facts(identity, fact);
            let mut canonical_corpus = Vec::new();
            for (case_index, fact) in coverage_facts.into_iter().enumerate() {
                let expected = fact.clone();
                let reencoded =
                    reencode_current_fact(fact, recorded_at_utc_ns).unwrap_or_else(|error| {
                        panic!(
                            "{identity:?} canonical case {} failed to encode: {error}",
                            case_index + 1
                        )
                    });
                let encoded_line = std::str::from_utf8(reencoded.line())
                    .expect("canonical case must encode as UTF-8");
                let decoded = decode_current_fact(
                    identity,
                    encoded_line.trim_end_matches('\n'),
                    case_index + 1,
                )
                .unwrap_or_else(|error| {
                    panic!(
                        "{identity:?} canonical case {} failed to decode: {error:#}",
                        case_index + 1
                    )
                });
                assert_eq!(
                    decoded,
                    expected,
                    "{identity:?} canonical case {} changed semantic fields across encode/decode",
                    case_index + 1
                );
                canonical_corpus.extend_from_slice(reencoded.line());
            }
            let mut canonical_states = std::collections::BTreeMap::<String, (bool, bool)>::new();
            let mut admitted_optional_paths = std::collections::BTreeSet::new();
            let mut accepted_omissions = std::collections::BTreeMap::new();
            let canonical_text = std::str::from_utf8(&canonical_corpus)
                .expect("canonical corpus bytes must be UTF-8");
            for (case_index, canonical_line) in canonical_text.lines().enumerate() {
                let canonical_value: serde_json::Value = serde_json::from_str(canonical_line)
                    .expect("canonical corpus line must be JSON");
                let canonical_object = canonical_value
                    .as_object()
                    .expect("canonical corpus line must be an object");
                let canonical_payload_key = canonical_object
                    .keys()
                    .find(|key| {
                        !matches!(
                            key.as_str(),
                            "schema_version"
                                | "recorded_at_utc_ns"
                                | "gate_id"
                                | "gate_version"
                                | "kind"
                        )
                    })
                    .expect("canonical corpus line must own a payload member");
                let mut canonical_paths = Vec::new();
                collect_object_field_paths(
                    canonical_object
                        .get(canonical_payload_key)
                        .expect("canonical payload must exist"),
                    &mut vec![JsonPathStep::Field(canonical_payload_key.clone())],
                    &mut canonical_paths,
                );
                for path in canonical_paths {
                    if matches!(
                        path.get(path.len().saturating_sub(2)),
                        Some(JsonPathStep::Field(parent))
                            if parent == "rejection_counters"
                                || parent == "unknown_source_rejections"
                    ) {
                        continue;
                    }
                    let original = value_at_path_mut(&mut canonical_value.clone(), &path).clone();
                    let normalized_path = normalized_coverage_path(&path);
                    let state = canonical_states.entry(normalized_path.clone()).or_default();
                    if original.is_null() {
                        state.0 = true;
                    } else {
                        state.1 = true;
                    }
                    let mut missing = canonical_value.clone();
                    remove_field_at_path(&mut missing, &path);
                    let missing_line = serde_json::to_string(&missing)
                        .expect("missing-field coverage mutation must serialize");
                    let Ok(missing_fact) =
                        decode_current_fact(identity, &missing_line, case_index + 1)
                    else {
                        continue;
                    };
                    let mut explicit_null = canonical_value.clone();
                    *value_at_path_mut(&mut explicit_null, &path) = serde_json::Value::Null;
                    let null_line = serde_json::to_string(&explicit_null)
                        .expect("null-field coverage mutation must serialize");
                    let null_fact =
                        decode_current_fact(identity, &null_line, case_index + 1).unwrap_or_else(
                            |error| {
                                panic!(
                                    "{identity:?} accepts optional omission but rejects null at {path:?}: {error:#}"
                                )
                            },
                        );
                    assert_eq!(
                        null_fact, missing_fact,
                        "{identity:?} optional omission and null differ at {path:?}"
                    );
                    admitted_optional_paths.insert(normalized_path.clone());
                    if original.is_null() {
                        accepted_omissions
                            .entry(normalized_path)
                            .or_insert(missing_line);
                    }
                }
            }
            for path in admitted_optional_paths {
                let (saw_null, saw_present) = canonical_states
                    .get(&path)
                    .copied()
                    .expect("admitted optional path must be present in the canonical corpus");
                assert!(
                    saw_null && saw_present,
                    "{identity:?} canonical corpus does not cover both null and present states for optional field {path}; saw_null={saw_null} saw_present={saw_present}"
                );
            }
            match accepted_noncanonical_corpus(identity) {
                None => assert!(
                    accepted_omissions.is_empty(),
                    "{identity:?} admits omissions but has no noncanonical corpus"
                ),
                Some(frozen_accepted) => {
                    assert!(
                        !accepted_omissions.is_empty(),
                        "{identity:?} owns a noncanonical corpus but admits no omissions"
                    );
                    let generated_accepted = accepted_omissions
                        .values()
                        .map(|line| format!("{line}\n"))
                        .collect::<String>();
                    assert_eq!(
                        frozen_accepted, generated_accepted,
                        "{identity:?} admitted omission corpus drifted"
                    );
                    let canonical_lines = canonical_text
                        .lines()
                        .collect::<std::collections::BTreeSet<_>>();
                    for (line_index, line) in frozen_accepted.lines().enumerate() {
                        let fact = decode_current_fact(identity, line, line_index + 1)
                            .expect("committed noncanonical omission must decode");
                        let canonical = reencode_current_fact(fact, recorded_at_utc_ns).expect(
                            "accepted omission must canonicalize through the production encoder",
                        );
                        let canonical = std::str::from_utf8(canonical.line())
                            .expect("canonical bytes must be UTF-8")
                            .trim_end_matches('\n');
                        assert_ne!(
                            line,
                            canonical,
                            "{identity:?} noncanonical corpus line {} is already canonical",
                            line_index + 1
                        );
                        assert!(
                            canonical_lines.contains(canonical),
                            "{identity:?} noncanonical corpus line {} canonicalized outside the frozen positive corpus",
                            line_index + 1
                        );
                    }
                }
            }
            if canonical_corpus != fixture.as_bytes() {
                corpus_mismatches.push(identity);
            }

            let payload_keys = object
                .keys()
                .filter(|key| {
                    !matches!(
                        key.as_str(),
                        "schema_version"
                            | "recorded_at_utc_ns"
                            | "gate_id"
                            | "gate_version"
                            | "kind"
                    )
                })
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(
                payload_keys.len(),
                1,
                "{identity:?} must own exactly one payload member"
            );
            let payload_key = payload_keys[0].as_str();

            let mut extra_envelope = value.clone();
            extra_envelope
                .as_object_mut()
                .expect("fixture envelope")
                .insert("unexpected_envelope_field".to_string(), true.into());
            mutation_is_rejected(identity, &extra_envelope);

            let mut wrong_gate = value.clone();
            wrong_gate
                .as_object_mut()
                .expect("fixture envelope")
                .insert("gate_id".to_string(), "wrong.gate".into());
            mutation_is_rejected(identity, &wrong_gate);

            let mut empty_gate_version = value.clone();
            empty_gate_version
                .as_object_mut()
                .expect("fixture envelope")
                .insert("gate_version".to_string(), " ".into());
            mutation_is_rejected(identity, &empty_gate_version);

            let mut wrong_kind = value.clone();
            wrong_kind
                .as_object_mut()
                .expect("fixture envelope")
                .insert("kind".to_string(), "wrong_kind".into());
            mutation_is_rejected(identity, &wrong_kind);

            let mut wrong_schema = value.clone();
            wrong_schema
                .as_object_mut()
                .expect("fixture envelope")
                .insert("schema_version".to_string(), 0.into());
            mutation_is_rejected(identity, &wrong_schema);

            let mut invalid_recorded_at = value.clone();
            invalid_recorded_at
                .as_object_mut()
                .expect("fixture envelope")
                .insert("recorded_at_utc_ns".to_string(), 0.into());
            mutation_is_rejected(identity, &invalid_recorded_at);

            for field in [
                "schema_version",
                "recorded_at_utc_ns",
                "gate_id",
                "gate_version",
                "kind",
            ] {
                let mut missing_envelope_field = value.clone();
                missing_envelope_field
                    .as_object_mut()
                    .expect("fixture envelope")
                    .remove(field);
                mutation_is_rejected(identity, &missing_envelope_field);
            }

            let mut missing_payload = value.clone();
            missing_payload
                .as_object_mut()
                .expect("fixture envelope")
                .remove(payload_key);
            mutation_is_rejected(identity, &missing_payload);

            let mut wrong_payload_type = value.clone();
            wrong_payload_type
                .as_object_mut()
                .expect("fixture envelope")
                .insert(payload_key.to_string(), "not-an-object".into());
            mutation_is_rejected(identity, &wrong_payload_type);

            let mut extra_payload_field = value.clone();
            extra_payload_field
                .as_object_mut()
                .and_then(|envelope| envelope.get_mut(payload_key))
                .and_then(serde_json::Value::as_object_mut)
                .expect("fixture payload must be an object")
                .insert("unexpected_payload_field".to_string(), true.into());
            mutation_is_rejected(identity, &extra_payload_field);

            let mut field_paths = Vec::new();
            collect_object_field_paths(
                object.get(payload_key).expect("fixture payload must exist"),
                &mut vec![JsonPathStep::Field(payload_key.to_string())],
                &mut field_paths,
            );
            for path in field_paths {
                let original = value_at_path_mut(&mut value.clone(), &path).clone();

                let mut wrong_type = value.clone();
                *value_at_path_mut(&mut wrong_type, &path) = incompatible_json_type(&original);
                mutation_is_rejected(identity, &wrong_type);

                let mut missing = value.clone();
                remove_field_at_path(&mut missing, &path);
                let missing_line =
                    serde_json::to_string(&missing).expect("missing-field mutation must serialize");
                if let Ok(missing_fact) = decode_current_fact(identity, &missing_line, 1) {
                    let mut explicit_null = value.clone();
                    *value_at_path_mut(&mut explicit_null, &path) = serde_json::Value::Null;
                    let null_line = serde_json::to_string(&explicit_null)
                        .expect("null-field mutation must serialize");
                    let null_fact = decode_current_fact(identity, &null_line, 1).unwrap_or_else(
                        |error| {
                            panic!(
                                "{identity:?} accepts an absent field but rejects the equivalent null at {path:?}: {error:#}"
                            )
                        },
                    );
                    assert_eq!(
                        null_fact, missing_fact,
                        "{identity:?} absent and null field semantics drifted at {path:?}"
                    );
                }
            }
        }
        assert!(
            corpus_mismatches.is_empty(),
            "canonical encoder cases drifted from frozen corpora: {corpus_mismatches:?}"
        );
    }

    #[test]
    fn semantic_validator_classes_reject_well_typed_contradictions() {
        let covered_identities = std::collections::BTreeSet::from([
            KnownIdentity::BlockedStrategyInputObservationV1,
            KnownIdentity::SubmitLinkedStrategyInputSnapshotV1,
            KnownIdentity::EntryOrderIntentV1,
            KnownIdentity::RiskReducingExitOrderIntentV1,
            KnownIdentity::AdmittedEntryAdmissionV1,
            KnownIdentity::RejectedEntryAdmissionV1,
            KnownIdentity::RiskReducingExitAdmissionV1,
            KnownIdentity::ForcedReductionAdmissionV1,
            KnownIdentity::BasketAdmissionGrantedV1,
            KnownIdentity::BasketAdmissionRejectedV1,
            KnownIdentity::CapitalAdmissionRebuildV1,
            KnownIdentity::SubmitReservationMetadataV1,
            KnownIdentity::SubmitReservationFillV1,
            KnownIdentity::EntrySkipObservationV1,
            KnownIdentity::ExitSubmissionDecisionV1,
            KnownIdentity::ExitHoldDecisionV1,
            KnownIdentity::ExitEvaluationV1,
            KnownIdentity::LossGovernorHaltV1,
            KnownIdentity::OrderRejectV1,
            KnownIdentity::OrderLifecycleV1,
            KnownIdentity::RequoteThrottleObservationV1,
            KnownIdentity::SettlementV1,
            KnownIdentity::TerminalSettlementV1,
            KnownIdentity::VenueTruthCaptureFailureV1,
            KnownIdentity::VenueTruthDivergenceV1,
        ]);
        assert_eq!(
            covered_identities,
            super::super::generated_contract::ALL_IDENTITIES
                .iter()
                .copied()
                .collect(),
            "every current identity must retain a well-typed semantic contradiction"
        );

        let mut capital = positive_value(KnownIdentity::CapitalAdmissionRebuildV1);
        capital["decision"]["recovered_reservation_count"] = 3.into();
        mutation_is_rejected(KnownIdentity::CapitalAdmissionRebuildV1, &capital);

        for identity in [
            KnownIdentity::AdmittedEntryAdmissionV1,
            KnownIdentity::RejectedEntryAdmissionV1,
            KnownIdentity::RiskReducingExitAdmissionV1,
            KnownIdentity::ForcedReductionAdmissionV1,
        ] {
            let mut admission = positive_value(identity);
            admission["decision"]["strategy_id"] = "".into();
            mutation_is_rejected(identity, &admission);
        }

        for identity in [
            KnownIdentity::BasketAdmissionGrantedV1,
            KnownIdentity::BasketAdmissionRejectedV1,
        ] {
            let mut basket = positive_value(identity);
            basket["decision"]["leg_order_count"] = 0.into();
            mutation_is_rejected(identity, &basket);
        }

        for identity in [
            KnownIdentity::EntryOrderIntentV1,
            KnownIdentity::RiskReducingExitOrderIntentV1,
        ] {
            let mut intent = positive_value(identity);
            intent["order_intent"]["strategy_id"] = "".into();
            mutation_is_rejected(identity, &intent);
        }

        let mut reject = positive_value(KnownIdentity::OrderRejectV1);
        reject["order_reject"]["retry_count"] = 0.into();
        mutation_is_rejected(KnownIdentity::OrderRejectV1, &reject);

        let mut lifecycle = positive_value(KnownIdentity::OrderLifecycleV1);
        lifecycle["lifecycle"]["strategy_id"] = "".into();
        mutation_is_rejected(KnownIdentity::OrderLifecycleV1, &lifecycle);

        let mut reservation = positive_value(KnownIdentity::SubmitReservationMetadataV1);
        reservation["metadata"]["client_order_id"] = "".into();
        mutation_is_rejected(KnownIdentity::SubmitReservationMetadataV1, &reservation);

        let mut fill = positive_value(KnownIdentity::SubmitReservationFillV1);
        fill["fill"]["client_order_id"] = "".into();
        mutation_is_rejected(KnownIdentity::SubmitReservationFillV1, &fill);

        for (identity, payload_member) in [
            (
                KnownIdentity::SubmitLinkedStrategyInputSnapshotV1,
                "snapshot",
            ),
            (
                KnownIdentity::BlockedStrategyInputObservationV1,
                "blocked_strategy_input_observation",
            ),
        ] {
            let mut strategy_input = positive_value(identity);
            strategy_input[payload_member]["details"]["reference_quote_ts_event"] = 0.into();
            mutation_is_rejected(identity, &strategy_input);
        }

        let mut entry_skip = positive_value(KnownIdentity::EntrySkipObservationV1);
        entry_skip["entry_skip"]["now_ms"] = 0.into();
        mutation_is_rejected(KnownIdentity::EntrySkipObservationV1, &entry_skip);

        let mut exit_submission = positive_value(KnownIdentity::ExitSubmissionDecisionV1);
        exit_submission["exit_decision"]["details"]["strategy_id"] = "".into();
        mutation_is_rejected(KnownIdentity::ExitSubmissionDecisionV1, &exit_submission);

        let mut exit_hold = positive_value(KnownIdentity::ExitHoldDecisionV1);
        exit_hold["exit_decision"]["blocked_reason"] = "no_open_position".into();
        mutation_is_rejected(KnownIdentity::ExitHoldDecisionV1, &exit_hold);

        let mut exit_evaluation = positive_value(KnownIdentity::ExitEvaluationV1);
        exit_evaluation["exit_evaluation"]["exit_eval_now_ms"] = (-1).into();
        mutation_is_rejected(KnownIdentity::ExitEvaluationV1, &exit_evaluation);

        let mut loss = positive_value(KnownIdentity::LossGovernorHaltV1);
        loss["halt"]["retry_count"] = 3.into();
        mutation_is_rejected(KnownIdentity::LossGovernorHaltV1, &loss);

        let mut requote = positive_value(KnownIdentity::RequoteThrottleObservationV1);
        requote["observation"]["submit_command_cap"] = 0.into();
        mutation_is_rejected(KnownIdentity::RequoteThrottleObservationV1, &requote);

        let mut venue_truth = positive_value(KnownIdentity::VenueTruthDivergenceV1);
        venue_truth["divergence"]["source"] = "".into();
        mutation_is_rejected(KnownIdentity::VenueTruthDivergenceV1, &venue_truth);

        let mut capture = positive_value(KnownIdentity::VenueTruthCaptureFailureV1);
        capture["capture_failure"]["captures_missed"] = 0.into();
        mutation_is_rejected(KnownIdentity::VenueTruthCaptureFailureV1, &capture);

        let mut settlement = positive_value(KnownIdentity::SettlementV1);
        settlement["settlement"]["settlement_key"] = "".into();
        mutation_is_rejected(KnownIdentity::SettlementV1, &settlement);

        let mut terminal = positive_value(KnownIdentity::TerminalSettlementV1);
        terminal["terminal_settlement"]["booking_error"]["settlement_key"] =
            "different-settlement-key".into();
        mutation_is_rejected(KnownIdentity::TerminalSettlementV1, &terminal);
    }

    #[test]
    fn startup_recovery_dispositions_have_typed_reducers_for_every_current_identity() {
        use super::super::{
            facts::StartupRecoveryProjections,
            generated_contract::{
                ALL_IDENTITIES, ConsumerDisposition, KnownConsumer, disposition_for,
                fact_for_identity,
            },
            reader::apply_startup_recovery_projections,
        };

        for identity in ALL_IDENTITIES.iter().copied() {
            let line = positive_corpus(identity)
                .lines()
                .next()
                .expect("positive corpus must contain a baseline record");
            let mut recovery = StartupRecoveryProjections::default();
            apply_startup_recovery_projections(identity, line, 1, &mut recovery).unwrap_or_else(
                |error| panic!("{identity:?} must have a typed reducer: {error:#}"),
            );
            for (consumer, projected) in [
                (
                    KnownConsumer::ReservationRecoveryV1,
                    !recovery.reservation.is_empty(),
                ),
                (
                    KnownConsumer::SettlementRecoveryV1,
                    !recovery.settlement.is_empty(),
                ),
                (
                    KnownConsumer::BookingRecoveryV1,
                    !recovery.booking.is_empty(),
                ),
            ] {
                let relevant = matches!(
                    disposition_for(fact_for_identity(identity), consumer),
                    ConsumerDisposition::Relevant(_)
                );
                assert_eq!(
                    projected, relevant,
                    "{identity:?} × {consumer:?} must agree with its typed projection"
                );
            }
        }
    }

    #[test]
    fn backtest_run_guard_dispositions_have_typed_reducers_for_every_current_identity() {
        use super::super::{
            generated_contract::{
                ALL_IDENTITIES, ConsumerDisposition, KnownConsumer, disposition_for,
                fact_for_identity,
            },
            reader::into_backtest_run_guard_event,
        };

        for identity in ALL_IDENTITIES.iter().copied() {
            let line = positive_corpus(identity)
                .lines()
                .next()
                .expect("positive corpus must contain a baseline record");
            let fact = decode_current_fact(identity, line, 1)
                .unwrap_or_else(|error| panic!("{identity:?} must decode: {error:#}"));
            let expected_relevant = matches!(
                disposition_for(
                    fact_for_identity(identity),
                    KnownConsumer::BacktestRunGuardV1,
                ),
                ConsumerDisposition::Relevant(_)
            );
            let reduced = into_backtest_run_guard_event(fact);
            assert_eq!(
                reduced.is_ok(),
                expected_relevant,
                "{identity:?} backtest relevance must agree with its typed reducer"
            );
        }
    }

    #[test]
    fn committed_rejection_corpus_fails_at_the_owned_boundary() {
        use std::io::Write;

        const UNKNOWN_IDENTITY: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/unknown_identity.jsonl"
        ));
        const UNKNOWN_ENUM: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/unknown_enum.jsonl"
        ));
        const WRONG_GATE: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/wrong_gate.jsonl"
        ));
        const WRONG_SINK: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/wrong_sink.jsonl"
        ));
        const EXTRA_FIELD: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/extra_field.jsonl"
        ));
        const TORN_RECORD: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/bolt_v3/current_evidence/reject/torn_record.jsonl"
        ));

        let unknown: serde_json::Value =
            serde_json::from_str(UNKNOWN_IDENTITY.trim_end()).expect("fixture must be JSON");
        assert_eq!(
            super::super::generated_contract::resolve_identity(
                unknown["kind"].as_str().expect("fixture kind"),
                u32::try_from(unknown["schema_version"].as_u64().expect("fixture version"))
                    .expect("fixture version must fit u32"),
            ),
            None
        );
        assert!(
            decode_current_fact(
                KnownIdentity::BasketAdmissionRejectedV1,
                UNKNOWN_ENUM.trim_end(),
                1,
            )
            .is_err()
        );
        assert!(
            decode_current_fact(
                KnownIdentity::BasketAdmissionGrantedV1,
                WRONG_GATE.trim_end(),
                1,
            )
            .is_err()
        );
        assert!(
            decode_current_fact(
                KnownIdentity::BasketAdmissionGrantedV1,
                EXTRA_FIELD.trim_end(),
                1,
            )
            .is_err()
        );

        let mut wrong_sink = tempfile::tempfile().expect("temporary stream");
        wrong_sink
            .write_all(WRONG_SINK.as_bytes())
            .expect("write wrong-sink fixture");
        let error = match super::super::reader::validate_stream(
            &mut wrong_sink,
            super::super::generated_contract::KnownSink::Observation,
            u64::MAX,
        ) {
            Ok(_) => panic!("machine identity in observation stream must fail"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("machine identity in observation")
        );

        assert!(!TORN_RECORD.ends_with('\n'));
        let mut torn = tempfile::tempfile().expect("temporary stream");
        torn.write_all(TORN_RECORD.as_bytes())
            .expect("write torn fixture");
        let error = match super::super::reader::validate_stream(
            &mut torn,
            super::super::generated_contract::KnownSink::Machine,
            u64::MAX,
        ) {
            Ok(_) => panic!("torn final record must fail framing"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("non-newline-terminated final record")
        );
    }

    #[test]
    fn reservation_identity_binding_is_deterministic_and_round_trips() {
        let expected = metadata();
        let encoded = <CurrentCodecs as CodecFor<identities::SubmitReservationMetadataV1>>::encode(
            &expected, 7,
        )
        .expect("valid metadata must encode");
        let line = std::str::from_utf8(encoded.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        let decoded =
            <CurrentCodecs as CodecFor<identities::SubmitReservationMetadataV1>>::decode(line, 1)
                .expect("encoded metadata must decode");

        assert_eq!(decoded, expected);
        assert!(line.contains("\"recorded_at_utc_ns\":7"));
        assert!(matches!(
            <CurrentCodecs as CodecFor<identities::SubmitReservationMetadataV1>>::encode(
                &metadata(),
                0,
            ),
            Err(RecordFailure::Rejected(_))
        ));

        let expected_fill = SubmitReservationFillFact {
            client_order_id: "client-1".to_string(),
            submit_reservation_id: "reservation-1".to_string(),
            trade_id: "trade-1".to_string(),
            instrument_id: "YES-USD.POLYMARKET".to_string(),
            side: "buy".to_string(),
            fill_quantity: "0.5".to_string(),
            observed_at_ns: 2,
            reconciliation: false,
            source: "execution_event".to_string(),
        };
        let encoded_fill =
            <CurrentCodecs as CodecFor<identities::SubmitReservationFillV1>>::encode(
                &expected_fill,
                8,
            )
            .expect("valid fill must encode");
        let fill_line = std::str::from_utf8(encoded_fill.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::SubmitReservationFillV1>>::decode(fill_line, 1,)
                .expect("encoded fill must decode"),
            expected_fill
        );
    }

    #[test]
    fn settlement_identity_binding_is_deterministic_and_round_trips() {
        let expected = settlement();
        let encoded = <CurrentCodecs as CodecFor<identities::SettlementV1>>::encode(&expected, 11)
            .expect("valid settlement must encode");
        let line = std::str::from_utf8(encoded.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        let decoded = <CurrentCodecs as CodecFor<identities::SettlementV1>>::decode(line, 1)
            .expect("encoded settlement must decode");

        assert_eq!(decoded, expected);
        assert!(line.contains("\"recorded_at_utc_ns\":11"));
    }

    #[test]
    fn terminal_settlement_preserves_embedded_booking_error() {
        let expected_terminal = terminal();
        let terminal_record =
            <CurrentCodecs as CodecFor<identities::TerminalSettlementV1>>::encode(
                &expected_terminal,
                13,
            )
            .expect("valid terminal settlement must encode");
        let terminal_line = std::str::from_utf8(terminal_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::TerminalSettlementV1>>::decode(
                terminal_line,
                1,
            )
            .expect("encoded terminal settlement must decode"),
            expected_terminal
        );
    }

    #[test]
    fn order_intent_identities_are_distinct_and_round_trip() {
        let entry = EntryOrderIntentFact {
            details: order_intent_details(),
        };
        let entry_record =
            <CurrentCodecs as CodecFor<identities::EntryOrderIntentV1>>::encode(&entry, 14)
                .expect("valid entry intent must encode");
        let entry_line = std::str::from_utf8(entry_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::EntryOrderIntentV1>>::decode(entry_line, 1)
                .expect("entry intent must decode"),
            entry
        );

        let exit = RiskReducingExitOrderIntentFact {
            details: order_intent_details(),
        };
        let exit_record =
            <CurrentCodecs as CodecFor<identities::RiskReducingExitOrderIntentV1>>::encode(
                &exit, 15,
            )
            .expect("valid exit intent must encode");
        let exit_line = std::str::from_utf8(exit_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::RiskReducingExitOrderIntentV1>>::decode(
                exit_line, 1,
            )
            .expect("exit intent must decode"),
            exit
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::EntryOrderIntentV1>>::decode(exit_line, 1)
                .is_err()
        );
    }

    #[test]
    fn basket_admission_identities_are_distinct_and_round_trip() {
        let granted = BasketAdmissionGrantedFact {
            details: basket_details(),
        };
        let granted_record =
            <CurrentCodecs as CodecFor<identities::BasketAdmissionGrantedV1>>::encode(&granted, 16)
                .expect("valid granted basket must encode");
        let granted_line = std::str::from_utf8(granted_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::BasketAdmissionGrantedV1>>::decode(
                granted_line,
                1,
            )
            .expect("granted basket must decode"),
            granted
        );

        let rejected = BasketAdmissionRejectedFact {
            details: basket_details(),
            reason: super::super::facts::BasketAdmissionRejectionReason::EdgeThreshold,
        };
        let rejected_record =
            <CurrentCodecs as CodecFor<identities::BasketAdmissionRejectedV1>>::encode(
                &rejected, 17,
            )
            .expect("valid rejected basket must encode");
        let rejected_line = std::str::from_utf8(rejected_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::BasketAdmissionRejectedV1>>::decode(
                rejected_line,
                1,
            )
            .expect("rejected basket must decode"),
            rejected
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::BasketAdmissionGrantedV1>>::decode(
                rejected_line,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn capital_admission_rebuild_identity_round_trips() {
        let expected = capital_rebuild();
        let record = <CurrentCodecs as CodecFor<identities::CapitalAdmissionRebuildV1>>::encode(
            &expected, 18,
        )
        .expect("valid capital rebuild must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::CapitalAdmissionRebuildV1>>::decode(line, 1)
                .expect("capital rebuild must decode"),
            expected
        );
    }

    #[test]
    fn order_lifecycle_identity_owns_its_wire_domain() {
        let expected = terminal().lifecycle;
        let record =
            <CurrentCodecs as CodecFor<identities::OrderLifecycleV1>>::encode(&expected, 19)
                .expect("valid lifecycle must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::OrderLifecycleV1>>::decode(line, 1)
                .expect("lifecycle must decode"),
            expected
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::TerminalSettlementV1>>::decode(line, 1).is_err()
        );
    }

    #[test]
    fn requote_observation_round_trips_to_the_observation_identity() {
        let expected = requote_observation();
        let record = <CurrentCodecs as CodecFor<identities::RequoteThrottleObservationV1>>::encode(
            &expected, 20,
        )
        .expect("valid requote observation must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::RequoteThrottleObservationV1>>::decode(line, 1)
                .expect("requote observation must decode"),
            expected
        );
    }

    #[test]
    fn venue_truth_identities_are_distinct_and_round_trip() {
        let failure = venue_capture_failure();
        let failure_record =
            <CurrentCodecs as CodecFor<identities::VenueTruthCaptureFailureV1>>::encode(
                &failure, 21,
            )
            .expect("valid capture failure must encode");
        let failure_line = std::str::from_utf8(failure_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::VenueTruthCaptureFailureV1>>::decode(
                failure_line,
                1,
            )
            .expect("capture failure must decode"),
            failure
        );

        let divergence = venue_divergence();
        let divergence_record =
            <CurrentCodecs as CodecFor<identities::VenueTruthDivergenceV1>>::encode(
                &divergence,
                22,
            )
            .expect("valid divergence must encode");
        let divergence_line = std::str::from_utf8(divergence_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::VenueTruthDivergenceV1>>::decode(
                divergence_line,
                1,
            )
            .expect("divergence must decode"),
            divergence
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::VenueTruthCaptureFailureV1>>::decode(
                divergence_line,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn loss_governor_halt_identity_round_trips() {
        let expected = loss_halt();
        let record =
            <CurrentCodecs as CodecFor<identities::LossGovernorHaltV1>>::encode(&expected, 23)
                .expect("valid loss halt must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::LossGovernorHaltV1>>::decode(line, 1)
                .expect("loss halt must decode"),
            expected
        );
    }

    #[test]
    fn admission_identities_are_purpose_pure_and_round_trip() {
        let admitted = AdmittedEntryAdmissionFact {
            details: admission_details(),
        };
        let admitted_record =
            <CurrentCodecs as CodecFor<identities::AdmittedEntryAdmissionV1>>::encode(
                &admitted, 24,
            )
            .expect("valid admitted entry must encode");
        let admitted_line = std::str::from_utf8(admitted_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::AdmittedEntryAdmissionV1>>::decode(
                admitted_line,
                1,
            )
            .expect("admitted entry must decode"),
            admitted
        );

        let rejected = RejectedEntryAdmissionFact {
            details: admission_details(),
            reason: AdmissionRejectionReason::NotionalCapExceeded,
        };
        let rejected_record =
            <CurrentCodecs as CodecFor<identities::RejectedEntryAdmissionV1>>::encode(
                &rejected, 25,
            )
            .expect("valid rejected entry must encode");
        let rejected_line = std::str::from_utf8(rejected_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::RejectedEntryAdmissionV1>>::decode(
                rejected_line,
                1,
            )
            .expect("rejected entry must decode"),
            rejected
        );

        let exit = RiskReducingExitAdmissionFact {
            details: admission_details(),
            outcome: AdmissionDecisionOutcome::Admitted,
        };
        let exit_record =
            <CurrentCodecs as CodecFor<identities::RiskReducingExitAdmissionV1>>::encode(&exit, 26)
                .expect("valid exit admission must encode");
        let exit_line = std::str::from_utf8(exit_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::RiskReducingExitAdmissionV1>>::decode(
                exit_line, 1,
            )
            .expect("exit admission must decode"),
            exit
        );

        let forced = ForcedReductionAdmissionFact {
            details: admission_details(),
            outcome: AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::KillSwitchForcedReductionCapExceeded,
            ),
        };
        let forced_record =
            <CurrentCodecs as CodecFor<identities::ForcedReductionAdmissionV1>>::encode(
                &forced, 27,
            )
            .expect("valid forced reduction admission must encode");
        let forced_line = std::str::from_utf8(forced_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::ForcedReductionAdmissionV1>>::decode(
                forced_line,
                1,
            )
            .expect("forced reduction admission must decode"),
            forced
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::AdmittedEntryAdmissionV1>>::decode(
                rejected_line,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn order_reject_identity_round_trips_and_rejects_invalid_admission_shape() {
        let expected = order_reject();
        let record = <CurrentCodecs as CodecFor<identities::OrderRejectV1>>::encode(&expected, 28)
            .expect("valid order reject must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::OrderRejectV1>>::decode(line, 1)
                .expect("order reject must decode"),
            expected
        );

        let invalid = OrderRejectFact {
            reject_source: OrderRejectSource::Venue,
            reject_reason: OrderRejectReason::AdmissionRejected,
            admission_outcome: Some(AdmissionDecisionOutcome::Rejected(
                AdmissionRejectionReason::NotionalCapExceeded,
            )),
            ..order_reject()
        };
        assert!(
            <CurrentCodecs as CodecFor<identities::OrderRejectV1>>::encode(&invalid, 29).is_err()
        );
    }

    #[test]
    fn entry_skip_identity_round_trips_to_observation_sink_shape() {
        let expected = entry_skip();
        let record =
            <CurrentCodecs as CodecFor<identities::EntrySkipObservationV1>>::encode(&expected, 31)
                .expect("valid entry skip must encode");
        let line = std::str::from_utf8(record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::EntrySkipObservationV1>>::decode(line, 1)
                .expect("entry skip must decode"),
            expected
        );
    }

    #[test]
    fn strategy_input_identities_are_role_pure_and_model_rv_absence() {
        let blocked = BlockedStrategyInputObservationFact {
            details: strategy_input_details(|value| Some(value.to_string())),
        };
        let blocked_record = <CurrentCodecs as CodecFor<
            identities::BlockedStrategyInputObservationV1,
        >>::encode(&blocked, 32)
        .expect("valid blocked observation must encode");
        let blocked_line = std::str::from_utf8(blocked_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::BlockedStrategyInputObservationV1>>::decode(
                blocked_line,
                1,
            )
            .expect("blocked observation must decode"),
            blocked
        );

        let submit = SubmitLinkedStrategyInputSnapshotFact {
            details: strategy_input_details(str::to_string),
            submission: SubmissionLinkage {
                instrument_id: "YES-USD.POLYMARKET".to_string(),
                order_side: "buy".to_string(),
                price: "0.4".to_string(),
                quantity: "1".to_string(),
                client_order_id: "client-1".to_string(),
            },
        };
        let submit_record = <CurrentCodecs as CodecFor<
            identities::SubmitLinkedStrategyInputSnapshotV1,
        >>::encode(&submit, 33)
        .expect("valid submit snapshot must encode");
        let submit_line = std::str::from_utf8(submit_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::SubmitLinkedStrategyInputSnapshotV1>>::decode(
                submit_line,
                1,
            )
            .expect("submit snapshot must decode"),
            submit
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::BlockedStrategyInputObservationV1>>::decode(
                submit_line,
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn exit_identities_are_role_pure_and_round_trip() {
        let submission = ExitSubmissionDecisionFact {
            details: exit_decision_details(),
            outcome: ExitSubmissionOutcome::Exit,
            submission: SubmissionLinkage {
                instrument_id: "YES-USD.POLYMARKET".to_string(),
                order_side: "sell".to_string(),
                price: "0.6".to_string(),
                quantity: "1".to_string(),
                client_order_id: "exit-1".to_string(),
            },
        };
        let submission_record =
            <CurrentCodecs as CodecFor<identities::ExitSubmissionDecisionV1>>::encode(
                &submission,
                35,
            )
            .expect("valid exit submission must encode");
        let submission_line = std::str::from_utf8(submission_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::ExitSubmissionDecisionV1>>::decode(
                submission_line,
                1,
            )
            .expect("exit submission must decode"),
            submission
        );

        let hold = ExitHoldDecisionFact {
            details: exit_decision_details(),
            outcome: ExitHoldOutcome::Hold,
            blocked_reason: None,
        };
        let hold_record =
            <CurrentCodecs as CodecFor<identities::ExitHoldDecisionV1>>::encode(&hold, 36)
                .expect("valid exit hold must encode");
        let hold_line = std::str::from_utf8(hold_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::ExitHoldDecisionV1>>::decode(hold_line, 1)
                .expect("exit hold must decode"),
            hold
        );
        assert!(
            <CurrentCodecs as CodecFor<identities::ExitHoldDecisionV1>>::decode(
                submission_line,
                1,
            )
            .is_err()
        );

        let evaluation = ExitEvaluationFact {
            position_id: Some("position-1".to_string()),
            market_id: Some("market-1".to_string()),
            instrument_id: Some("YES-USD.POLYMARKET".to_string()),
            client_order_id: Some("exit-1".to_string()),
            exit_eval_now_ms: 37,
            exit_trigger_source: ExitTriggerSource::SignalQuote,
            trigger_ts_event_ms: Some(37),
            trigger_ts_init_ms: Some(37),
            rv_surface_id: "surface-1".to_string(),
            rv_as_of_ms: None,
            rv_ready: false,
            rv_snapshot_receive_watermark_ms: None,
            rv_max_source_age_ms: Some(1_000),
            rv_blockers: vec![RealizedVolBlockReason::NotWarm],
            rv_source_diagnostics: vec!["source_waiting".to_string()],
            rv_gate_result: RvGateResult::MissingSnapshot,
            rv_as_of_minus_now_ms: None,
            spot_price: Some("100".to_string()),
            spot_venue_name: Some("binance".to_string()),
            fast_venue_available: true,
            reference_current_price: Some("100".to_string()),
            reference_current_price_available: true,
            interval_open: Some("100".to_string()),
            fair_probability_up: Some("0.5".to_string()),
            fair_probability_down: Some("0.5".to_string()),
            uncertainty_band_probability: Some("0.01".to_string()),
            up_fee_bps: Some("0".to_string()),
            down_fee_bps: Some("0".to_string()),
            hold_ev_bps: Some("1".to_string()),
            exit_ev_bps: Some("2".to_string()),
            decision: ExitEvaluationDecision::Hold {
                outcome: ExitHoldOutcome::Hold,
                blocked_reason: None,
            },
            forced_flat_reasons: vec![],
        };
        let evaluation_record =
            <CurrentCodecs as CodecFor<identities::ExitEvaluationV1>>::encode(&evaluation, 38)
                .expect("valid exit evaluation must encode");
        let evaluation_line = std::str::from_utf8(evaluation_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::ExitEvaluationV1>>::decode(evaluation_line, 1)
                .expect("exit evaluation must decode"),
            evaluation
        );
    }
}
