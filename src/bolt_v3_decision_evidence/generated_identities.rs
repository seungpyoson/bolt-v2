// @generated from config/decision-evidence-identities.toml. Do not edit.

use anyhow::{Result, anyhow};

pub const BOLT_V3_ADMISSION_DECISION_RECORD_KIND: &str = "admission_decision";
pub const BOLT_V3_BASKET_ADMISSION_DECISION_RECORD_KIND: &str = "basket_admission_decision";
pub const BOLT_V3_CAPITAL_ADMISSION_REBUILD_RECORD_KIND: &str = "capital_admission_rebuild";
pub const BOLT_V3_ENTRY_SKIP_COMPLETE_REASON_RECORD_KIND: &str = "entry_skip_complete_reason";
pub const BOLT_V3_ENTRY_SKIP_RECORD_KIND: &str = "entry_skip";
pub const BOLT_V3_EXIT_DECISION_RECORD_KIND: &str = "exit_decision";
pub const BOLT_V3_EXIT_EVALUATION_RECORD_KIND: &str = "exit_evaluation";
pub const BOLT_V3_LOSS_GOVERNOR_HALT_RECORD_KIND: &str = "loss_governor_halt";
pub const BOLT_V3_ORDER_INTENT_RECORD_KIND: &str = "order_intent";
pub const BOLT_V3_ORDER_LIFECYCLE_RECORD_KIND: &str = "order_lifecycle";
pub const BOLT_V3_ORDER_REJECT_RECORD_KIND: &str = "order_reject";
pub const BOLT_V3_POSITION_SIZER_REBUILD_LEGACY_RECORD_KIND: &str = "position_sizer_rebuild";
pub const BOLT_V3_REQUOTE_THROTTLE_RECORD_KIND: &str = "requote_throttle";
pub const BOLT_V3_SETTLEMENT_BOOKING_ERROR_RECORD_KIND: &str = "settlement_booking_error";
pub const BOLT_V3_SETTLEMENT_RECORD_KIND: &str = "settlement";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_RECORD_KIND: &str = "strategy_input_snapshot";
pub const BOLT_V3_SUBMIT_RESERVATION_FILL_RECORD_KIND: &str = "submit_reservation_fill";
pub const BOLT_V3_SUBMIT_RESERVATION_METADATA_RECORD_KIND: &str = "submit_reservation_metadata";
pub const BOLT_V3_TERMINAL_SETTLEMENT_RECORD_KIND: &str = "terminal_settlement";
pub const BOLT_V3_VENUE_TRUTH_CAPTURE_FAILURE_RECORD_KIND: &str = "venue_truth_capture_failure";
pub const BOLT_V3_VENUE_TRUTH_DIVERGENCE_RECORD_KIND: &str = "venue_truth_divergence";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EvidenceRecordIdentity {
    StrategyInputSnapshotV14,
    StrategyInputSnapshotV15,
    OrderIntentV14,
    OrderIntentV15,
    AdmissionDecisionV14,
    AdmissionDecisionV15,
    BasketAdmissionDecisionV14,
    BasketAdmissionDecisionV15,
    CapitalAdmissionRebuildV14,
    CapitalAdmissionRebuildV15,
    SubmitReservationMetadataV13,
    SubmitReservationMetadataV14,
    SubmitReservationMetadataV15,
    SubmitReservationFillV13,
    SubmitReservationFillV14,
    SubmitReservationFillV15,
    EntrySkipLegacyV14,
    EntrySkipLegacyV15,
    EntrySkipCompleteReasonV15,
    ExitDecisionV14,
    ExitDecisionV15,
    ExitEvaluationV14,
    ExitEvaluationV15,
    LossGovernorHaltV14,
    LossGovernorHaltV15,
    OrderRejectV14,
    OrderRejectV15,
    SettlementV14,
    SettlementV15,
    SettlementBookingErrorV14,
    SettlementBookingErrorV15,
    TerminalSettlementV14,
    TerminalSettlementV15,
    OrderLifecycleV14,
    OrderLifecycleV15,
    VenueTruthCaptureFailureV14,
    VenueTruthCaptureFailureV15,
    VenueTruthDivergenceV14,
    VenueTruthDivergenceV15,
    RequoteThrottleV14,
    RequoteThrottleV15,
    AdmissionDecisionLegacyV13,
    PositionSizerRebuildLegacyV13,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceConsumer {
    EntryDecisionChain,
    SubmitReservation,
    ExitEvaluation,
    LossGovernorHalt,
    OrderReject,
    Settlement,
    SettlementBookingError,
    TerminalSettlement,
    ShadowPnl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceDecodeAction {
    StrategyInputSnapshot,
    OrderIntent,
    AdmissionDecision,
    BasketAdmissionDecision,
    CapitalAdmissionRebuild,
    SubmitReservationMetadata,
    SubmitReservationFill,
    EntrySkipV15,
    EntrySkipCompleteReason,
    ExitDecision,
    ExitEvaluation,
    LossGovernorHalt,
    OrderReject,
    Settlement,
    SettlementBookingError,
    TerminalSettlement,
    OrderLifecycle,
    VenueTruthCaptureFailure,
    VenueTruthDivergence,
    RequoteThrottle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvidenceIdentityMetadata {
    pub kind: &'static str,
    pub schema_version: u32,
    pub gate_id: &'static str,
    pub decode_action: EvidenceDecodeAction,
}

pub fn resolve_evidence_record_identity(kind: &str, schema_version: u32) -> Result<EvidenceRecordIdentity> {
    match (kind, schema_version) {
        ("strategy_input_snapshot", 14) => Ok(EvidenceRecordIdentity::StrategyInputSnapshotV14),
        ("strategy_input_snapshot", 15) => Ok(EvidenceRecordIdentity::StrategyInputSnapshotV15),
        ("order_intent", 14) => Ok(EvidenceRecordIdentity::OrderIntentV14),
        ("order_intent", 15) => Ok(EvidenceRecordIdentity::OrderIntentV15),
        ("admission_decision", 14) => Ok(EvidenceRecordIdentity::AdmissionDecisionV14),
        ("admission_decision", 15) => Ok(EvidenceRecordIdentity::AdmissionDecisionV15),
        ("basket_admission_decision", 14) => Ok(EvidenceRecordIdentity::BasketAdmissionDecisionV14),
        ("basket_admission_decision", 15) => Ok(EvidenceRecordIdentity::BasketAdmissionDecisionV15),
        ("capital_admission_rebuild", 14) => Ok(EvidenceRecordIdentity::CapitalAdmissionRebuildV14),
        ("capital_admission_rebuild", 15) => Ok(EvidenceRecordIdentity::CapitalAdmissionRebuildV15),
        ("submit_reservation_metadata", 13) => Ok(EvidenceRecordIdentity::SubmitReservationMetadataV13),
        ("submit_reservation_metadata", 14) => Ok(EvidenceRecordIdentity::SubmitReservationMetadataV14),
        ("submit_reservation_metadata", 15) => Ok(EvidenceRecordIdentity::SubmitReservationMetadataV15),
        ("submit_reservation_fill", 13) => Ok(EvidenceRecordIdentity::SubmitReservationFillV13),
        ("submit_reservation_fill", 14) => Ok(EvidenceRecordIdentity::SubmitReservationFillV14),
        ("submit_reservation_fill", 15) => Ok(EvidenceRecordIdentity::SubmitReservationFillV15),
        ("entry_skip", 14) => Ok(EvidenceRecordIdentity::EntrySkipLegacyV14),
        ("entry_skip", 15) => Ok(EvidenceRecordIdentity::EntrySkipLegacyV15),
        ("entry_skip_complete_reason", 15) => Ok(EvidenceRecordIdentity::EntrySkipCompleteReasonV15),
        ("exit_decision", 14) => Ok(EvidenceRecordIdentity::ExitDecisionV14),
        ("exit_decision", 15) => Ok(EvidenceRecordIdentity::ExitDecisionV15),
        ("exit_evaluation", 14) => Ok(EvidenceRecordIdentity::ExitEvaluationV14),
        ("exit_evaluation", 15) => Ok(EvidenceRecordIdentity::ExitEvaluationV15),
        ("loss_governor_halt", 14) => Ok(EvidenceRecordIdentity::LossGovernorHaltV14),
        ("loss_governor_halt", 15) => Ok(EvidenceRecordIdentity::LossGovernorHaltV15),
        ("order_reject", 14) => Ok(EvidenceRecordIdentity::OrderRejectV14),
        ("order_reject", 15) => Ok(EvidenceRecordIdentity::OrderRejectV15),
        ("settlement", 14) => Ok(EvidenceRecordIdentity::SettlementV14),
        ("settlement", 15) => Ok(EvidenceRecordIdentity::SettlementV15),
        ("settlement_booking_error", 14) => Ok(EvidenceRecordIdentity::SettlementBookingErrorV14),
        ("settlement_booking_error", 15) => Ok(EvidenceRecordIdentity::SettlementBookingErrorV15),
        ("terminal_settlement", 14) => Ok(EvidenceRecordIdentity::TerminalSettlementV14),
        ("terminal_settlement", 15) => Ok(EvidenceRecordIdentity::TerminalSettlementV15),
        ("order_lifecycle", 14) => Ok(EvidenceRecordIdentity::OrderLifecycleV14),
        ("order_lifecycle", 15) => Ok(EvidenceRecordIdentity::OrderLifecycleV15),
        ("venue_truth_capture_failure", 14) => Ok(EvidenceRecordIdentity::VenueTruthCaptureFailureV14),
        ("venue_truth_capture_failure", 15) => Ok(EvidenceRecordIdentity::VenueTruthCaptureFailureV15),
        ("venue_truth_divergence", 14) => Ok(EvidenceRecordIdentity::VenueTruthDivergenceV14),
        ("venue_truth_divergence", 15) => Ok(EvidenceRecordIdentity::VenueTruthDivergenceV15),
        ("requote_throttle", 14) => Ok(EvidenceRecordIdentity::RequoteThrottleV14),
        ("requote_throttle", 15) => Ok(EvidenceRecordIdentity::RequoteThrottleV15),
        ("admission_decision", 13) => Ok(EvidenceRecordIdentity::AdmissionDecisionLegacyV13),
        ("position_sizer_rebuild", 13) => Ok(EvidenceRecordIdentity::PositionSizerRebuildLegacyV13),
        _ => Err(anyhow!("unregistered decision-evidence identity kind={kind:?} schema_version={schema_version}")),
    }
}

impl EvidenceRecordIdentity {
    #[must_use]
    pub const fn metadata(self) -> EvidenceIdentityMetadata {
        match self {
            Self::StrategyInputSnapshotV14 => EvidenceIdentityMetadata { kind: "strategy_input_snapshot", schema_version: 14, gate_id: "bolt_v3.strategy_input_snapshot", decode_action: EvidenceDecodeAction::StrategyInputSnapshot },
            Self::StrategyInputSnapshotV15 => EvidenceIdentityMetadata { kind: "strategy_input_snapshot", schema_version: 15, gate_id: "bolt_v3.strategy_input_snapshot", decode_action: EvidenceDecodeAction::StrategyInputSnapshot },
            Self::OrderIntentV14 => EvidenceIdentityMetadata { kind: "order_intent", schema_version: 14, gate_id: "bolt_v3.order_intent", decode_action: EvidenceDecodeAction::OrderIntent },
            Self::OrderIntentV15 => EvidenceIdentityMetadata { kind: "order_intent", schema_version: 15, gate_id: "bolt_v3.order_intent", decode_action: EvidenceDecodeAction::OrderIntent },
            Self::AdmissionDecisionV14 => EvidenceIdentityMetadata { kind: "admission_decision", schema_version: 14, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::AdmissionDecision },
            Self::AdmissionDecisionV15 => EvidenceIdentityMetadata { kind: "admission_decision", schema_version: 15, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::AdmissionDecision },
            Self::BasketAdmissionDecisionV14 => EvidenceIdentityMetadata { kind: "basket_admission_decision", schema_version: 14, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::BasketAdmissionDecision },
            Self::BasketAdmissionDecisionV15 => EvidenceIdentityMetadata { kind: "basket_admission_decision", schema_version: 15, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::BasketAdmissionDecision },
            Self::CapitalAdmissionRebuildV14 => EvidenceIdentityMetadata { kind: "capital_admission_rebuild", schema_version: 14, gate_id: "bolt_v3.capital_admission_rebuild", decode_action: EvidenceDecodeAction::CapitalAdmissionRebuild },
            Self::CapitalAdmissionRebuildV15 => EvidenceIdentityMetadata { kind: "capital_admission_rebuild", schema_version: 15, gate_id: "bolt_v3.capital_admission_rebuild", decode_action: EvidenceDecodeAction::CapitalAdmissionRebuild },
            Self::SubmitReservationMetadataV13 => EvidenceIdentityMetadata { kind: "submit_reservation_metadata", schema_version: 13, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationMetadata },
            Self::SubmitReservationMetadataV14 => EvidenceIdentityMetadata { kind: "submit_reservation_metadata", schema_version: 14, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationMetadata },
            Self::SubmitReservationMetadataV15 => EvidenceIdentityMetadata { kind: "submit_reservation_metadata", schema_version: 15, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationMetadata },
            Self::SubmitReservationFillV13 => EvidenceIdentityMetadata { kind: "submit_reservation_fill", schema_version: 13, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationFill },
            Self::SubmitReservationFillV14 => EvidenceIdentityMetadata { kind: "submit_reservation_fill", schema_version: 14, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationFill },
            Self::SubmitReservationFillV15 => EvidenceIdentityMetadata { kind: "submit_reservation_fill", schema_version: 15, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::SubmitReservationFill },
            Self::EntrySkipLegacyV14 => EvidenceIdentityMetadata { kind: "entry_skip", schema_version: 14, gate_id: "bolt_v3.entry_skip", decode_action: EvidenceDecodeAction::EntrySkipV15 },
            Self::EntrySkipLegacyV15 => EvidenceIdentityMetadata { kind: "entry_skip", schema_version: 15, gate_id: "bolt_v3.entry_skip", decode_action: EvidenceDecodeAction::EntrySkipV15 },
            Self::EntrySkipCompleteReasonV15 => EvidenceIdentityMetadata { kind: "entry_skip_complete_reason", schema_version: 15, gate_id: "bolt_v3.entry_skip", decode_action: EvidenceDecodeAction::EntrySkipCompleteReason },
            Self::ExitDecisionV14 => EvidenceIdentityMetadata { kind: "exit_decision", schema_version: 14, gate_id: "bolt_v3.exit_decision", decode_action: EvidenceDecodeAction::ExitDecision },
            Self::ExitDecisionV15 => EvidenceIdentityMetadata { kind: "exit_decision", schema_version: 15, gate_id: "bolt_v3.exit_decision", decode_action: EvidenceDecodeAction::ExitDecision },
            Self::ExitEvaluationV14 => EvidenceIdentityMetadata { kind: "exit_evaluation", schema_version: 14, gate_id: "bolt_v3.exit_evaluation", decode_action: EvidenceDecodeAction::ExitEvaluation },
            Self::ExitEvaluationV15 => EvidenceIdentityMetadata { kind: "exit_evaluation", schema_version: 15, gate_id: "bolt_v3.exit_evaluation", decode_action: EvidenceDecodeAction::ExitEvaluation },
            Self::LossGovernorHaltV14 => EvidenceIdentityMetadata { kind: "loss_governor_halt", schema_version: 14, gate_id: "bolt_v3.loss_governor_halt", decode_action: EvidenceDecodeAction::LossGovernorHalt },
            Self::LossGovernorHaltV15 => EvidenceIdentityMetadata { kind: "loss_governor_halt", schema_version: 15, gate_id: "bolt_v3.loss_governor_halt", decode_action: EvidenceDecodeAction::LossGovernorHalt },
            Self::OrderRejectV14 => EvidenceIdentityMetadata { kind: "order_reject", schema_version: 14, gate_id: "bolt_v3.order_reject", decode_action: EvidenceDecodeAction::OrderReject },
            Self::OrderRejectV15 => EvidenceIdentityMetadata { kind: "order_reject", schema_version: 15, gate_id: "bolt_v3.order_reject", decode_action: EvidenceDecodeAction::OrderReject },
            Self::SettlementV14 => EvidenceIdentityMetadata { kind: "settlement", schema_version: 14, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::Settlement },
            Self::SettlementV15 => EvidenceIdentityMetadata { kind: "settlement", schema_version: 15, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::Settlement },
            Self::SettlementBookingErrorV14 => EvidenceIdentityMetadata { kind: "settlement_booking_error", schema_version: 14, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::SettlementBookingError },
            Self::SettlementBookingErrorV15 => EvidenceIdentityMetadata { kind: "settlement_booking_error", schema_version: 15, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::SettlementBookingError },
            Self::TerminalSettlementV14 => EvidenceIdentityMetadata { kind: "terminal_settlement", schema_version: 14, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::TerminalSettlement },
            Self::TerminalSettlementV15 => EvidenceIdentityMetadata { kind: "terminal_settlement", schema_version: 15, gate_id: "bolt_v3.settlement", decode_action: EvidenceDecodeAction::TerminalSettlement },
            Self::OrderLifecycleV14 => EvidenceIdentityMetadata { kind: "order_lifecycle", schema_version: 14, gate_id: "bolt_v3.order_lifecycle", decode_action: EvidenceDecodeAction::OrderLifecycle },
            Self::OrderLifecycleV15 => EvidenceIdentityMetadata { kind: "order_lifecycle", schema_version: 15, gate_id: "bolt_v3.order_lifecycle", decode_action: EvidenceDecodeAction::OrderLifecycle },
            Self::VenueTruthCaptureFailureV14 => EvidenceIdentityMetadata { kind: "venue_truth_capture_failure", schema_version: 14, gate_id: "bolt_v3.venue_truth_capture_failure", decode_action: EvidenceDecodeAction::VenueTruthCaptureFailure },
            Self::VenueTruthCaptureFailureV15 => EvidenceIdentityMetadata { kind: "venue_truth_capture_failure", schema_version: 15, gate_id: "bolt_v3.venue_truth_capture_failure", decode_action: EvidenceDecodeAction::VenueTruthCaptureFailure },
            Self::VenueTruthDivergenceV14 => EvidenceIdentityMetadata { kind: "venue_truth_divergence", schema_version: 14, gate_id: "bolt_v3.venue_truth_divergence", decode_action: EvidenceDecodeAction::VenueTruthDivergence },
            Self::VenueTruthDivergenceV15 => EvidenceIdentityMetadata { kind: "venue_truth_divergence", schema_version: 15, gate_id: "bolt_v3.venue_truth_divergence", decode_action: EvidenceDecodeAction::VenueTruthDivergence },
            Self::RequoteThrottleV14 => EvidenceIdentityMetadata { kind: "requote_throttle", schema_version: 14, gate_id: "bolt_v3.requote_throttle", decode_action: EvidenceDecodeAction::RequoteThrottle },
            Self::RequoteThrottleV15 => EvidenceIdentityMetadata { kind: "requote_throttle", schema_version: 15, gate_id: "bolt_v3.requote_throttle", decode_action: EvidenceDecodeAction::RequoteThrottle },
            Self::AdmissionDecisionLegacyV13 => EvidenceIdentityMetadata { kind: "admission_decision", schema_version: 13, gate_id: "bolt_v3.submit_admission", decode_action: EvidenceDecodeAction::AdmissionDecision },
            Self::PositionSizerRebuildLegacyV13 => EvidenceIdentityMetadata { kind: "position_sizer_rebuild", schema_version: 13, gate_id: "bolt_v3.position_sizer_rebuild", decode_action: EvidenceDecodeAction::CapitalAdmissionRebuild },
        }
    }

    #[must_use]
    pub const fn decode_action_for(self, consumer: EvidenceConsumer) -> Option<EvidenceDecodeAction> {
        match (self, consumer) {
            (Self::StrategyInputSnapshotV14, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::StrategyInputSnapshot),
            (Self::StrategyInputSnapshotV14, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::StrategyInputSnapshot),
            (Self::StrategyInputSnapshotV15, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::StrategyInputSnapshot),
            (Self::StrategyInputSnapshotV15, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::StrategyInputSnapshot),
            (Self::OrderIntentV14, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::OrderIntent),
            (Self::OrderIntentV14, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::OrderIntent),
            (Self::OrderIntentV15, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::OrderIntent),
            (Self::OrderIntentV15, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::OrderIntent),
            (Self::AdmissionDecisionV14, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::AdmissionDecision),
            (Self::AdmissionDecisionV14, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::AdmissionDecision),
            (Self::AdmissionDecisionV15, EvidenceConsumer::EntryDecisionChain) => Some(EvidenceDecodeAction::AdmissionDecision),
            (Self::AdmissionDecisionV15, EvidenceConsumer::ShadowPnl) => Some(EvidenceDecodeAction::AdmissionDecision),
            (Self::SubmitReservationMetadataV13, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationMetadata),
            (Self::SubmitReservationMetadataV14, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationMetadata),
            (Self::SubmitReservationMetadataV15, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationMetadata),
            (Self::SubmitReservationFillV13, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationFill),
            (Self::SubmitReservationFillV14, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationFill),
            (Self::SubmitReservationFillV15, EvidenceConsumer::SubmitReservation) => Some(EvidenceDecodeAction::SubmitReservationFill),
            (Self::ExitEvaluationV14, EvidenceConsumer::ExitEvaluation) => Some(EvidenceDecodeAction::ExitEvaluation),
            (Self::ExitEvaluationV15, EvidenceConsumer::ExitEvaluation) => Some(EvidenceDecodeAction::ExitEvaluation),
            (Self::LossGovernorHaltV14, EvidenceConsumer::LossGovernorHalt) => Some(EvidenceDecodeAction::LossGovernorHalt),
            (Self::LossGovernorHaltV15, EvidenceConsumer::LossGovernorHalt) => Some(EvidenceDecodeAction::LossGovernorHalt),
            (Self::OrderRejectV14, EvidenceConsumer::OrderReject) => Some(EvidenceDecodeAction::OrderReject),
            (Self::OrderRejectV15, EvidenceConsumer::OrderReject) => Some(EvidenceDecodeAction::OrderReject),
            (Self::SettlementV14, EvidenceConsumer::Settlement) => Some(EvidenceDecodeAction::Settlement),
            (Self::SettlementV15, EvidenceConsumer::Settlement) => Some(EvidenceDecodeAction::Settlement),
            (Self::SettlementBookingErrorV14, EvidenceConsumer::SettlementBookingError) => Some(EvidenceDecodeAction::SettlementBookingError),
            (Self::SettlementBookingErrorV15, EvidenceConsumer::SettlementBookingError) => Some(EvidenceDecodeAction::SettlementBookingError),
            (Self::TerminalSettlementV14, EvidenceConsumer::TerminalSettlement) => Some(EvidenceDecodeAction::TerminalSettlement),
            (Self::TerminalSettlementV14, EvidenceConsumer::SettlementBookingError) => Some(EvidenceDecodeAction::TerminalSettlement),
            (Self::TerminalSettlementV15, EvidenceConsumer::TerminalSettlement) => Some(EvidenceDecodeAction::TerminalSettlement),
            (Self::TerminalSettlementV15, EvidenceConsumer::SettlementBookingError) => Some(EvidenceDecodeAction::TerminalSettlement),
            _ => None,
        }
    }

    #[must_use]
    pub const fn current_strategy_input_snapshot() -> Self {
        Self::StrategyInputSnapshotV15
    }

    #[must_use]
    pub const fn current_order_intent() -> Self {
        Self::OrderIntentV15
    }

    #[must_use]
    pub const fn current_admission_decision() -> Self {
        Self::AdmissionDecisionV15
    }

    #[must_use]
    pub const fn current_basket_admission_decision() -> Self {
        Self::BasketAdmissionDecisionV15
    }

    #[must_use]
    pub const fn current_capital_admission_rebuild() -> Self {
        Self::CapitalAdmissionRebuildV15
    }

    #[must_use]
    pub const fn current_submit_reservation_metadata() -> Self {
        Self::SubmitReservationMetadataV15
    }

    #[must_use]
    pub const fn current_submit_reservation_fill() -> Self {
        Self::SubmitReservationFillV15
    }

    #[must_use]
    pub const fn current_entry_skip() -> Self {
        Self::EntrySkipCompleteReasonV15
    }

    #[must_use]
    pub const fn current_exit_decision() -> Self {
        Self::ExitDecisionV15
    }

    #[must_use]
    pub const fn current_exit_evaluation() -> Self {
        Self::ExitEvaluationV15
    }

    #[must_use]
    pub const fn current_loss_governor_halt() -> Self {
        Self::LossGovernorHaltV15
    }

    #[must_use]
    pub const fn current_order_reject() -> Self {
        Self::OrderRejectV15
    }

    #[must_use]
    pub const fn current_settlement() -> Self {
        Self::SettlementV15
    }

    #[must_use]
    pub const fn current_settlement_booking_error() -> Self {
        Self::SettlementBookingErrorV15
    }

    #[must_use]
    pub const fn current_terminal_settlement() -> Self {
        Self::TerminalSettlementV15
    }

    #[must_use]
    pub const fn current_order_lifecycle() -> Self {
        Self::OrderLifecycleV15
    }

    #[must_use]
    pub const fn current_venue_truth_capture_failure() -> Self {
        Self::VenueTruthCaptureFailureV15
    }

    #[must_use]
    pub const fn current_venue_truth_divergence() -> Self {
        Self::VenueTruthDivergenceV15
    }

    #[must_use]
    pub const fn current_requote_throttle() -> Self {
        Self::RequoteThrottleV15
    }
}
