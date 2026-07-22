mod admission;
mod basket_admission;
mod lifecycle;
mod order_intent;
mod requote;
mod reservation;
mod settlement;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    facts::{
        BasketAdmissionGrantedFact, BasketAdmissionRejectedFact, CapitalAdmissionRebuildFact,
        EntryOrderIntentFact, OrderIntentClampNotEvaluatedReason, OrderIntentClampOutcome,
        OrderIntentDetails, OrderIntentOrderFields, OrderLifecycleFact, RecoveryFact,
        RequoteThrottleObservationFact, RiskReducingExitOrderIntentFact,
        SettlementBookingErrorFact, SettlementFact, SubmitReservationFillFact,
        SubmitReservationMetadataFact, TerminalSettlementFact,
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

pub(crate) fn encode_entry_order_intent(
    fact: EntryOrderIntentFact,
) -> Result<EncodedEvidenceRecord, RecordFailure> {
    <CurrentCodecs as CodecFor<identities::EntryOrderIntentV1>>::encode(&fact, current_utc_ns()?)
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
}
