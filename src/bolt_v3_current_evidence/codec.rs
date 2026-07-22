mod admission;
mod basket_admission;
mod entry_skip;
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
        BlockedStrategyInputObservationFact, CapitalAdmissionRebuildFact, EntryOrderIntentFact,
        EntrySkipFact, ForcedReductionAdmissionFact, LossGovernorHaltFact, OrderLifecycleFact,
        OrderRejectFact, RecoveryFact, RejectedEntryAdmissionFact, RequoteThrottleObservationFact,
        RiskReducingExitAdmissionFact, RiskReducingExitOrderIntentFact, SettlementBookingErrorFact,
        SettlementFact, SubmitLinkedStrategyInputSnapshotFact, SubmitReservationFillFact,
        SubmitReservationMetadataFact, TerminalSettlementFact, VenueTruthCaptureFailureFact,
        VenueTruthDivergenceFact,
    },
    generated_contract::{
        ConsumerDisposition, IdentityDescriptor, KnownConsumer, KnownIdentity, KnownPurpose,
        current_identity_for_purpose, descriptor_for_identity, disposition_for, fact_for_identity,
        identities,
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

impl CodecFor<identities::SettlementBookingErrorV1> for CurrentCodecs {
    type Input = SettlementBookingErrorFact;
    type Fact = SettlementBookingErrorFact;

    fn encode(
        input: &Self::Input,
        recorded_at_utc_ns: i64,
    ) -> Result<EncodedEvidenceRecord, RecordFailure> {
        settlement::encode_booking_error(input.clone(), recorded_at_utc_ns)
    }

    fn decode(line: &str, line_number: usize) -> Result<Self::Fact> {
        settlement::decode_booking_error(line, line_number)
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

pub(crate) fn encode_settlement_booking_error(
    fact: SettlementBookingErrorFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::SettlementBookingErrorV1>>::encode(
        &fact,
        current_utc_ns()?,
    )
}

pub(crate) fn encode_terminal_settlement(
    fact: TerminalSettlementFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::TerminalSettlementV1>>::encode(&fact, current_utc_ns()?)
}

pub(crate) fn decode_startup_recovery_fact(
    identity: KnownIdentity,
    line: &str,
    line_number: usize,
) -> Result<Option<RecoveryFact>> {
    if !is_startup_relevant(identity) {
        return Ok(None);
    }
    let fact = match identity {
        KnownIdentity::SubmitReservationMetadataV1 => {
            RecoveryFact::ReservationMetadata(<CurrentCodecs as CodecFor<
                identities::SubmitReservationMetadataV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::SubmitReservationFillV1 => {
            RecoveryFact::ReservationFill(<CurrentCodecs as CodecFor<
                identities::SubmitReservationFillV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::SettlementV1 => {
            RecoveryFact::Settlement(
                <CurrentCodecs as CodecFor<identities::SettlementV1>>::decode(line, line_number)?,
            )
        }
        KnownIdentity::SettlementBookingErrorV1 => {
            RecoveryFact::BookingError(<CurrentCodecs as CodecFor<
                identities::SettlementBookingErrorV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::TerminalSettlementV1 => {
            RecoveryFact::TerminalSettlement(<CurrentCodecs as CodecFor<
                identities::TerminalSettlementV1,
            >>::decode(line, line_number)?)
        }
        KnownIdentity::BlockedStrategyInputObservationV1
        | KnownIdentity::SubmitLinkedStrategyInputSnapshotV1
        | KnownIdentity::EntryOrderIntentV1
        | KnownIdentity::RiskReducingExitOrderIntentV1
        | KnownIdentity::AdmittedEntryAdmissionV1
        | KnownIdentity::RejectedEntryAdmissionV1
        | KnownIdentity::RiskReducingExitAdmissionV1
        | KnownIdentity::ForcedReductionAdmissionV1
        | KnownIdentity::BasketAdmissionGrantedV1
        | KnownIdentity::BasketAdmissionRejectedV1
        | KnownIdentity::CapitalAdmissionRebuildV1
        | KnownIdentity::EntrySkipObservationV1
        | KnownIdentity::ExitSubmissionDecisionV1
        | KnownIdentity::ExitHoldDecisionV1
        | KnownIdentity::ExitEvaluationV1
        | KnownIdentity::LossGovernorHaltV1
        | KnownIdentity::OrderRejectV1
        | KnownIdentity::OrderLifecycleV1
        | KnownIdentity::RequoteThrottleObservationV1
        | KnownIdentity::VenueTruthCaptureFailureV1
        | KnownIdentity::VenueTruthDivergenceV1 => {
            unreachable!("startup-irrelevant identity returned relevant disposition")
        }
    };
    Ok(Some(fact))
}

fn is_startup_relevant(identity: KnownIdentity) -> bool {
    let fact = fact_for_identity(identity);
    [
        KnownConsumer::ReservationRecoveryV1,
        KnownConsumer::SettlementRecoveryV1,
        KnownConsumer::BookingRecoveryV1,
    ]
    .into_iter()
    .any(|consumer| {
        matches!(
            disposition_for(fact, consumer),
            ConsumerDisposition::Relevant(_)
        )
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

fn decode<'a, T: Deserialize<'a>>(line: &'a str, line_number: usize) -> Result<T> {
    serde_json::from_str(line).with_context(|| {
        format!("malformed relevant payload at machine evidence line {line_number}")
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
        "identity mismatch at machine evidence line {line_number}"
    );
    ensure!(
        gate_id == descriptor.gate_id,
        "wrong gate_id at machine evidence line {line_number}"
    );
    ensure!(
        recorded_at_utc_ns > 0,
        "recorded_at_utc_ns must be positive at machine evidence line {line_number}"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::facts::{
        AdmissionDecisionOutcome, AdmissionDetails, AdmissionRejectionReason, LossHaltReason,
        LossSnapshotSource, OrderIntentClampNotEvaluatedReason, OrderIntentClampOutcome,
        OrderIntentDetails, OrderIntentOrderFields, OrderRejectFact, OrderRejectReason,
        OrderRejectSource, RvGateResult, StrategyInputDetails, StrategyInputRvState,
        SubmissionLinkage,
    };
    use super::*;

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

    fn strategy_input_details() -> StrategyInputDetails {
        StrategyInputDetails {
            strategy_id: "strategy-1".to_string(),
            configured_target_id: "target-1".to_string(),
            market_selection_ruleset_id: "ruleset-1".to_string(),
            market_selection_outcome: "selected".to_string(),
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
            price_to_beat_value: "100".to_string(),
            reference_quote_ts_event: 31,
            spot_price: "100".to_string(),
            fast_venue_available: true,
            reference_current_price: Some("100".to_string()),
            reference_current_price_available: true,
            reference_current_price_source_id: Some("chainlink".to_string()),
            reference_current_price_failed_over: Some(false),
            realized_volatility: StrategyInputRvState::Absent {
                gate_result: RvGateResult::MissingSnapshot,
            },
            seconds_to_market_end: 60,
            pricing_kurtosis: "3".to_string(),
            theta_decay_factor: "1".to_string(),
            theta_scaled_min_edge_bps: "10".to_string(),
            fair_probability_up: "0.5".to_string(),
            uncertainty_band_probability: "0.01".to_string(),
            expected_edge_basis_points: "20".to_string(),
            worst_case_edge_basis_points: "10".to_string(),
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
            fee_rate_basis_points: "0".to_string(),
            selected_side: None,
        }
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
    fn settlement_error_identities_preserve_complete_semantic_facts() {
        let expected_error = booking_error();
        let error_record =
            <CurrentCodecs as CodecFor<identities::SettlementBookingErrorV1>>::encode(
                &expected_error,
                12,
            )
            .expect("valid booking error must encode");
        let error_line = std::str::from_utf8(error_record.line())
            .expect("encoded evidence must be UTF-8")
            .trim_end_matches('\n');
        assert_eq!(
            <CurrentCodecs as CodecFor<identities::SettlementBookingErrorV1>>::decode(
                error_line, 1,
            )
            .expect("encoded booking error must decode"),
            expected_error
        );

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
            details: strategy_input_details(),
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
            details: strategy_input_details(),
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
}
