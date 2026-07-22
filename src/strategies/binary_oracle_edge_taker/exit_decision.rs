use nautilus_model::{
    enums::{OrderSide, OrderType, PositionSide, TimeInForce, TrailingOffsetType, TriggerType},
    identifiers::{ClientOrderId, InstrumentId, PositionId},
    types::Quantity,
};

use crate::{
    bolt_v3_current_evidence::{
        ExitBlockedReason as EvidenceExitBlockedReason, ExitDecisionDetails,
        ExitTriggerSource as EvidenceExitTriggerSource,
        ForcedFlatReason as EvidenceForcedFlatReason, RealizedVolatilitySourceDiagnosticFact,
        RvGateResult as EvidenceRvGateResult,
    },
    bolt_v3_feed_health::ForcedFlatReason,
    bolt_v3_market_families::OutcomeSide,
    bolt_v3_realized_volatility::RealizedVolBlockReason,
    bolt_v3_timestamp_domain::{LocalReceiveMs, VenueEventMs},
};

use super::{
    EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING, EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING,
    EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE, EXIT_BLOCK_REASON_EXIT_HOLD,
    EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID, EXIT_BLOCK_REASON_EXIT_PRICE_MISSING,
    EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE,
    EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED, EXIT_BLOCK_REASON_NO_OPEN_POSITION,
    EXIT_BLOCK_REASON_OPEN_POSITION_MISSING, EXIT_BLOCK_REASON_POSITION_INTERVAL_ENDED,
    EXIT_BLOCK_REASON_POSITION_INTERVAL_UNKNOWN, entry_decision::ForcedFlatEvidenceInputs,
    forced_flat_reason_to_evidence, option_evidence_number, orders::ConfiguredNtOrderTemplate,
    orders::ExitOrderExecutionConfig, outcome_side_to_evidence, selection::SelectionPhase,
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitEvaluation {
    pub(super) realized_volatility_receipt: ExitRealizedVolatilityGateReceipt,
    pub(super) position_outcome_side: Option<OutcomeSide>,
    pub(super) forced_flat_reasons: Vec<ForcedFlatReason>,
    pub(super) hold_ev_bps: Option<f64>,
    pub(super) exit_ev_bps: Option<f64>,
    pub(super) exit_decision: Option<ExitDecision>,
    pub(super) blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitRealizedVolatilityGateReceipt {
    pub(super) gate_result: EvidenceRvGateResult,
    pub(super) surface_id: String,
    pub(super) max_source_age_ms: u64,
    pub(super) evaluation_receive_ms: Option<LocalReceiveMs>,
    pub(super) snapshot_as_of_ms: Option<u64>,
    pub(super) snapshot_receive_watermark_ms: Option<LocalReceiveMs>,
    pub(super) snapshot_ready: bool,
    pub(super) snapshot_has_ready_realized_vol: bool,
    pub(super) realized_vol: Option<f64>,
    pub(super) realized_vol_source_venue: Option<String>,
    pub(super) realized_vol_source_ts_ms: Option<u64>,
    pub(super) raw_snapshot_blockers: Vec<RealizedVolBlockReason>,
    pub(super) source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticFact>,
    pub(super) snapshot_as_of_minus_trigger_event_ms: Option<i128>,
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
    pub(super) uncertainty_band_probability: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitSubmissionDecision {
    pub(super) evaluation: ExitEvaluation,
    pub(super) instrument_id: Option<InstrumentId>,
    pub(super) order_type: Option<OrderType>,
    pub(super) order_side: Option<OrderSide>,
    pub(super) position_side: Option<PositionSide>,
    pub(super) time_in_force: Option<TimeInForce>,
    pub(super) price: Option<f64>,
    pub(super) quantity: Option<Quantity>,
    pub(super) client_order_id: Option<ClientOrderId>,
    pub(super) is_post_only: Option<bool>,
    pub(super) is_reduce_only: Option<bool>,
    pub(super) is_quote_quantity: Option<bool>,
    pub(super) expire_time_unix_nanos: Option<u64>,
    pub(super) trigger_price: Option<f64>,
    pub(super) activation_price: Option<f64>,
    pub(super) trigger_type: Option<TriggerType>,
    pub(super) trigger_instrument_id: Option<InstrumentId>,
    pub(super) trailing_offset: Option<f64>,
    pub(super) trailing_offset_type: Option<TrailingOffsetType>,
    pub(super) blocked_reason: Option<&'static str>,
    pub(super) forced_flat_reasons: Vec<ForcedFlatReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExitEvaluationTriggerContext {
    pub(super) source: EvidenceExitTriggerSource,
    pub(super) ts_event_ms: u64,
    pub(super) ts_init_ms: Option<u64>,
}

impl ExitEvaluationTriggerContext {
    pub(super) const fn from_market_data(
        source: EvidenceExitTriggerSource,
        ts_event_ms: u64,
        receive_ms: LocalReceiveMs,
    ) -> Self {
        Self {
            source,
            ts_event_ms,
            ts_init_ms: Some(receive_ms.value()),
        }
    }

    pub(super) const fn from_local_selection_handler(receive_ms: LocalReceiveMs) -> Self {
        Self::from_market_data(
            EvidenceExitTriggerSource::SelectionUpdate,
            receive_ms.value(),
            receive_ms,
        )
    }

    #[cfg(test)]
    pub(super) const fn new(
        source: EvidenceExitTriggerSource,
        ts_event_ms: u64,
        ts_init_ms: Option<u64>,
    ) -> Self {
        Self {
            source,
            ts_event_ms,
            ts_init_ms,
        }
    }

    #[cfg(test)]
    pub(super) const fn unknown(now_ms: u64) -> Self {
        Self::new(EvidenceExitTriggerSource::Unknown, now_ms, Some(now_ms))
    }

    #[cfg(test)]
    pub(super) const fn diagnostic_missing(
        source: EvidenceExitTriggerSource,
        ts_event_ms: u64,
    ) -> Self {
        Self::new(source, ts_event_ms, None)
    }

    /// Every runtime trigger is evaluated after it enters the process, so
    /// `ts_init` is the universal clock for pricing freshness regardless of
    /// whether the trigger also owns a venue event timestamp.
    pub(super) const fn receive_ms(self) -> Option<LocalReceiveMs> {
        match self.ts_init_ms {
            Some(value) => Some(LocalReceiveMs::new(value)),
            None => None,
        }
    }

    pub(super) const fn venue_event_ms(self) -> Option<VenueEventMs> {
        match self.source {
            EvidenceExitTriggerSource::SignalQuote
            | EvidenceExitTriggerSource::ReferenceUpdate
            | EvidenceExitTriggerSource::BookDelta => Some(VenueEventMs::new(self.ts_event_ms)),
            EvidenceExitTriggerSource::SelectionUpdate
            | EvidenceExitTriggerSource::Unknown
            | EvidenceExitTriggerSource::Other => None,
        }
    }
}

impl ExitSubmissionDecision {
    pub(super) fn execution_config(&self) -> Option<ExitOrderExecutionConfig> {
        Some(ExitOrderExecutionConfig {
            side: self.order_side?,
            position_side: self.position_side?,
            order_template: ConfiguredNtOrderTemplate {
                order_type: self.order_type?,
                time_in_force: self.time_in_force?,
                expire_time_unix_nanos: self.expire_time_unix_nanos,
                trigger_price: self.trigger_price,
                activation_price: self.activation_price,
                trigger_type: self.trigger_type,
                trigger_instrument_id: self.trigger_instrument_id,
                trailing_offset: self.trailing_offset,
                trailing_offset_type: self.trailing_offset_type,
                is_post_only: self.is_post_only?,
                is_reduce_only: self.is_reduce_only?,
                is_quote_quantity: self.is_quote_quantity?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExitEvaluationLogFields {
    pub(super) market_id: Option<String>,
    pub(super) phase: SelectionPhase,
    pub(super) position_outcome_side: Option<OutcomeSide>,
    pub(super) position_id: Option<PositionId>,
    pub(super) position_instrument_id: Option<InstrumentId>,
    pub(super) position_quantity: Option<Quantity>,
    pub(super) position_avg_px_open: Option<f64>,
    pub(super) forced_flat_reasons: Vec<ForcedFlatReason>,
    pub(super) spot_price: Option<f64>,
    pub(super) spot_venue_name: Option<String>,
    pub(super) fast_venue_available: bool,
    pub(super) reference_current_price: Option<f64>,
    pub(super) interval_open: Option<f64>,
    pub(super) seconds_to_expiry: Option<u64>,
    pub(super) realized_vol: Option<f64>,
    pub(super) realized_vol_source_venue: Option<String>,
    pub(super) realized_vol_source_ts_ms: Option<u64>,
    pub(super) rv_surface_id: String,
    pub(super) rv_snapshot_as_of_ms: Option<u64>,
    pub(super) rv_snapshot_ready: bool,
    pub(super) rv_snapshot_has_ready_realized_vol: Option<bool>,
    pub(super) rv_snapshot_receive_watermark_ms: Option<u64>,
    pub(super) rv_max_source_age_ms: Option<u64>,
    pub(super) rv_snapshot_blockers: Vec<crate::bolt_v3_current_evidence::RealizedVolBlockReason>,
    pub(super) rv_source_diagnostics: Vec<RealizedVolatilitySourceDiagnosticFact>,
    pub(super) rv_gate_result: EvidenceRvGateResult,
    pub(super) rv_future_dating_delta_ms: Option<u64>,
    pub(super) exit_eval_now_ms: u64,
    pub(super) exit_trigger_source: EvidenceExitTriggerSource,
    pub(super) trigger_ts_event_ms: u64,
    pub(super) trigger_ts_init_ms: Option<u64>,
    pub(super) pricing_kurtosis: f64,
    pub(super) exit_hysteresis_bps: i64,
    pub(super) fair_probability_up: Option<f64>,
    pub(super) fair_probability_down: Option<f64>,
    pub(super) uncertainty_band_probability: Option<f64>,
    pub(super) up_fee_bps: Option<f64>,
    pub(super) down_fee_bps: Option<f64>,
    pub(super) hold_ev_bps: Option<f64>,
    pub(super) exit_ev_bps: Option<f64>,
    pub(super) exit_decision: Option<ExitDecision>,
    pub(super) historical_entry_fee_rate_known: bool,
    pub(super) historical_entry_fee_rate_reason: &'static str,
    pub(super) final_fee_amount_known: bool,
    pub(super) final_fee_amount_reason: &'static str,
    pub(super) submission_instrument_id: Option<InstrumentId>,
    pub(super) submission_order_side: Option<OrderSide>,
    pub(super) submission_price: Option<f64>,
    pub(super) submission_quantity: Option<Quantity>,
    pub(super) submission_client_order_id: Option<ClientOrderId>,
    pub(super) submission_blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExitDecision {
    Hold,
    Exit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExitDecisionDedupeKey {
    pub(super) market_id: Option<String>,
    pub(super) position_id: Option<String>,
    pub(super) forced_flat_reasons: Vec<EvidenceForcedFlatReason>,
    pub(super) exit_decision: ExitDecisionDisposition,
    pub(super) blocked_reason: Option<EvidenceExitBlockedReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ExitDecisionDisposition {
    Exit,
    ExitFailClosed,
    Hold,
    Blocked,
}

pub(super) fn exit_decision_details(
    strategy_id: String,
    ts_ms: u64,
    fields: &ExitEvaluationLogFields,
    forced_flat_inputs: ForcedFlatEvidenceInputs,
) -> ExitDecisionDetails {
    let spot_price = option_evidence_number(fields.spot_price);
    let reference_current_price = option_evidence_number(fields.reference_current_price);
    let interval_open = option_evidence_number(fields.interval_open);
    let fair_probability_up = option_evidence_number(fields.fair_probability_up);
    let fair_probability_down = option_evidence_number(fields.fair_probability_down);
    let uncertainty_band_probability = option_evidence_number(fields.uncertainty_band_probability);
    let up_fee_bps = option_evidence_number(fields.up_fee_bps);
    let down_fee_bps = option_evidence_number(fields.down_fee_bps);
    let hold_ev_bps = option_evidence_number(fields.hold_ev_bps);
    let exit_ev_bps = option_evidence_number(fields.exit_ev_bps);
    let realized_vol = option_evidence_number(fields.realized_vol);
    ExitDecisionDetails {
        strategy_id,
        market_id: fields.market_id.clone(),
        position_id: fields
            .position_id
            .map(|position_id| position_id.to_string()),
        position_instrument_id: fields
            .position_instrument_id
            .map(|instrument_id| instrument_id.to_string()),
        position_outcome_side: fields.position_outcome_side.map(outcome_side_to_evidence),
        forced_flat_reasons: fields
            .forced_flat_reasons
            .iter()
            .map(forced_flat_reason_to_evidence)
            .collect(),
        spot_price,
        spot_venue_name: fields.spot_venue_name.clone(),
        fast_venue_available: fields.fast_venue_available,
        reference_current_price_available: reference_current_price.is_some(),
        reference_current_price,
        interval_open,
        fair_probability_up,
        fair_probability_down,
        uncertainty_band_probability,
        up_fee_bps,
        down_fee_bps,
        hold_ev_bps,
        exit_ev_bps,
        realized_vol,
        realized_vol_source_venue: fields.realized_vol_source_venue.clone(),
        realized_vol_source_ts_ms: fields.realized_vol_source_ts_ms,
        exit_eval_now_ms: fields.exit_eval_now_ms,
        exit_trigger_source: fields.exit_trigger_source,
        trigger_ts_event_ms: fields.trigger_ts_event_ms,
        trigger_ts_init_ms: fields.trigger_ts_init_ms,
        rv_surface_id: fields.rv_surface_id.clone(),
        rv_snapshot_as_of_ms: fields.rv_snapshot_as_of_ms,
        rv_snapshot_ready: fields.rv_snapshot_ready,
        rv_snapshot_has_ready_realized_vol: fields.rv_snapshot_has_ready_realized_vol,
        rv_snapshot_receive_watermark_ms: fields.rv_snapshot_receive_watermark_ms,
        rv_max_source_age_ms: fields.rv_max_source_age_ms,
        rv_snapshot_blockers: fields.rv_snapshot_blockers.clone(),
        rv_source_diagnostics: fields.rv_source_diagnostics.clone(),
        rv_gate_result: fields.rv_gate_result,
        rv_future_dating_delta_ms: fields.rv_future_dating_delta_ms,
        exit_hysteresis_bps: fields.exit_hysteresis_bps.to_string(),
        seconds_to_market_end: fields.seconds_to_expiry,
        ts_ms,
        stale_reference_after_ms: forced_flat_inputs.stale_reference_after_ms,
        last_reference_ts_ms: forced_flat_inputs.last_reference_ts_ms,
        min_liquidity_required: forced_flat_inputs.min_liquidity_required,
        liquidity_available: forced_flat_inputs.liquidity_available,
        frozen: forced_flat_inputs.frozen,
        metadata_matches_selection: forced_flat_inputs.metadata_matches_selection,
        fast_venue_incoherent: forced_flat_inputs.fast_venue_incoherent,
    }
}

pub(super) fn exit_block_reason_to_evidence(reason: &str) -> EvidenceExitBlockedReason {
    match reason {
        EXIT_BLOCK_REASON_NO_OPEN_POSITION => EvidenceExitBlockedReason::NoOpenPosition,
        EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING => EvidenceExitBlockedReason::ExitAlreadyPending,
        EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING => {
            EvidenceExitBlockedReason::EntryOrderStillWorking
        }
        EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE => {
            EvidenceExitBlockedReason::ExitDecisionUnavailable
        }
        EXIT_BLOCK_REASON_EXIT_HOLD => EvidenceExitBlockedReason::ExitHold,
        EXIT_BLOCK_REASON_POSITION_INTERVAL_ENDED => {
            EvidenceExitBlockedReason::PositionIntervalEnded
        }
        EXIT_BLOCK_REASON_POSITION_INTERVAL_UNKNOWN => {
            EvidenceExitBlockedReason::PositionIntervalUnknown
        }
        EXIT_BLOCK_REASON_OPEN_POSITION_MISSING => EvidenceExitBlockedReason::OpenPositionMissing,
        EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID => {
            EvidenceExitBlockedReason::ExitOrderConfigInvalid
        }
        EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED => {
            EvidenceExitBlockedReason::ExitQuoteQuantityUnsupported
        }
        EXIT_BLOCK_REASON_EXIT_PRICE_MISSING => EvidenceExitBlockedReason::ExitPriceMissing,
        EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE => {
            EvidenceExitBlockedReason::ExitQuantityNotPositive
        }
        _ => unreachable!("unknown exit blocked reason `{reason}`"),
    }
}

/// Stable key for #885 exit-evaluation evidence flood-gating. Two exit evaluations
/// with the same key produce the same RCA story, so only the first is recorded
/// durably (subsequent identical ticks are suppressed). Deliberately excludes the
/// client_order_id (re-minted per attempt) and timestamps so a per-tick flood
/// collapses to one record. `Ord` lets it key a `BTreeMap` without a new import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ExitOutcomeKey {
    pub(super) exit_decision: ExitDecisionDisposition,
    pub(super) submission_blocked_reason: Option<&'static str>,
    pub(super) rv_gate_result: EvidenceRvGateResult,
}

/// Map the strategy-internal [`ExitDecision`] to the closed evidence enum. `None`
/// (blocked before a decision was computed, e.g. exit already pending or entry order
/// still working) maps to `Hold` — no exit action was taken — and the durable record's
/// `submission_blocked_reason` field separately explains why.
pub(super) fn exit_decision_evidence_from_optional(
    decision: Option<ExitDecision>,
) -> ExitDecisionDisposition {
    match decision {
        Some(ExitDecision::Hold) | None => ExitDecisionDisposition::Hold,
        Some(ExitDecision::Exit) => ExitDecisionDisposition::Exit,
    }
}

pub(super) fn evaluate_exit_decision(
    hold_ev_bps: Option<f64>,
    exit_ev_bps: Option<f64>,
    exit_hysteresis_bps: f64,
) -> ExitDecision {
    let Some(hold_ev_bps) = hold_ev_bps.filter(|value| value.is_finite()) else {
        return ExitDecision::Hold;
    };
    let Some(exit_ev_bps) = exit_ev_bps.filter(|value| value.is_finite()) else {
        return ExitDecision::Hold;
    };
    if !exit_hysteresis_bps.is_finite() {
        return ExitDecision::Hold;
    }

    if exit_ev_bps > hold_ev_bps + exit_hysteresis_bps {
        ExitDecision::Exit
    } else {
        ExitDecision::Hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_decision_evidence_preserves_observed_inputs_from_log_fields() {
        let fields = ExitEvaluationLogFields {
            market_id: Some("market-one".to_string()),
            phase: SelectionPhase::Freeze,
            position_outcome_side: Some(OutcomeSide::Up),
            position_id: Some(PositionId::from("position-one")),
            position_instrument_id: Some(InstrumentId::from("UP.POLYMARKET")),
            position_quantity: Some(Quantity::new(7.0, 2)),
            position_avg_px_open: Some(0.45),
            forced_flat_reasons: vec![ForcedFlatReason::Freeze],
            spot_price: Some(3_100.5),
            spot_venue_name: Some("venue-one".to_string()),
            fast_venue_available: true,
            reference_current_price: Some(3_099.75),
            interval_open: Some(3_100.0),
            seconds_to_expiry: Some(240),
            realized_vol: Some(1.5),
            realized_vol_source_venue: Some("source-one".to_string()),
            realized_vol_source_ts_ms: Some(1_200),
            rv_surface_id: "surface-one".to_string(),
            rv_snapshot_as_of_ms: Some(1_200),
            rv_snapshot_ready: true,
            rv_snapshot_has_ready_realized_vol: Some(true),
            rv_snapshot_receive_watermark_ms: Some(1_195),
            rv_max_source_age_ms: Some(500),
            rv_snapshot_blockers: Vec::new(),
            rv_source_diagnostics: Vec::new(),
            rv_gate_result: EvidenceRvGateResult::Accepted,
            rv_future_dating_delta_ms: None,
            exit_eval_now_ms: 1_200,
            exit_trigger_source: EvidenceExitTriggerSource::SignalQuote,
            trigger_ts_event_ms: 1_190,
            trigger_ts_init_ms: Some(1_195),
            pricing_kurtosis: 3.0,
            exit_hysteresis_bps: 5,
            fair_probability_up: Some(0.55),
            fair_probability_down: Some(0.45),
            uncertainty_band_probability: Some(0.02),
            up_fee_bps: Some(1.25),
            down_fee_bps: Some(2.5),
            hold_ev_bps: Some(12.5),
            exit_ev_bps: Some(11.25),
            exit_decision: Some(ExitDecision::Exit),
            historical_entry_fee_rate_known: true,
            historical_entry_fee_rate_reason: "known",
            final_fee_amount_known: false,
            final_fee_amount_reason: "pending_fill",
            submission_instrument_id: Some(InstrumentId::from("SUBMISSION.POLYMARKET")),
            submission_order_side: Some(OrderSide::Sell),
            submission_price: Some(0.49),
            submission_quantity: Some(Quantity::new(7.0, 2)),
            submission_client_order_id: Some(ClientOrderId::from("client-order-one")),
            submission_blocked_reason: None,
        };

        let evidence = exit_decision_details(
            "strategy-one".to_string(),
            1_201,
            &fields,
            ForcedFlatEvidenceInputs {
                stale_reference_after_ms: Some(1_500),
                last_reference_ts_ms: Some(1_000),
                min_liquidity_required: Some("100".to_string()),
                liquidity_available: Some("80".to_string()),
                frozen: true,
                metadata_matches_selection: true,
                fast_venue_incoherent: false,
            },
        );

        assert_eq!(evidence.spot_price.as_deref(), Some("3100.5"));
        assert_eq!(evidence.spot_venue_name.as_deref(), Some("venue-one"));
        assert!(evidence.fast_venue_available);
        assert_eq!(evidence.reference_current_price.as_deref(), Some("3099.75"));
        assert!(evidence.reference_current_price_available);
        assert_eq!(evidence.interval_open.as_deref(), Some("3100"));
        assert_eq!(evidence.fair_probability_up.as_deref(), Some("0.55"));
        assert_eq!(evidence.fair_probability_down.as_deref(), Some("0.45"));
        assert_eq!(
            evidence.uncertainty_band_probability.as_deref(),
            Some("0.02")
        );
        assert_eq!(evidence.up_fee_bps.as_deref(), Some("1.25"));
        assert_eq!(evidence.down_fee_bps.as_deref(), Some("2.5"));
        assert_eq!(
            evidence.position_instrument_id.as_deref(),
            Some("UP.POLYMARKET")
        );
    }
}
