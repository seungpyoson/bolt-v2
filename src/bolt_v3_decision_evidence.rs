use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, anyhow};
use nautilus_model::orders::{Order, OrderAny};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
use crate::bolt_v3_config::LoadedBoltV3Config;
use crate::bolt_v3_numeric::Probability;
use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;
use crate::bolt_v3_realized_volatility::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
    RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceDiagnostic,
    RealizedVolSourceRejectReason, RealizedVolSourceStatus,
};
use crate::bolt_v3_timestamp_domain::LocalReceiveMs;
use crate::bolt_v3_venue_truth::{VenueTruthCaptureFailureEvidence, VenueTruthDivergenceEvidence};

pub mod contract_generator;
#[doc(hidden)]
pub mod current;
#[doc(hidden)]
pub mod decode;
#[doc(hidden)]
pub mod facts;
pub(crate) mod generated_contract;
#[doc(hidden)]
pub mod sink;
mod startup;
pub use startup::{CurrentMachineStreamPreflight, preflight_current_machine_stream};

fn serialize_optional_local_receive_ms<S>(
    value: &Option<LocalReceiveMs>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value.map(LocalReceiveMs::value).serialize(serializer)
}

pub const BOLT_V3_DECISION_EVIDENCE_GATE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOLT_V3_LOSS_GOVERNOR_HALT_SUBSYSTEM: &str = "loss_governor";
const SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY: &str = "prediction_market_binary";
const SUBMIT_RESERVATION_METADATA_SIDE_BUY: &str = "buy";
const SUBMIT_RESERVATION_METADATA_SIDE_SELL: &str = "sell";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT: &str = "next";
/// Single source of truth for the upper bound on retained reject-episode maps. Both
/// the submit-admission reject-episode map and the venue/NT order-reject observer
/// feed key by high-cardinality identifiers (instrument ids churn, e.g. Polymarket
/// token ids), so without a cap either map grows unbounded over a long-running node.
/// Eviction is oldest-first and only resets an evidence-sampling counter; it never
/// touches any trading decision. Set generously so eviction is rare in practice.
pub(crate) const BOLT_V3_REJECT_EVIDENCE_MAX_EPISODES: usize = 4096;

/// Implemented by reject-episode value types so the shared bounded-map helper can
/// rank episodes by age (oldest-first) without knowing the concrete struct.
pub(crate) trait EpisodeFirstNs {
    /// Nanosecond timestamp of the first observation in this episode. Smaller is
    /// older, and the oldest episode is evicted first when the map exceeds its cap.
    fn first_ns(&self) -> u64;
}

/// While `map` exceeds `cap`, drop the entry with the smallest `first_ns` (oldest
/// episode). A single linear scan per eviction is adequate at this cap and avoids
/// pulling in an LRU dependency. Shared by every reject-episode map so the eviction
/// semantics live in exactly one place. Eviction only discards an evidence-sampling
/// counter; a later reject for the same key simply re-starts its episode.
pub(crate) fn evict_oldest_episodes_over_cap<V: EpisodeFirstNs>(
    map: &mut BTreeMap<String, V>,
    cap: usize,
) {
    while map.len() > cap {
        let oldest_key = map
            .iter()
            .min_by_key(|(_, episode)| episode.first_ns())
            .map(|(key, _)| key.clone());
        match oldest_key {
            Some(key) => {
                map.remove(&key);
            }
            None => break,
        }
    }
}

#[derive(Debug, Clone)]
pub enum BoltV3DecisionEvidenceCommand {
    BlockedStrategyInputObservation(BoltV3StrategyInputEvidenceSnapshot),
    SubmitLinkedStrategyInputSnapshot(BoltV3StrategyInputEvidenceSnapshot),
    EntryOrderIntent(BoltV3OrderIntentEvidence),
    RiskReducingExitOrderIntent(BoltV3OrderIntentEvidence),
    AdmittedEntryAdmission(BoltV3AdmissionDecisionEvidence),
    RejectedEntryAdmission(BoltV3AdmissionDecisionEvidence),
    RiskReducingExitAdmission(BoltV3AdmissionDecisionEvidence),
    ForcedReductionAdmission(BoltV3AdmissionDecisionEvidence),
    BasketAdmissionGranted(BoltV3BasketAdmissionDecisionEvidence),
    BasketAdmissionRejected(BoltV3BasketAdmissionDecisionEvidence),
    CapitalAdmissionRebuild(BoltV3CapitalAdmissionRebuildAuditEvidence),
    SubmitReservationMetadata(BoltV3SubmitReservationMetadataEvidence),
    SubmitReservationFill(BoltV3SubmitReservationFillEvidence),
    EntrySkipObservation(BoltV3EntrySkipEvidence),
    ExitSubmissionDecision(BoltV3ExitDecisionEvidence),
    ExitHoldDecision(BoltV3ExitDecisionEvidence),
    ExitEvaluation(BoltV3ExitEvaluationEvidence),
    LossGovernorHalt(BoltV3LossGovernorHaltEvidence),
    OrderReject(BoltV3OrderRejectEvidence),
    OrderLifecycle(BoltV3OrderLifecycleEvidence),
    RequoteThrottle(BoltV3RequoteThrottleEvidence),
    Settlement(BoltV3SettlementEvidence),
    SettlementBookingError(BoltV3SettlementBookingErrorEvidence),
    TerminalSettlement(BoltV3TerminalSettlementEvidence),
    VenueTruthCaptureFailure(VenueTruthCaptureFailureEvidence),
    VenueTruthDivergence(VenueTruthDivergenceEvidence),
}

impl BoltV3DecisionEvidenceCommand {
    fn effect_policy(&self) -> generated_contract::EffectPolicy {
        use generated_contract::KnownPurpose as Purpose;

        let purpose = match self {
            Self::BlockedStrategyInputObservation(_) => Purpose::BlockedStrategyInputObservation,
            Self::SubmitLinkedStrategyInputSnapshot(_) => {
                Purpose::SubmitLinkedStrategyInputSnapshot
            }
            Self::EntryOrderIntent(_) => Purpose::EntryOrderIntent,
            Self::RiskReducingExitOrderIntent(_) => Purpose::RiskReducingExitOrderIntent,
            Self::AdmittedEntryAdmission(_) => Purpose::AdmittedEntryAdmission,
            Self::RejectedEntryAdmission(_) => Purpose::RejectedEntryAdmission,
            Self::RiskReducingExitAdmission(_) => Purpose::RiskReducingExitAdmission,
            Self::ForcedReductionAdmission(_) => Purpose::ForcedReductionAdmission,
            Self::BasketAdmissionGranted(_) => Purpose::BasketAdmissionGranted,
            Self::BasketAdmissionRejected(_) => Purpose::BasketAdmissionRejected,
            Self::CapitalAdmissionRebuild(_) => Purpose::CapitalAdmissionRebuild,
            Self::SubmitReservationMetadata(_) => Purpose::SubmitReservationMetadata,
            Self::SubmitReservationFill(_) => Purpose::SubmitReservationFill,
            Self::EntrySkipObservation(_) => Purpose::EntrySkipObservation,
            Self::ExitSubmissionDecision(_) => Purpose::ExitSubmissionDecision,
            Self::ExitHoldDecision(_) => Purpose::ExitHoldDecision,
            Self::ExitEvaluation(_) => Purpose::ExitEvaluation,
            Self::LossGovernorHalt(_) => Purpose::LossGovernorHalt,
            Self::OrderReject(_) => Purpose::OrderReject,
            Self::OrderLifecycle(_) => Purpose::OrderLifecycle,
            Self::RequoteThrottle(_) => Purpose::RequoteThrottleObservation,
            Self::Settlement(_) => Purpose::Settlement,
            Self::SettlementBookingError(_) => Purpose::SettlementBookingError,
            Self::TerminalSettlement(_) => Purpose::TerminalSettlement,
            Self::VenueTruthCaptureFailure(_) => Purpose::VenueTruthCaptureFailure,
            Self::VenueTruthDivergence(_) => Purpose::VenueTruthDivergence,
        };
        generated_contract::effect_policy_for_purpose(purpose)
    }

    fn encode(self) -> Result<sink::EncodedEvidenceRecord> {
        match self {
            Self::BlockedStrategyInputObservation(value) => {
                current::encode_blocked_strategy_input_observation(&value)
            }
            Self::SubmitLinkedStrategyInputSnapshot(value) => {
                current::encode_submit_linked_strategy_input_snapshot(&value)
            }
            Self::EntryOrderIntent(value) => current::encode_entry_order_intent(&value),
            Self::RiskReducingExitOrderIntent(value) => {
                current::encode_risk_reducing_exit_order_intent(&value)
            }
            Self::AdmittedEntryAdmission(value) => current::encode_admitted_entry_admission(&value),
            Self::RejectedEntryAdmission(value) => current::encode_rejected_entry_admission(&value),
            Self::RiskReducingExitAdmission(value) => {
                current::encode_risk_reducing_exit_admission(&value)
            }
            Self::ForcedReductionAdmission(value) => {
                current::encode_forced_reduction_admission(&value)
            }
            Self::BasketAdmissionGranted(value) => current::encode_basket_admission_granted(&value),
            Self::BasketAdmissionRejected(value) => {
                current::encode_basket_admission_rejected(&value)
            }
            Self::CapitalAdmissionRebuild(value) => {
                current::encode_capital_admission_rebuild(&value)
            }
            Self::SubmitReservationMetadata(value) => {
                current::encode_submit_reservation_metadata(&value)
            }
            Self::SubmitReservationFill(value) => current::encode_submit_reservation_fill(&value),
            Self::EntrySkipObservation(value) => current::encode_entry_skip_observation(&value),
            Self::ExitSubmissionDecision(value) => current::encode_exit_submission_decision(&value),
            Self::ExitHoldDecision(value) => current::encode_exit_hold_decision(&value),
            Self::ExitEvaluation(value) => current::encode_exit_evaluation(&value),
            Self::LossGovernorHalt(value) => current::encode_loss_governor_halt(&value),
            Self::OrderReject(value) => current::encode_order_reject(&value),
            Self::OrderLifecycle(value) => current::encode_order_lifecycle(&value),
            Self::RequoteThrottle(value) => current::encode_requote_throttle(&value),
            Self::Settlement(value) => current::encode_settlement(&value),
            Self::SettlementBookingError(value) => current::encode_settlement_booking_error(&value),
            Self::TerminalSettlement(value) => current::encode_terminal_settlement(&value),
            Self::VenueTruthCaptureFailure(value) => {
                current::encode_venue_truth_capture_failure(&value)
            }
            Self::VenueTruthDivergence(value) => current::encode_venue_truth_divergence(&value),
        }
    }
}

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn try_record_command(&self, command: BoltV3DecisionEvidenceCommand) -> Result<()>;

    fn drain_shutdown(&self) -> Result<()>;
}

pub trait BoltV3DecisionEvidenceWriterExt: BoltV3DecisionEvidenceWriter {
    fn record_current(&self, command: BoltV3DecisionEvidenceCommand) -> Result<()> {
        let policy = command.effect_policy();
        if let Err(error) = self.try_record_command(command) {
            if matches!(
                policy,
                generated_contract::EffectPolicy::MustPrecedeNewRisk
                    | generated_contract::EffectPolicy::ReconciliationFailClosed
            ) {
                return Err(error);
            }
            log::error!(
                "decision-evidence write failed under non-blocking effect policy \
                 {policy:?}; preserving the caller result: error={error:#}"
            );
        }
        Ok(())
    }

    fn record_blocked_strategy_input_observation(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        self.record_current(
            BoltV3DecisionEvidenceCommand::BlockedStrategyInputObservation(snapshot.clone()),
        )
    }

    fn record_submit_linked_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        self.record_current(
            BoltV3DecisionEvidenceCommand::SubmitLinkedStrategyInputSnapshot(snapshot.clone()),
        )
    }

    fn record_entry_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::EntryOrderIntent(
            intent.clone(),
        ))
    }

    fn record_risk_reducing_exit_order_intent(
        &self,
        intent: &BoltV3OrderIntentEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::RiskReducingExitOrderIntent(
            intent.clone(),
        ))
    }

    fn record_admitted_entry_admission(
        &self,
        decision: &BoltV3AdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::AdmittedEntryAdmission(
            decision.clone(),
        ))
    }

    fn record_rejected_entry_admission(
        &self,
        decision: &BoltV3AdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::RejectedEntryAdmission(
            decision.clone(),
        ))
    }

    fn record_risk_reducing_exit_admission(
        &self,
        decision: &BoltV3AdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::RiskReducingExitAdmission(
            decision.clone(),
        ))
    }

    fn record_forced_reduction_admission(
        &self,
        decision: &BoltV3AdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::ForcedReductionAdmission(
            decision.clone(),
        ))
    }

    fn record_basket_admission_granted(
        &self,
        decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::BasketAdmissionGranted(
            decision.clone(),
        ))
    }

    fn record_basket_admission_rejected(
        &self,
        decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::BasketAdmissionRejected(
            decision.clone(),
        ))
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::CapitalAdmissionRebuild(
            audit.clone(),
        ))
    }

    fn record_submit_reservation_metadata(
        &self,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::SubmitReservationMetadata(
            metadata.clone(),
        ))
    }

    fn record_submit_reservation_fill(
        &self,
        fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::SubmitReservationFill(
            fill.clone(),
        ))
    }

    fn record_entry_skip(&self, skip: &BoltV3EntrySkipEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::EntrySkipObservation(
            skip.clone(),
        ))
    }

    fn record_exit_submission_decision(&self, decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::ExitSubmissionDecision(
            decision.clone(),
        ))
    }

    fn record_exit_hold_decision(&self, decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::ExitHoldDecision(
            decision.clone(),
        ))
    }

    fn record_exit_evaluation(&self, evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::ExitEvaluation(
            evidence.clone(),
        ))
    }

    fn record_loss_governor_halt(&self, evidence: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::LossGovernorHalt(
            evidence.clone(),
        ))
    }

    fn record_order_reject(&self, evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::OrderReject(evidence.clone()))
    }

    fn record_order_lifecycle(&self, evidence: &BoltV3OrderLifecycleEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::OrderLifecycle(
            evidence.clone(),
        ))
    }

    fn record_requote_throttle(&self, throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::RequoteThrottle(
            throttle.clone(),
        ))
    }

    fn record_settlement(&self, evidence: &BoltV3SettlementEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::Settlement(evidence.clone()))
    }

    fn record_settlement_booking_error(
        &self,
        evidence: &BoltV3SettlementBookingErrorEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::SettlementBookingError(
            evidence.clone(),
        ))
    }

    fn record_terminal_settlement(
        &self,
        evidence: &BoltV3TerminalSettlementEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::TerminalSettlement(
            evidence.clone(),
        ))
    }

    fn record_venue_truth_capture_failure(
        &self,
        evidence: &VenueTruthCaptureFailureEvidence,
    ) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::VenueTruthCaptureFailure(
            evidence.clone(),
        ))
    }

    fn record_venue_truth_divergence(&self, evidence: &VenueTruthDivergenceEvidence) -> Result<()> {
        self.record_current(BoltV3DecisionEvidenceCommand::VenueTruthDivergence(
            evidence.clone(),
        ))
    }
}

impl<T: BoltV3DecisionEvidenceWriter + ?Sized> BoltV3DecisionEvidenceWriterExt for T {}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct NoStrategyDecisionEvidenceWriter;

#[cfg(test)]
impl BoltV3DecisionEvidenceWriter for NoStrategyDecisionEvidenceWriter {
    fn try_record_command(&self, _command: BoltV3DecisionEvidenceCommand) -> Result<()> {
        Err(anyhow!("decision-evidence writer is not configured"))
    }

    fn drain_shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3SubmitIntentKind {
    Entry,
    RiskReducingExit,
    ReplaceSubmit,
    KillSwitchForcedReduction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentClampNotEvaluatedReason {
    NoVenueTruth,
    ForeignInstrument,
    NonSellOrderSide,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub enum BoltV3OrderIntentClampOutcome {
    WithinBounds,
    Clamped {
        original_quantity: String,
    },
    Rejected,
    NotEvaluated {
        reason: BoltV3OrderIntentClampNotEvaluatedReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
    pub clamp_outcome: Option<BoltV3OrderIntentClampOutcome>,
    pub order_fields: BoltV3OrderIntentOrderFields,
}

pub(crate) fn compiled_order_price_source(fallback_price: String, order: &OrderAny) -> String {
    selected_compiled_order_price_source(
        order.price().map(|price| price.to_string()),
        order.trigger_price().map(|price| price.to_string()),
        order.activation_price().map(|price| price.to_string()),
        fallback_price,
    )
}

fn selected_compiled_order_price_source(
    price: Option<String>,
    trigger_price: Option<String>,
    activation_price: Option<String>,
    fallback_price: String,
) -> String {
    price
        .or(trigger_price)
        .or(activation_price)
        .unwrap_or(fallback_price)
}

impl BoltV3OrderIntentEvidence {
    pub fn from_compiled_order(
        strategy_id: String,
        intent_kind: BoltV3OrderIntentKind,
        fallback_price: String,
        order: &OrderAny,
    ) -> Self {
        Self {
            strategy_id,
            intent_kind,
            instrument_id: order.instrument_id().to_string(),
            client_order_id: order.client_order_id().to_string(),
            order_side: order.order_side().to_string(),
            price: compiled_order_price_source(fallback_price, order),
            quantity: order.quantity().to_string(),
            clamp_outcome: None,
            order_fields: BoltV3OrderIntentOrderFields::from_order(order),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderIntentOrderFields {
    pub order_type: String,
    pub time_in_force: String,
    pub price: Option<String>,
    pub trigger_price: Option<String>,
    pub activation_price: Option<String>,
    pub trigger_type: Option<String>,
    pub trigger_instrument_id: Option<String>,
    pub trailing_offset: Option<String>,
    pub trailing_offset_type: Option<String>,
    pub expire_time_unix_nanos: Option<String>,
    pub is_post_only: bool,
    pub is_reduce_only: bool,
    pub is_quote_quantity: bool,
}

impl BoltV3OrderIntentOrderFields {
    pub fn from_order(order: &OrderAny) -> Self {
        Self {
            order_type: order.order_type().to_string(),
            time_in_force: order.time_in_force().to_string(),
            price: order.price().map(|price| price.to_string()),
            trigger_price: order.trigger_price().map(|price| price.to_string()),
            activation_price: order.activation_price().map(|price| price.to_string()),
            trigger_type: order
                .trigger_type()
                .map(|trigger_type| trigger_type.to_string()),
            trigger_instrument_id: order
                .trigger_instrument_id()
                .map(|trigger_instrument_id| trigger_instrument_id.to_string()),
            trailing_offset: order
                .trailing_offset()
                .map(|trailing_offset| trailing_offset.to_string()),
            trailing_offset_type: order
                .trailing_offset_type()
                .map(|trailing_offset_type| trailing_offset_type.to_string()),
            expire_time_unix_nanos: order
                .expire_time()
                .map(|expire_time| expire_time.as_u64().to_string()),
            is_post_only: order.is_post_only(),
            is_reduce_only: order.is_reduce_only(),
            is_quote_quantity: order.is_quote_quantity(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3RealizedVolatilitySourceDiagnosticEvidence {
    pub source_id: String,
    pub source_class: String,
    pub sample_kind: String,
    pub enabled: bool,
    pub counts_toward_quorum: bool,
    pub status: String,
    pub annualized_realized_volatility_decimal: Option<String>,
    pub measured_annualized_realized_volatility_decimal: Option<String>,
    pub noise_robust_annualized_realized_volatility_decimal: Option<String>,
    pub continuous_annualized_realized_volatility_decimal: Option<String>,
    pub jump_annualized_realized_volatility_decimal: Option<String>,
    pub first_sample_ts_ms: Option<u64>,
    pub last_sample_ts_ms: Option<u64>,
    pub raw_sample_count: usize,
    pub grid_sample_count: usize,
    pub coverage_ratio: String,
    pub max_inter_sample_gap_ms: Option<u64>,
    pub last_rejected_reason: Option<String>,
    pub last_rejected_event_ts_ms: Option<u64>,
    pub last_rejected_recv_ts_ms: Option<u64>,
    pub rejection_counters: BTreeMap<String, u64>,
    pub block_reason: Option<String>,
}

impl BoltV3RealizedVolatilitySourceDiagnosticEvidence {
    pub fn from_realized_vol_diagnostic(diagnostic: &RealizedVolSourceDiagnostic) -> Self {
        Self {
            source_id: diagnostic.source_id.clone(),
            source_class: realized_volatility_source_class_evidence_label(diagnostic.source_class)
                .to_string(),
            sample_kind: realized_volatility_sample_kind_evidence_label(diagnostic.sample_kind)
                .to_string(),
            enabled: diagnostic.enabled,
            counts_toward_quorum: diagnostic.counts_toward_quorum,
            status: realized_volatility_source_status_evidence_label(diagnostic.status).to_string(),
            annualized_realized_volatility_decimal: diagnostic
                .annualized_realized_vol_decimal
                .map(number_evidence),
            measured_annualized_realized_volatility_decimal: diagnostic
                .measured_annualized_realized_vol_decimal
                .map(number_evidence),
            noise_robust_annualized_realized_volatility_decimal: diagnostic
                .noise_robust_annualized_realized_vol_decimal
                .map(number_evidence),
            continuous_annualized_realized_volatility_decimal: diagnostic
                .continuous_annualized_realized_vol_decimal
                .map(number_evidence),
            jump_annualized_realized_volatility_decimal: diagnostic
                .jump_annualized_realized_vol_decimal
                .map(number_evidence),
            first_sample_ts_ms: diagnostic.first_sample_ts_ms,
            last_sample_ts_ms: diagnostic.last_sample_ts_ms,
            raw_sample_count: diagnostic.raw_sample_count,
            grid_sample_count: diagnostic.grid_sample_count,
            coverage_ratio: number_evidence(diagnostic.coverage_ratio),
            max_inter_sample_gap_ms: diagnostic.max_inter_sample_gap_ms,
            last_rejected_reason: diagnostic
                .last_rejected_reason
                .map(realized_volatility_reject_reason_evidence_label)
                .map(str::to_string),
            last_rejected_event_ts_ms: diagnostic.last_rejected_event_ts_ms,
            last_rejected_recv_ts_ms: diagnostic.last_rejected_recv_ts_ms,
            rejection_counters: diagnostic
                .rejection_counters
                .iter()
                .map(|(reason, count)| {
                    (
                        realized_volatility_reject_reason_evidence_label(*reason).to_string(),
                        *count,
                    )
                })
                .collect(),
            block_reason: diagnostic
                .block_reason
                .map(realized_volatility_block_reason_evidence_label)
                .map(str::to_string),
        }
    }
}

pub fn realized_volatility_aggregation_evidence_label(
    aggregation: RealizedVolAggregation,
) -> &'static str {
    match aggregation {
        RealizedVolAggregation::UpperQuantile { .. } => "upper_quantile",
        RealizedVolAggregation::Median => "median",
        RealizedVolAggregation::TrimmedMean { .. } => "trimmed_mean",
        RealizedVolAggregation::MedianWithUpperQuantileGuard { .. } => {
            "median_with_upper_quantile_guard"
        }
    }
}

pub fn realized_volatility_block_reason_evidence_label(
    reason: RealizedVolBlockReason,
) -> &'static str {
    match reason {
        RealizedVolBlockReason::InvalidConfig => "invalid_config",
        RealizedVolBlockReason::QuorumNotReady => "quorum_not_ready",
        RealizedVolBlockReason::SourceStale => "source_stale",
        RealizedVolBlockReason::CoverageBelowMinimum => "coverage_below_minimum",
        RealizedVolBlockReason::InterSampleGapExceeded => "inter_sample_gap_exceeded",
        RealizedVolBlockReason::SourceClassMismatch => "source_class_mismatch",
        RealizedVolBlockReason::SampleKindMismatch => "sample_kind_mismatch",
        RealizedVolBlockReason::CrossSourceDispersion => "cross_source_dispersion",
        RealizedVolBlockReason::AnnualizationBasisInvalid => "annualization_basis_invalid",
        RealizedVolBlockReason::NotWarm => "not_warm",
    }
}

/// Projects an RV-engine [`RealizedVolBlockReason`] onto the exit-evidence
/// [`BoltV3ExitRvSnapshotBlocker`]. Lives here, alongside its sibling
/// [`realized_volatility_block_reason_evidence_label`], so strategy code consumes
/// the projection without owning the realized-volatility block-reason taxonomy
/// (enforced by the strategy source fence).
pub fn realized_vol_blocker_to_exit_evidence(
    reason: RealizedVolBlockReason,
) -> BoltV3ExitRvSnapshotBlocker {
    match reason {
        RealizedVolBlockReason::InvalidConfig => BoltV3ExitRvSnapshotBlocker::InvalidConfig,
        RealizedVolBlockReason::QuorumNotReady => BoltV3ExitRvSnapshotBlocker::QuorumNotReady,
        RealizedVolBlockReason::SourceStale => BoltV3ExitRvSnapshotBlocker::SourceStale,
        RealizedVolBlockReason::CoverageBelowMinimum => {
            BoltV3ExitRvSnapshotBlocker::CoverageBelowMinimum
        }
        RealizedVolBlockReason::InterSampleGapExceeded => {
            BoltV3ExitRvSnapshotBlocker::InterSampleGapExceeded
        }
        RealizedVolBlockReason::SourceClassMismatch => {
            BoltV3ExitRvSnapshotBlocker::SourceClassMismatch
        }
        RealizedVolBlockReason::SampleKindMismatch => {
            BoltV3ExitRvSnapshotBlocker::SampleKindMismatch
        }
        RealizedVolBlockReason::CrossSourceDispersion => {
            BoltV3ExitRvSnapshotBlocker::CrossSourceDispersion
        }
        RealizedVolBlockReason::AnnualizationBasisInvalid => {
            BoltV3ExitRvSnapshotBlocker::AnnualizationBasisInvalid
        }
        RealizedVolBlockReason::NotWarm => BoltV3ExitRvSnapshotBlocker::NotWarm,
    }
}

pub fn realized_volatility_pricing_component_evidence_label(
    component: RealizedVolPricingComponent,
) -> &'static str {
    match component {
        RealizedVolPricingComponent::Measured => "measured",
        RealizedVolPricingComponent::NoiseRobust => "noise_robust",
        RealizedVolPricingComponent::Continuous => "continuous",
        RealizedVolPricingComponent::Forecast => "forecast",
    }
}

fn realized_volatility_source_class_evidence_label(
    source_class: RealizedVolSourceClass,
) -> &'static str {
    match source_class {
        RealizedVolSourceClass::SpotQuote => "spot_quote",
        RealizedVolSourceClass::Trade => "trade",
        RealizedVolSourceClass::Mark => "mark",
        RealizedVolSourceClass::Index => "index",
    }
}

fn realized_volatility_sample_kind_evidence_label(
    sample_kind: RealizedVolSampleKind,
) -> &'static str {
    match sample_kind {
        RealizedVolSampleKind::Midpoint => "midpoint",
        RealizedVolSampleKind::Trade => "trade",
        RealizedVolSampleKind::Mark => "mark",
        RealizedVolSampleKind::Index => "index",
    }
}

fn realized_volatility_source_status_evidence_label(
    status: RealizedVolSourceStatus,
) -> &'static str {
    match status {
        RealizedVolSourceStatus::Ready => "ready",
        RealizedVolSourceStatus::Blocked => "blocked",
        RealizedVolSourceStatus::DiagnosticOnly => "diagnostic_only",
        RealizedVolSourceStatus::Waiting => "waiting",
    }
}

fn realized_volatility_reject_reason_evidence_label(
    reason: RealizedVolSourceRejectReason,
) -> &'static str {
    match reason {
        RealizedVolSourceRejectReason::DisabledSource => "disabled_source",
        RealizedVolSourceRejectReason::InvalidPrice => "invalid_price",
        RealizedVolSourceRejectReason::SourceClassMismatch => "source_class_mismatch",
        RealizedVolSourceRejectReason::SampleKindMismatch => "sample_kind_mismatch",
        RealizedVolSourceRejectReason::EventTimeRegression => "event_time_regression",
        RealizedVolSourceRejectReason::DuplicateTimestamp => "duplicate_timestamp",
        RealizedVolSourceRejectReason::StaleSameEventUpdate => "stale_same_event_update",
        RealizedVolSourceRejectReason::ReceiveBeforeEvent => "receive_before_event",
        RealizedVolSourceRejectReason::EventReceiveLagExceeded => "event_receive_lag_exceeded",
    }
}

pub(crate) fn number_evidence(value: f64) -> String {
    value.to_string()
}

pub(crate) fn probability_evidence(probability: Probability) -> String {
    number_evidence(probability.value())
}

pub(crate) fn option_probability_evidence(probability: Option<Probability>) -> Option<String> {
    probability.map(probability_evidence)
}

pub(crate) fn option_number_evidence(value: Option<f64>) -> Option<String> {
    value.filter(|value| value.is_finite()).map(number_evidence)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ForcedFlatReason {
    Freeze,
    StaleReference,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExposureOccupancy {
    PendingEntry,
    EntryReconcilePending,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntryBlockReason {
    PhaseNotActive,
    MetadataMismatch,
    ActiveBookNotPriced,
    BookCrossed,
    IntervalOpenMissing,
    WarmupIncomplete,
    FeesNotReady,
    RecoveryMode,
    MarketCoolingDown,
    SpotSpikeCooldown,
    ForcedFlat(BoltV3ForcedFlatReason),
    OnePositionInvariant(BoltV3ExposureOccupancy),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3BinaryOutcomeEdgeBlockReason {
    MissingOrderBook,
    InsufficientDepth,
    InvalidProbability,
    InvalidCost,
    UnsupportedOrderShape,
    EdgeBelowThreshold,
    SpreadOrSlippageWipedEdge,
    FeeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntryPricingBlockReason {
    SpotPriceMissing,
    ReferenceCurrentPriceStale,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    UncertaintyBandUnavailable,
    FairProbabilityUnavailable,
    FeeUnavailable(BoltV3OutcomeSide),
    ExecutableEntryCostUnavailable(BoltV3OutcomeSide),
    ExecutableEdgeUnavailable(BoltV3OutcomeSide, BoltV3BinaryOutcomeEdgeBlockReason),
    SizedNotionalUnsupported(BoltV3OutcomeSide),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3EntrySkipReasonCategory {
    StrategyCoreNotRegistered,
    EntryGateBlocked,
    EntryPricingBlocked,
    NoSideSelected,
    SizedNotionalNotPositive,
    InstrumentIdMissing,
    InstrumentMissingFromCache,
    EntryPriceMissing,
    QuantityRoundingFailed,
    LimitNotionalExceedsSizedNotional,
    QuantityNotPositive,
    PositionContractInvalid,
    EntryPositionContractUnsupported,
    HistoricalEntryFeeUnavailable,
    OnePositionInvariantViolation,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3EntrySkipEvidence {
    pub strategy_id: String,
    pub now_ms: u64,
    pub reason_category: BoltV3EntrySkipReasonCategory,
    pub unclassified_context: Option<String>,
    pub gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pub pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    pub market_id: Option<String>,
    pub phase: String,
    pub seconds_to_market_end: Option<u64>,
    pub spot_price: Option<String>,
    pub reference_current_price: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price_available: bool,
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub realized_vol_gate_result: Option<BoltV3RvGateResult>,
    #[serde(serialize_with = "serialize_optional_local_receive_ms")]
    pub realized_vol_receive_watermark_ms: Option<LocalReceiveMs>,
    pub realized_vol_snapshot: Option<BoltV3EntryRealizedVolatilitySnapshotEvidence>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub selected_side: Option<BoltV3OutcomeSide>,
    pub sized_notional: Option<String>,
    pub sized_worst_case_ev_bps: Option<String>,
    pub sized_edge_cents_per_share: Option<String>,
    pub theta_scaled_min_edge_bps: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub submission_blocked_reason: Option<BoltV3EntrySkipReasonCategory>,
    pub stale_reference_after_ms: Option<u64>,
    pub last_reference_ts_ms: Option<u64>,
    pub min_liquidity_required: Option<String>,
    pub liquidity_available: Option<String>,
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub fast_venue_incoherent: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3EntrySkipEvidenceWire {
    strategy_id: String,
    now_ms: u64,
    reason_category: BoltV3EntrySkipReasonCategory,
    unclassified_context: Option<String>,
    gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    market_id: Option<String>,
    phase: String,
    seconds_to_market_end: Option<u64>,
    spot_price: Option<String>,
    reference_current_price: Option<String>,
    fast_venue_available: bool,
    reference_current_price_available: bool,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    realized_vol_gate_result: Option<BoltV3RvGateResult>,
    realized_vol_receive_watermark_ms: Option<u64>,
    realized_vol_snapshot: Option<BoltV3EntryRealizedVolatilitySnapshotEvidence>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    selected_side: Option<BoltV3OutcomeSide>,
    sized_notional: Option<String>,
    sized_worst_case_ev_bps: Option<String>,
    sized_edge_cents_per_share: Option<String>,
    theta_scaled_min_edge_bps: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    submission_blocked_reason: Option<BoltV3EntrySkipReasonCategory>,
    stale_reference_after_ms: Option<u64>,
    last_reference_ts_ms: Option<u64>,
    min_liquidity_required: Option<String>,
    liquidity_available: Option<String>,
    frozen: bool,
    metadata_matches_selection: bool,
    fast_venue_incoherent: bool,
}

impl<'de> Deserialize<'de> for BoltV3EntrySkipEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3EntrySkipEvidenceWire::deserialize(deserializer)?;
        Ok(Self {
            strategy_id: wire.strategy_id,
            now_ms: wire.now_ms,
            reason_category: wire.reason_category,
            unclassified_context: wire.unclassified_context,
            gate_blocked_by: wire.gate_blocked_by,
            pricing_blocked_by: wire.pricing_blocked_by,
            market_id: wire.market_id,
            phase: wire.phase,
            seconds_to_market_end: wire.seconds_to_market_end,
            spot_price: wire.spot_price,
            reference_current_price: wire.reference_current_price,
            fast_venue_available: wire.fast_venue_available,
            reference_current_price_available: wire.reference_current_price_available,
            realized_vol: wire.realized_vol,
            realized_vol_source_venue: wire.realized_vol_source_venue,
            realized_vol_source_ts_ms: wire.realized_vol_source_ts_ms,
            realized_vol_gate_result: wire.realized_vol_gate_result,
            realized_vol_receive_watermark_ms: wire
                .realized_vol_receive_watermark_ms
                .map(LocalReceiveMs::new),
            realized_vol_snapshot: wire.realized_vol_snapshot,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            selected_side: wire.selected_side,
            sized_notional: wire.sized_notional,
            sized_worst_case_ev_bps: wire.sized_worst_case_ev_bps,
            sized_edge_cents_per_share: wire.sized_edge_cents_per_share,
            theta_scaled_min_edge_bps: wire.theta_scaled_min_edge_bps,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            submission_blocked_reason: wire.submission_blocked_reason,
            stale_reference_after_ms: wire.stale_reference_after_ms,
            last_reference_ts_ms: wire.last_reference_ts_ms,
            min_liquidity_required: wire.min_liquidity_required,
            liquidity_available: wire.liquidity_available,
            frozen: wire.frozen,
            metadata_matches_selection: wire.metadata_matches_selection,
            fast_venue_incoherent: wire.fast_venue_incoherent,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3EntryRealizedVolatilitySnapshotEvidence {
    pub surface_id: String,
    pub as_of_ms: Option<u64>,
    pub annualized_decimal: String,
    pub measured_annualized_decimal: String,
    pub noise_robust_annualized_decimal: String,
    pub continuous_annualized_decimal: String,
    pub jump_annualized_decimal: String,
    pub forecast_annualized_decimal: String,
    pub pricing_component: String,
    pub seconds_per_annum: String,
    pub aggregation: String,
    pub sources_used: Vec<String>,
    pub source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub unknown_source_rejections: BTreeMap<String, u64>,
    pub blockers: Vec<String>,
    pub config_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitDecisionOutcome {
    Exit,
    ExitFailClosed,
    Hold,
    Blocked,
}

/// Closed taxonomy of the runtime path that triggered an exit evaluation. Used by
/// `BoltV3ExitEvaluationEvidence` so the durable record explains *which clock base*
/// produced `exit_eval_now_ms` (the 2026-06-20 incident root cause was a
/// timestamp-base mismatch between the exit clock and the realized-vol snapshot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitTriggerSource {
    SignalQuote,
    ReferenceUpdate,
    SelectionUpdate,
    BookDelta,
    Unknown,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitRvSnapshotBlocker {
    InvalidConfig,
    QuorumNotReady,
    SourceStale,
    CoverageBelowMinimum,
    InterSampleGapExceeded,
    SourceClassMismatch,
    SampleKindMismatch,
    CrossSourceDispersion,
    AnnualizationBasisInvalid,
    NotWarm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitRvGateResult {
    Accepted,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
    MissingSnapshot,
    MissingEvaluationEventTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3ExitBlockedReason {
    NoOpenPosition,
    ExitAlreadyPending,
    EntryOrderStillWorking,
    ExitDecisionUnavailable,
    ExitHold,
    PositionIntervalEnded,
    PositionIntervalUnknown,
    OpenPositionMissing,
    ExitOrderConfigInvalid,
    ExitQuoteQuantityUnsupported,
    ExitPriceMissing,
    ExitQuantityNotPositive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ExitDecisionEvidence {
    pub strategy_id: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub position_instrument_id: Option<String>,
    pub position_outcome_side: Option<BoltV3OutcomeSide>,
    pub forced_flat_reasons: Vec<BoltV3ForcedFlatReason>,
    pub spot_price: Option<String>,
    pub spot_venue_name: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub interval_open: Option<String>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub uncertainty_band_probability: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub hold_ev_bps: Option<String>,
    pub exit_ev_bps: Option<String>,
    pub realized_vol: Option<String>,
    pub realized_vol_source_venue: Option<String>,
    pub realized_vol_source_ts_ms: Option<u64>,
    pub exit_eval_now_ms: u64,
    pub exit_trigger_source: BoltV3ExitTriggerSource,
    pub trigger_ts_event_ms: u64,
    pub trigger_ts_init_ms: Option<u64>,
    pub rv_surface_id: String,
    pub rv_snapshot_as_of_ms: Option<u64>,
    pub rv_snapshot_ready: bool,
    pub rv_snapshot_has_ready_realized_vol: Option<bool>,
    pub rv_snapshot_receive_watermark_ms: Option<u64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_snapshot_blockers: Vec<BoltV3ExitRvSnapshotBlocker>,
    pub rv_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub rv_gate_result: BoltV3ExitRvGateResult,
    pub rv_future_dating_delta_ms: Option<u64>,
    pub exit_hysteresis_bps: String,
    pub exit_decision: BoltV3ExitDecisionOutcome,
    pub blocked_reason: Option<BoltV3ExitBlockedReason>,
    pub client_order_id: Option<String>,
    pub submission_order_side: Option<String>,
    pub submission_price: Option<String>,
    pub submission_quantity: Option<String>,
    pub seconds_to_market_end: Option<u64>,
    pub ts_ms: u64,
    pub stale_reference_after_ms: Option<u64>,
    pub last_reference_ts_ms: Option<u64>,
    pub min_liquidity_required: Option<String>,
    pub liquidity_available: Option<String>,
    pub frozen: bool,
    pub metadata_matches_selection: bool,
    pub fast_venue_incoherent: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3ExitDecisionEvidenceWire {
    strategy_id: String,
    market_id: Option<String>,
    position_id: Option<String>,
    position_instrument_id: Option<String>,
    position_outcome_side: Option<BoltV3OutcomeSide>,
    forced_flat_reasons: Vec<BoltV3ForcedFlatReason>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    realized_vol: Option<String>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    exit_eval_now_ms: u64,
    exit_trigger_source: BoltV3ExitTriggerSource,
    trigger_ts_event_ms: u64,
    trigger_ts_init_ms: Option<u64>,
    rv_surface_id: String,
    rv_snapshot_as_of_ms: Option<u64>,
    rv_snapshot_ready: bool,
    rv_snapshot_has_ready_realized_vol: Option<bool>,
    rv_snapshot_receive_watermark_ms: Option<u64>,
    rv_max_source_age_ms: Option<u64>,
    rv_snapshot_blockers: Vec<BoltV3ExitRvSnapshotBlocker>,
    rv_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    rv_gate_result: BoltV3ExitRvGateResult,
    rv_future_dating_delta_ms: Option<u64>,
    exit_hysteresis_bps: String,
    exit_decision: BoltV3ExitDecisionOutcome,
    blocked_reason: Option<BoltV3ExitBlockedReason>,
    client_order_id: Option<String>,
    submission_order_side: Option<String>,
    submission_price: Option<String>,
    submission_quantity: Option<String>,
    seconds_to_market_end: Option<u64>,
    ts_ms: u64,
    stale_reference_after_ms: Option<u64>,
    last_reference_ts_ms: Option<u64>,
    min_liquidity_required: Option<String>,
    liquidity_available: Option<String>,
    frozen: bool,
    metadata_matches_selection: bool,
    fast_venue_incoherent: bool,
}

impl<'de> Deserialize<'de> for BoltV3ExitDecisionEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3ExitDecisionEvidenceWire::deserialize(deserializer)?;
        Ok(Self {
            strategy_id: wire.strategy_id,
            market_id: wire.market_id,
            position_id: wire.position_id,
            position_instrument_id: wire.position_instrument_id,
            position_outcome_side: wire.position_outcome_side,
            forced_flat_reasons: wire.forced_flat_reasons,
            spot_price: wire.spot_price,
            spot_venue_name: wire.spot_venue_name,
            fast_venue_available: wire.fast_venue_available,
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire.reference_current_price_available,
            interval_open: wire.interval_open,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            hold_ev_bps: wire.hold_ev_bps,
            exit_ev_bps: wire.exit_ev_bps,
            realized_vol: wire.realized_vol,
            realized_vol_source_venue: wire.realized_vol_source_venue,
            realized_vol_source_ts_ms: wire.realized_vol_source_ts_ms,
            exit_eval_now_ms: wire.exit_eval_now_ms,
            exit_trigger_source: wire.exit_trigger_source,
            trigger_ts_event_ms: wire.trigger_ts_event_ms,
            trigger_ts_init_ms: wire.trigger_ts_init_ms,
            rv_surface_id: wire.rv_surface_id,
            rv_snapshot_as_of_ms: wire.rv_snapshot_as_of_ms,
            rv_snapshot_ready: wire.rv_snapshot_ready,
            rv_snapshot_has_ready_realized_vol: wire.rv_snapshot_has_ready_realized_vol,
            rv_snapshot_receive_watermark_ms: wire.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: wire.rv_max_source_age_ms,
            rv_snapshot_blockers: wire.rv_snapshot_blockers,
            rv_source_diagnostics: wire.rv_source_diagnostics,
            rv_gate_result: wire.rv_gate_result,
            rv_future_dating_delta_ms: wire.rv_future_dating_delta_ms,
            exit_hysteresis_bps: wire.exit_hysteresis_bps,
            exit_decision: wire.exit_decision,
            blocked_reason: wire.blocked_reason,
            client_order_id: wire.client_order_id,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            seconds_to_market_end: wire.seconds_to_market_end,
            ts_ms: wire.ts_ms,
            stale_reference_after_ms: wire.stale_reference_after_ms,
            last_reference_ts_ms: wire.last_reference_ts_ms,
            min_liquidity_required: wire.min_liquidity_required,
            liquidity_available: wire.liquidity_available,
            frozen: wire.frozen,
            metadata_matches_selection: wire.metadata_matches_selection,
            fast_venue_incoherent: wire.fast_venue_incoherent,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossHaltReason {
    PerTradeLossLimit,
    DailyLossLimit,
    RollingLossLimit,
    MaxDrawdownLimit,
    StaleLossSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossSnapshotSource {
    NtLossRuntimeFeed,
    NtPortfolioSnapshot,
    NtAccountSnapshot,
    NtAccountAndPositionSnapshot,
    NtPositionEvent,
    NtPositionChanged,
    NtPositionClosed,
    NtPositionAdjusted,
    NtCapitalAdmissionState,
    BoltLossSnapshot,
    LossGovernor,
    Unknown,
    Other,
}

const LOSS_SNAPSHOT_SOURCE_NT_LOSS_RUNTIME_FEED: &str = stringify!(nt_loss_runtime_feed);
const LOSS_SNAPSHOT_SOURCE_NT_PORTFOLIO_SNAPSHOT: &str = stringify!(nt_portfolio_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_SNAPSHOT: &str = stringify!(nt_account_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_AND_POSITION_SNAPSHOT: &str =
    stringify!(nt_account_and_position_snapshot);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_EVENT: &str = stringify!(nt_position_event);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CHANGED: &str = stringify!(nt_position_changed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_CLOSED: &str = stringify!(nt_position_closed);
const LOSS_SNAPSHOT_SOURCE_NT_POSITION_ADJUSTED: &str = stringify!(nt_position_adjusted);
const LOSS_SNAPSHOT_SOURCE_NT_CAPITAL_ADMISSION_STATE: &str =
    stringify!(nt_capital_admission_state);
const LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT: &str = stringify!(bolt_loss_snapshot);
const LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR: &str = stringify!(loss_governor);

#[must_use]
pub fn loss_snapshot_source_to_evidence(source: &str) -> BoltV3LossSnapshotSource {
    match source {
        LOSS_SNAPSHOT_SOURCE_NT_LOSS_RUNTIME_FEED => BoltV3LossSnapshotSource::NtLossRuntimeFeed,
        LOSS_SNAPSHOT_SOURCE_NT_PORTFOLIO_SNAPSHOT => BoltV3LossSnapshotSource::NtPortfolioSnapshot,
        LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_SNAPSHOT => BoltV3LossSnapshotSource::NtAccountSnapshot,
        LOSS_SNAPSHOT_SOURCE_NT_ACCOUNT_AND_POSITION_SNAPSHOT => {
            BoltV3LossSnapshotSource::NtAccountAndPositionSnapshot
        }
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_EVENT => BoltV3LossSnapshotSource::NtPositionEvent,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_CHANGED => BoltV3LossSnapshotSource::NtPositionChanged,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_CLOSED => BoltV3LossSnapshotSource::NtPositionClosed,
        LOSS_SNAPSHOT_SOURCE_NT_POSITION_ADJUSTED => BoltV3LossSnapshotSource::NtPositionAdjusted,
        LOSS_SNAPSHOT_SOURCE_NT_CAPITAL_ADMISSION_STATE => {
            BoltV3LossSnapshotSource::NtCapitalAdmissionState
        }
        LOSS_SNAPSHOT_SOURCE_BOLT_LOSS_SNAPSHOT => BoltV3LossSnapshotSource::BoltLossSnapshot,
        LOSS_SNAPSHOT_SOURCE_LOSS_GOVERNOR => BoltV3LossSnapshotSource::LossGovernor,
        _ if source.trim().is_empty() => BoltV3LossSnapshotSource::Unknown,
        _ => BoltV3LossSnapshotSource::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3LossSnapshotStaleReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3TradingState {
    Active,
    Halted,
    Reducing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteActionCostClass {
    FreshSubmit,
    CancelResubmit,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteThrottleBlockReason {
    RequoteBudgetExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RequoteThrottleBound {
    SubmitCommandWindow,
    RestCallWindow,
    MinInterval,
    WindowCap,
    OutOfOrderTs,
    Overflow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3RequoteThrottleEvidence {
    pub strategy_id: String,
    pub family_key: String,
    pub market_id: Option<String>,
    pub leg: String,
    pub now_ms: u64,
    pub observed_at_ns: u64,
    pub action_cost_class: BoltV3RequoteActionCostClass,
    pub block_reason: BoltV3RequoteThrottleBlockReason,
    pub bound_by: BoltV3RequoteThrottleBound,
    pub submit_commands_in_window: usize,
    pub submit_command_cap: u64,
    pub submit_window_ms: u64,
    pub rest_cost_in_window: u64,
    pub rest_cap_per_minute: u64,
    pub rest_window_ms: u64,
    pub min_interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SettlementEvidence {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: String,
    pub position_id: String,
    pub instrument_id: String,
    pub product_id: String,
    pub outcome_side: BoltV3OutcomeSide,
    pub entry_order_side: String,
    pub quantity: String,
    pub entry_price: String,
    pub family_key: String,
    pub strike_price: String,
    pub resolution_instrument_id: String,
    pub resolution_ts_event_ns: u64,
    pub reference_close_price: String,
    pub payout_per_share: String,
    pub terminal_value: String,
    pub realized_pnl: String,
    pub settlement_currency: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3SettlementBookingErrorReason {
    ResolutionFeedMissing,
    SettlementAlreadyBooked,
    SettlementInputInvalid,
    SettlementBlocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SettlementBookingErrorEvidence {
    pub strategy_id: String,
    pub settlement_key: String,
    pub market_id: Option<String>,
    pub position_id: Option<String>,
    pub instrument_id: Option<String>,
    pub resolution_instrument_id: Option<String>,
    pub reason: BoltV3SettlementBookingErrorReason,
    pub detail: String,
    pub observed_at_ns: u64,
}

/// The canonical durable result of either valid terminal-eligibility leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3TerminalSettlementEvidence {
    pub settlement_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub booking_error: Option<BoltV3SettlementBookingErrorEvidence>,
    pub lifecycle: BoltV3OrderLifecycleEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StrategyInputEvidenceSnapshot {
    pub strategy_id: String,
    pub configured_target_id: String,
    pub market_selection_ruleset_id: String,
    pub market_selection_outcome: String,
    pub market_id: Option<String>,
    pub polymarket_condition_id: Option<String>,
    pub polymarket_market_slug: Option<String>,
    pub polymarket_question_id: Option<String>,
    pub up_instrument_id: Option<String>,
    pub down_instrument_id: Option<String>,
    pub market_selection_timestamp_ms: Option<u64>,
    pub selected_market_observed_timestamp_ms: Option<u64>,
    pub polymarket_market_start_timestamp_ms: Option<u64>,
    pub polymarket_market_end_timestamp_ms: Option<u64>,
    pub price_to_beat_source: String,
    pub price_to_beat_value: String,
    pub reference_quote_ts_event: u64,
    pub spot_price: String,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub reference_current_price_source_id: Option<String>,
    pub reference_current_price_failed_over: Option<bool>,
    pub realized_volatility: String,
    pub realized_volatility_surface_id: String,
    pub realized_volatility_as_of_ms: Option<u64>,
    pub realized_volatility_gate_result: Option<BoltV3RvGateResult>,
    #[serde(serialize_with = "serialize_optional_local_receive_ms")]
    pub realized_volatility_receive_watermark_ms: Option<LocalReceiveMs>,
    pub realized_volatility_annualized_decimal: String,
    pub realized_volatility_measured_annualized_decimal: String,
    pub realized_volatility_noise_robust_annualized_decimal: String,
    pub realized_volatility_continuous_annualized_decimal: String,
    pub realized_volatility_jump_annualized_decimal: String,
    pub realized_volatility_forecast_annualized_decimal: String,
    pub realized_volatility_pricing_component: String,
    pub realized_volatility_seconds_per_annum: String,
    pub realized_volatility_aggregation: String,
    pub realized_volatility_sources_used: Vec<String>,
    pub realized_volatility_source_diagnostics:
        Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    pub realized_volatility_unknown_source_rejections: BTreeMap<String, u64>,
    pub realized_volatility_blockers: Vec<String>,
    pub realized_volatility_config_fingerprint: String,
    pub seconds_to_market_end: u64,
    pub pricing_kurtosis: String,
    pub theta_decay_factor: String,
    pub theta_scaled_min_edge_bps: String,
    pub fair_probability_up: String,
    pub uncertainty_band_probability: String,
    pub expected_edge_basis_points: String,
    pub worst_case_edge_basis_points: String,
    pub up_worst_case_edge_basis_points: Option<String>,
    pub down_worst_case_edge_basis_points: Option<String>,
    pub gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pub pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    pub fast_venue_name: Option<String>,
    pub fast_venue_age_ms: Option<u64>,
    pub fast_venue_jitter_ms: Option<u64>,
    pub fast_venue_incoherent: bool,
    pub lead_agreement_corr: Option<String>,
    pub fee_rate_basis_points: String,
    pub selected_side: Option<String>,
    pub submission_instrument_id: String,
    pub submission_order_side: String,
    pub submission_price: String,
    pub submission_quantity: String,
    pub client_order_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3StrategyInputEvidenceSnapshotWire {
    strategy_id: String,
    configured_target_id: String,
    market_selection_ruleset_id: String,
    market_selection_outcome: String,
    market_id: Option<String>,
    polymarket_condition_id: Option<String>,
    polymarket_market_slug: Option<String>,
    polymarket_question_id: Option<String>,
    up_instrument_id: Option<String>,
    down_instrument_id: Option<String>,
    market_selection_timestamp_ms: Option<u64>,
    selected_market_observed_timestamp_ms: Option<u64>,
    polymarket_market_start_timestamp_ms: Option<u64>,
    polymarket_market_end_timestamp_ms: Option<u64>,
    price_to_beat_source: String,
    price_to_beat_value: String,
    reference_quote_ts_event: u64,
    spot_price: String,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    reference_current_price_source_id: Option<String>,
    reference_current_price_failed_over: Option<bool>,
    realized_volatility: String,
    realized_volatility_surface_id: String,
    realized_volatility_as_of_ms: Option<u64>,
    realized_volatility_gate_result: Option<BoltV3RvGateResult>,
    realized_volatility_receive_watermark_ms: Option<u64>,
    realized_volatility_annualized_decimal: String,
    realized_volatility_measured_annualized_decimal: String,
    realized_volatility_noise_robust_annualized_decimal: String,
    realized_volatility_continuous_annualized_decimal: String,
    realized_volatility_jump_annualized_decimal: String,
    realized_volatility_forecast_annualized_decimal: String,
    realized_volatility_pricing_component: String,
    realized_volatility_seconds_per_annum: String,
    realized_volatility_aggregation: String,
    realized_volatility_sources_used: Vec<String>,
    realized_volatility_source_diagnostics: Vec<BoltV3RealizedVolatilitySourceDiagnosticEvidence>,
    realized_volatility_unknown_source_rejections: BTreeMap<String, u64>,
    realized_volatility_blockers: Vec<String>,
    realized_volatility_config_fingerprint: String,
    seconds_to_market_end: u64,
    pricing_kurtosis: String,
    theta_decay_factor: String,
    theta_scaled_min_edge_bps: String,
    fair_probability_up: String,
    uncertainty_band_probability: String,
    expected_edge_basis_points: String,
    worst_case_edge_basis_points: String,
    up_worst_case_edge_basis_points: Option<String>,
    down_worst_case_edge_basis_points: Option<String>,
    gate_blocked_by: Vec<BoltV3EntryBlockReason>,
    pricing_blocked_by: Vec<BoltV3EntryPricingBlockReason>,
    fast_venue_name: Option<String>,
    fast_venue_age_ms: Option<u64>,
    fast_venue_jitter_ms: Option<u64>,
    fast_venue_incoherent: bool,
    lead_agreement_corr: Option<String>,
    fee_rate_basis_points: String,
    selected_side: Option<String>,
    submission_instrument_id: String,
    submission_order_side: String,
    submission_price: String,
    submission_quantity: String,
    client_order_id: String,
}

impl<'de> Deserialize<'de> for BoltV3StrategyInputEvidenceSnapshot {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3StrategyInputEvidenceSnapshotWire::deserialize(deserializer)?;
        Ok(Self {
            strategy_id: wire.strategy_id,
            configured_target_id: wire.configured_target_id,
            market_selection_ruleset_id: wire.market_selection_ruleset_id,
            market_selection_outcome: wire.market_selection_outcome,
            market_id: wire.market_id,
            polymarket_condition_id: wire.polymarket_condition_id,
            polymarket_market_slug: wire.polymarket_market_slug,
            polymarket_question_id: wire.polymarket_question_id,
            up_instrument_id: wire.up_instrument_id,
            down_instrument_id: wire.down_instrument_id,
            market_selection_timestamp_ms: wire.market_selection_timestamp_ms,
            selected_market_observed_timestamp_ms: wire.selected_market_observed_timestamp_ms,
            polymarket_market_start_timestamp_ms: wire.polymarket_market_start_timestamp_ms,
            polymarket_market_end_timestamp_ms: wire.polymarket_market_end_timestamp_ms,
            price_to_beat_source: wire.price_to_beat_source,
            price_to_beat_value: wire.price_to_beat_value,
            reference_quote_ts_event: wire.reference_quote_ts_event,
            spot_price: wire.spot_price,
            fast_venue_available: wire.fast_venue_available,
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire.reference_current_price_available,
            reference_current_price_source_id: wire.reference_current_price_source_id,
            reference_current_price_failed_over: wire.reference_current_price_failed_over,
            realized_volatility: wire.realized_volatility,
            realized_volatility_surface_id: wire.realized_volatility_surface_id,
            realized_volatility_as_of_ms: wire.realized_volatility_as_of_ms,
            realized_volatility_gate_result: wire.realized_volatility_gate_result,
            realized_volatility_receive_watermark_ms: wire
                .realized_volatility_receive_watermark_ms
                .map(LocalReceiveMs::new),
            realized_volatility_annualized_decimal: wire.realized_volatility_annualized_decimal,
            realized_volatility_measured_annualized_decimal: wire
                .realized_volatility_measured_annualized_decimal,
            realized_volatility_noise_robust_annualized_decimal: wire
                .realized_volatility_noise_robust_annualized_decimal,
            realized_volatility_continuous_annualized_decimal: wire
                .realized_volatility_continuous_annualized_decimal,
            realized_volatility_jump_annualized_decimal: wire
                .realized_volatility_jump_annualized_decimal,
            realized_volatility_forecast_annualized_decimal: wire
                .realized_volatility_forecast_annualized_decimal,
            realized_volatility_pricing_component: wire.realized_volatility_pricing_component,
            realized_volatility_seconds_per_annum: wire.realized_volatility_seconds_per_annum,
            realized_volatility_aggregation: wire.realized_volatility_aggregation,
            realized_volatility_sources_used: wire.realized_volatility_sources_used,
            realized_volatility_source_diagnostics: wire.realized_volatility_source_diagnostics,
            realized_volatility_unknown_source_rejections: wire
                .realized_volatility_unknown_source_rejections,
            realized_volatility_blockers: wire.realized_volatility_blockers,
            realized_volatility_config_fingerprint: wire.realized_volatility_config_fingerprint,
            seconds_to_market_end: wire.seconds_to_market_end,
            pricing_kurtosis: wire.pricing_kurtosis,
            theta_decay_factor: wire.theta_decay_factor,
            theta_scaled_min_edge_bps: wire.theta_scaled_min_edge_bps,
            fair_probability_up: wire.fair_probability_up,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            expected_edge_basis_points: wire.expected_edge_basis_points,
            worst_case_edge_basis_points: wire.worst_case_edge_basis_points,
            up_worst_case_edge_basis_points: wire.up_worst_case_edge_basis_points,
            down_worst_case_edge_basis_points: wire.down_worst_case_edge_basis_points,
            gate_blocked_by: wire.gate_blocked_by,
            pricing_blocked_by: wire.pricing_blocked_by,
            fast_venue_name: wire.fast_venue_name,
            fast_venue_age_ms: wire.fast_venue_age_ms,
            fast_venue_jitter_ms: wire.fast_venue_jitter_ms,
            fast_venue_incoherent: wire.fast_venue_incoherent,
            lead_agreement_corr: wire.lead_agreement_corr,
            fee_rate_basis_points: wire.fee_rate_basis_points,
            selected_side: wire.selected_side,
            submission_instrument_id: wire.submission_instrument_id,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            client_order_id: wire.client_order_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3AdmissionOutcome {
    Admitted,
    RejectedKillSwitchLatched,
    RejectedSubmitLifecycleDisallowed,
    RejectedLossGovernorHalted,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedInvalidRiskReducingExitProof,
    RejectedCountCapExhausted,
    RejectedKillSwitchForcedReductionProofInvalid,
    RejectedKillSwitchForcedReductionCapExceeded,
    RejectedCapitalAdmission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3AdmissionDecisionEvidence {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: String,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub outcome: BoltV3AdmissionOutcome,
    pub loss_halt_reasons: Vec<BoltV3LossHaltReason>,
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: Option<u64>,
    pub snapshot_source: Option<BoltV3LossSnapshotSource>,
    pub per_trade_pnl_present: bool,
    pub daily_pnl_present: bool,
    pub rolling_pnl_present: bool,
    pub current_equity_present: bool,
    pub peak_equity_present: bool,
    pub last_account_state_observed_at_ns: Option<u64>,
    pub last_portfolio_snapshot_observed_at_ns: Option<u64>,
    pub last_position_event_observed_at_ns: Option<u64>,
    pub stale_reason: Option<BoltV3LossSnapshotStaleReason>,
    pub loss_snapshot_observed_at_ns: Option<u64>,
    pub loss_eval_now_ns: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3CapitalAdmissionRebuildAuditEvidence {
    pub observed_at_ns: u64,
    pub source: String,
    pub observed_open_order_count: usize,
    pub all_open_orders_attributed: bool,
    pub accepted: bool,
    pub reason: Option<ReservationRejectionReason>,
    pub attempted_reservation_count: usize,
    pub recovered_reservation_count: usize,
    pub live_reserved_liability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SubmitReservationMetadataEvidence {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub venue_id: String,
    pub account_id: String,
    pub product_kind: String,
    pub collateral_currency: String,
    pub capital_pool_id: String,
    pub collateral_group_id: String,
    pub instrument_id: String,
    pub side: String,
    pub submitted_quantity: String,
    pub liability_factor: String,
    pub additive_liability: String,
    pub reserved_liability: String,
    pub observed_at_ns: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3SubmitReservationFillEvidence {
    pub client_order_id: String,
    pub submit_reservation_id: String,
    pub trade_id: String,
    pub instrument_id: String,
    pub side: String,
    pub fill_quantity: String,
    pub observed_at_ns: u64,
    pub reconciliation: bool,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3BasketAdmissionOutcome {
    Admitted,
    RejectedBasketNotionalCapExceeded,
    RejectedMaxOpenBasketCapExceeded,
    RejectedStaleScannerEvidence,
    RejectedStaleSubmitRecheck,
    RejectedNonPositiveCandidateCost,
    RejectedNonPositiveEdge,
    RejectedEdgeThreshold,
    RejectedMissingGroupingProof,
    RejectedMissingSettlementRules,
    RejectedRetryBudgetExceeded,
    RejectedSubmitSlots,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3BasketAdmissionDecisionEvidence {
    pub strategy_id: String,
    pub execution_client_id: String,
    pub basket_id: String,
    pub group_id: String,
    pub leg_instrument_ids: Vec<String>,
    pub total_notional: String,
    pub leg_order_count: u32,
    pub outcome: BoltV3BasketAdmissionOutcome,
}

/// Closed result of the realized-vol staleness gate as observed at exit-evaluation
/// time. `RejectedFutureDated` and `RejectedNotReady` are split apart here even
/// though the production gate collapses them into a single `None`, so evidence can
/// distinguish the incident's future-dated case from a not-yet-warm surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RvGateResult {
    Accepted,
    MissingSnapshot,
    MissingEvaluationEventTime,
    RejectedFutureDated,
    RejectedStale,
    RejectedNotReady,
}

/// Closed taxonomy of *why* a loss-governor snapshot was treated as stale. The
/// production governor collapses all of these into one `StaleLossSnapshot` halt
/// reason; this enum is the evidence-only decomposition (behaviour-neutral).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3StaleLossReason {
    MissingSnapshot,
    SourceEmpty,
    FutureDated,
    AgeExceeded,
    MissingRequiredField,
}

/// Closed taxonomy of the layer that rejected an order, for `BoltV3OrderRejectEvidence`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3RejectSource {
    SubmitAdmission,
    Venue,
    NtExecution,
    Internal,
}

/// Closed, best-effort classification of a venue/NT order rejection. The raw NT
/// reason text is preserved separately in `BoltV3OrderRejectEvidence`; this enum is
/// only the structural bucket so reads stay enum-driven (no free-form reason blobs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderRejectReason {
    AdmissionRejected,
    PrecisionRejected,
    MinSizeRejected,
    MinNotionalRejected,
    InsufficientBalance,
    DuplicateClientOrderId,
    Other,
}

/// Durable evidence for a single exit evaluation (RCA #885 root causes 1-4). Decimal
/// quantities are stored as strings and timestamps as integers, matching the existing
/// records in this module. Flood-gated by the strategy: emitted on outcome-key change
/// or actual submission, never per tick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3ExitEvaluationEvidence {
    pub position_id: Option<String>,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub client_order_id: Option<String>,
    pub exit_eval_now_ms: i64,
    pub exit_trigger_source: BoltV3ExitTriggerSource,
    pub trigger_ts_event_ms: Option<i64>,
    pub trigger_ts_init_ms: Option<i64>,
    pub rv_surface_id: String,
    pub rv_as_of_ms: Option<i64>,
    pub rv_ready: bool,
    pub rv_snapshot_receive_watermark_ms: Option<i64>,
    pub rv_max_source_age_ms: Option<u64>,
    pub rv_blockers: Vec<String>,
    pub rv_source_diagnostics: Vec<String>,
    pub rv_gate_result: BoltV3RvGateResult,
    pub rv_as_of_minus_now_ms: Option<i64>,
    pub spot_price: Option<String>,
    pub spot_venue_name: Option<String>,
    pub fast_venue_available: bool,
    pub reference_current_price: Option<String>,
    pub reference_current_price_available: bool,
    pub interval_open: Option<String>,
    pub fair_probability_up: Option<String>,
    pub fair_probability_down: Option<String>,
    pub uncertainty_band_probability: Option<String>,
    pub up_fee_bps: Option<String>,
    pub down_fee_bps: Option<String>,
    pub hold_ev_bps: Option<String>,
    pub exit_ev_bps: Option<String>,
    pub exit_decision: BoltV3ExitDecisionOutcome,
    pub forced_flat_reasons: Vec<String>,
    pub submission_order_side: Option<String>,
    pub submission_price: Option<String>,
    pub submission_quantity: Option<String>,
    pub submission_blocked_reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BoltV3ExitEvaluationEvidenceWire {
    position_id: Option<String>,
    market_id: Option<String>,
    instrument_id: Option<String>,
    client_order_id: Option<String>,
    exit_eval_now_ms: i64,
    exit_trigger_source: BoltV3ExitTriggerSource,
    trigger_ts_event_ms: Option<i64>,
    trigger_ts_init_ms: Option<i64>,
    rv_surface_id: String,
    rv_as_of_ms: Option<i64>,
    rv_ready: bool,
    rv_snapshot_receive_watermark_ms: Option<i64>,
    rv_max_source_age_ms: Option<u64>,
    rv_blockers: Vec<String>,
    rv_source_diagnostics: Vec<String>,
    rv_gate_result: BoltV3RvGateResult,
    rv_as_of_minus_now_ms: Option<i64>,
    spot_price: Option<String>,
    spot_venue_name: Option<String>,
    fast_venue_available: bool,
    reference_current_price: Option<String>,
    reference_current_price_available: bool,
    interval_open: Option<String>,
    fair_probability_up: Option<String>,
    fair_probability_down: Option<String>,
    uncertainty_band_probability: Option<String>,
    up_fee_bps: Option<String>,
    down_fee_bps: Option<String>,
    hold_ev_bps: Option<String>,
    exit_ev_bps: Option<String>,
    exit_decision: BoltV3ExitDecisionOutcome,
    forced_flat_reasons: Vec<String>,
    submission_order_side: Option<String>,
    submission_price: Option<String>,
    submission_quantity: Option<String>,
    submission_blocked_reason: Option<String>,
}

impl<'de> Deserialize<'de> for BoltV3ExitEvaluationEvidence {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = BoltV3ExitEvaluationEvidenceWire::deserialize(deserializer)?;
        if wire.trigger_ts_init_ms.is_some_and(i64::is_negative) {
            return Err(serde::de::Error::custom(
                "trigger_ts_init_ms must be non-negative",
            ));
        }
        if wire
            .rv_snapshot_receive_watermark_ms
            .is_some_and(i64::is_negative)
        {
            return Err(serde::de::Error::custom(
                "rv_snapshot_receive_watermark_ms must be non-negative",
            ));
        }
        Ok(Self {
            position_id: wire.position_id,
            market_id: wire.market_id,
            instrument_id: wire.instrument_id,
            client_order_id: wire.client_order_id,
            exit_eval_now_ms: wire.exit_eval_now_ms,
            exit_trigger_source: wire.exit_trigger_source,
            trigger_ts_event_ms: wire.trigger_ts_event_ms,
            trigger_ts_init_ms: wire.trigger_ts_init_ms,
            rv_surface_id: wire.rv_surface_id,
            rv_as_of_ms: wire.rv_as_of_ms,
            rv_ready: wire.rv_ready,
            rv_snapshot_receive_watermark_ms: wire.rv_snapshot_receive_watermark_ms,
            rv_max_source_age_ms: wire.rv_max_source_age_ms,
            rv_blockers: wire.rv_blockers,
            rv_source_diagnostics: wire.rv_source_diagnostics,
            rv_gate_result: wire.rv_gate_result,
            rv_as_of_minus_now_ms: wire.rv_as_of_minus_now_ms,
            spot_price: wire.spot_price,
            spot_venue_name: wire.spot_venue_name,
            fast_venue_available: wire.fast_venue_available,
            reference_current_price: wire.reference_current_price,
            reference_current_price_available: wire.reference_current_price_available,
            interval_open: wire.interval_open,
            fair_probability_up: wire.fair_probability_up,
            fair_probability_down: wire.fair_probability_down,
            uncertainty_band_probability: wire.uncertainty_band_probability,
            up_fee_bps: wire.up_fee_bps,
            down_fee_bps: wire.down_fee_bps,
            hold_ev_bps: wire.hold_ev_bps,
            exit_ev_bps: wire.exit_ev_bps,
            exit_decision: wire.exit_decision,
            forced_flat_reasons: wire.forced_flat_reasons,
            submission_order_side: wire.submission_order_side,
            submission_price: wire.submission_price,
            submission_quantity: wire.submission_quantity,
            submission_blocked_reason: wire.submission_blocked_reason,
        })
    }
}

/// Durable evidence for a loss-governor halt observed at the submit-admission consumer
/// (RCA #885 root cause 5). Captures snapshot freshness and per-source last-seen
/// timestamps so a stale halt can be explained from disk. Episode-gated (exponential
/// sampling keyed by `stable_halt_key`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3LossGovernorHaltEvidence {
    pub snapshot_present: bool,
    pub snapshot_observed_at_ns: Option<u64>,
    pub admission_now_ns: u64,
    pub snapshot_age_ns: Option<u64>,
    pub max_snapshot_age_ns: u64,
    pub snapshot_source: Option<String>,
    pub has_per_trade_pnl: bool,
    pub has_daily_pnl: bool,
    pub has_rolling_pnl: bool,
    pub has_current_equity: bool,
    pub has_peak_equity: bool,
    pub last_account_state_ts_ns: Option<u64>,
    pub last_portfolio_snapshot_ts_ns: Option<u64>,
    pub last_position_event_ts_ns: Option<u64>,
    pub account_state_count: u64,
    pub portfolio_snapshot_count: u64,
    pub position_event_count: u64,
    pub stale_reason: BoltV3StaleLossReason,
    pub stable_halt_key: String,
    pub retry_count: u32,
    pub elapsed_since_first_halt_ns: u64,
}

/// Durable evidence correlating an order rejection with its retry episode (RCA #885
/// root causes 6-7). Raw and normalized amounts plus venue precision constraints are
/// captured best-effort; bolt never computes NT-adapter-internal maker/taker amounts,
/// so those stay null unless the NT event exposes them. Episode-gated by
/// `stable_episode_key`, which excludes the per-attempt client_order_id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderRejectEvidence {
    pub reject_source: BoltV3RejectSource,
    pub reject_reason: BoltV3OrderRejectReason,
    pub admission_outcome: Option<BoltV3AdmissionOutcome>,
    pub raw_reason_text: Option<String>,
    pub instrument_id: String,
    pub order_side: Option<String>,
    pub raw_price: Option<String>,
    pub raw_quantity: Option<String>,
    pub raw_maker_amount: Option<String>,
    pub raw_taker_amount: Option<String>,
    pub normalized_price: Option<String>,
    pub normalized_quantity: Option<String>,
    pub normalized_maker_amount: Option<String>,
    pub normalized_taker_amount: Option<String>,
    pub venue_price_precision: Option<u32>,
    pub venue_size_precision: Option<u32>,
    pub venue_min_notional: Option<String>,
    pub prior_client_order_id: Option<String>,
    pub client_order_id: String,
    pub retry_count: u32,
    pub backoff_cooldown_state: Option<String>,
    pub stable_episode_key: String,
    pub elapsed_ns: u64,
}

/// Closed taxonomy of strategy-local order lifecycle transitions. These records
/// make lifecycle substitutes distinguishable from normal submit/fill evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderLifecycleTransition {
    BoundaryReclassification,
    EntryFillMaterialized,
    EntryReconcilePending,
    PositionTruthRematerialized,
    PositionClosed,
    ResidualRemanaged,
    RestartOpenOrderAdopted,
    RestartOpenOrderRecoveryBlocked,
    SettlementEvidenceRecoveryBlocked,
    /// Live or recovery path: settlement booking failed terminally; exposure released.
    SettlementBookingTerminal,
    OrderDenied,
    OrderRejected,
    OrderCanceled,
    OrderExpired,
    OrderFilled,
    ReconcileQueryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderLifecycleOutcome {
    PendingEntry,
    Managed,
    ExitPending,
    EntryReconcilePending,
    UnsupportedObserved,
    BlindRecovery,
    Flat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3OrderLifecycleEvidence {
    pub strategy_id: String,
    pub transition: BoltV3OrderLifecycleTransition,
    pub outcome: BoltV3OrderLifecycleOutcome,
    pub source: String,
    pub market_id: Option<String>,
    pub instrument_id: Option<String>,
    pub position_id: Option<String>,
    pub client_order_id: Option<String>,
    pub prior_client_order_id: Option<String>,
    pub raw_reason_text: Option<String>,
    pub order_side: Option<String>,
    pub filled_quantity: Option<String>,
    pub residual_quantity: Option<String>,
    pub ts_event_ns: Option<u64>,
}

#[derive(Debug)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    sink: sink::JsonlDecisionEvidenceSink,
}

impl JsonlBoltV3DecisionEvidenceWriter {
    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        Ok(Self {
            sink: sink::JsonlDecisionEvidenceSink::from_loaded_config(loaded)?,
        })
    }

    #[cfg(test)]
    pub(crate) fn from_test_path(path: &Path) -> Result<Self> {
        let observation_path = path.with_extension("observations.jsonl");
        Ok(Self {
            sink: sink::JsonlDecisionEvidenceSink::from_paths(
                path.to_path_buf(),
                observation_path,
            )?,
        })
    }

    pub fn drain_shutdown(&self) -> Result<()> {
        use sink::DecisionEvidenceSink as _;

        self.sink.drain_shutdown()
    }
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn try_record_command(&self, command: BoltV3DecisionEvidenceCommand) -> Result<()> {
        use sink::DecisionEvidenceSink as _;

        let record = command.encode()?;
        self.sink
            .append(record)
            .map(|_| ())
            .map_err(sink::RecordError::into_anyhow)
    }

    fn drain_shutdown(&self) -> Result<()> {
        JsonlBoltV3DecisionEvidenceWriter::drain_shutdown(self)
    }
}

pub(crate) fn validate_decision_evidence_relative_path(
    field: &str,
    raw: &str,
) -> Result<(), String> {
    let relative = Path::new(raw.trim());
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        return Err(format!(
            "persistence.decision_evidence.{field} must be non-empty, relative, and stay under catalog_directory"
        ));
    }
    Ok(())
}

fn configured_decision_evidence_path(
    loaded: &LoadedBoltV3Config,
    field: &str,
    raw: &str,
) -> Result<PathBuf> {
    validate_decision_evidence_relative_path(field, raw).map_err(|message| anyhow!(message))?;
    Ok(Path::new(&loaded.root.persistence.catalog_directory).join(Path::new(raw.trim())))
}

pub fn machine_decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    configured_decision_evidence_path(
        loaded,
        "machine_relative_path",
        &loaded
            .root
            .persistence
            .decision_evidence
            .machine_relative_path,
    )
}

pub fn observation_decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    configured_decision_evidence_path(
        loaded,
        "observation_relative_path",
        &loaded
            .root
            .persistence
            .decision_evidence
            .observation_relative_path,
    )
}

pub fn retired_decision_evidence_paths(loaded: &LoadedBoltV3Config) -> Result<Vec<PathBuf>> {
    loaded
        .root
        .persistence
        .decision_evidence
        .retired_relative_paths
        .iter()
        .map(|raw| configured_decision_evidence_path(loaded, "retired_relative_paths", raw))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3SubmitReservationRecoveryEvidence {
    pub metadata_by_client_order_id: BTreeMap<String, BoltV3RecoveredSubmitReservationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3RecoveredSubmitReservationEvidence {
    pub metadata: BoltV3SubmitReservationMetadataEvidence,
    pub fill_trade_ids: BTreeSet<String>,
}

pub fn read_submit_reservation_recovery_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<BoltV3SubmitReservationRecoveryEvidence> {
    use facts::{
        ReservationProductKind, ReservationSide, SubmitReservationRecoveryEvent,
        route_submit_reservation_recovery,
    };

    let mut metadata_by_client_order_id =
        BTreeMap::<String, BoltV3SubmitReservationMetadataEvidence>::new();
    let mut fills = Vec::<BoltV3SubmitReservationFillEvidence>::new();

    for fact in decode::read_registered_facts(path.as_ref(), max_bytes)? {
        let Some(event) = route_submit_reservation_recovery(fact)? else {
            continue;
        };
        match event {
            SubmitReservationRecoveryEvent::Metadata(metadata) => {
                let metadata = BoltV3SubmitReservationMetadataEvidence {
                    client_order_id: metadata.client_order_id,
                    submit_reservation_id: metadata.submit_reservation_id,
                    venue_id: metadata.venue_id,
                    account_id: metadata.account_id,
                    product_kind: match metadata.product_kind {
                        ReservationProductKind::PredictionMarketBinary => {
                            SUBMIT_RESERVATION_METADATA_PRODUCT_KIND_BINARY.to_string()
                        }
                    },
                    collateral_currency: metadata.collateral_currency,
                    capital_pool_id: metadata.capital_pool_id,
                    collateral_group_id: metadata.collateral_group_id,
                    instrument_id: metadata.instrument_id,
                    side: match metadata.side {
                        ReservationSide::Buy => SUBMIT_RESERVATION_METADATA_SIDE_BUY.to_string(),
                        ReservationSide::Sell => SUBMIT_RESERVATION_METADATA_SIDE_SELL.to_string(),
                    },
                    submitted_quantity: metadata.submitted_quantity.normalize().to_string(),
                    liability_factor: metadata.liability_factor.normalize().to_string(),
                    additive_liability: metadata.additive_liability.normalize().to_string(),
                    reserved_liability: metadata.reserved_liability.normalize().to_string(),
                    observed_at_ns: metadata.observed_at_ns,
                    source: metadata.source,
                };
                let replace = metadata_by_client_order_id
                    .get(&metadata.client_order_id)
                    .map(|existing| metadata.observed_at_ns > existing.observed_at_ns)
                    .unwrap_or(true);
                if replace {
                    metadata_by_client_order_id.insert(metadata.client_order_id.clone(), metadata);
                }
            }
            SubmitReservationRecoveryEvent::Fill(fill) => {
                fills.push(BoltV3SubmitReservationFillEvidence {
                    client_order_id: fill.client_order_id,
                    submit_reservation_id: fill.submit_reservation_id,
                    trade_id: fill.trade_id,
                    instrument_id: fill.instrument_id,
                    side: match fill.side {
                        ReservationSide::Buy => SUBMIT_RESERVATION_METADATA_SIDE_BUY.to_string(),
                        ReservationSide::Sell => SUBMIT_RESERVATION_METADATA_SIDE_SELL.to_string(),
                    },
                    fill_quantity: fill.fill_quantity.normalize().to_string(),
                    observed_at_ns: fill.observed_at_ns,
                    reconciliation: fill.reconciliation,
                    source: fill.source,
                });
            }
        }
    }

    let metadata_by_client_order_id = metadata_by_client_order_id
        .into_iter()
        .map(|(client_order_id, metadata)| {
            let fill_trade_ids = fills
                .iter()
                .filter(|fill| {
                    fill.client_order_id == client_order_id
                        && fill.submit_reservation_id == metadata.submit_reservation_id
                })
                .map(|fill| fill.trade_id.clone())
                .collect();
            (
                client_order_id,
                BoltV3RecoveredSubmitReservationEvidence {
                    metadata,
                    fill_trade_ids,
                },
            )
        })
        .collect();

    Ok(BoltV3SubmitReservationRecoveryEvidence {
        metadata_by_client_order_id,
    })
}

fn open_regular_decision_evidence_file(path: &Path) -> std::io::Result<fs::File> {
    let pre_open_metadata = fs::symlink_metadata(path)?;
    validate_decision_evidence_regular_file(&pre_open_metadata)?;
    let file = open_decision_evidence_file_no_follow(path)?;
    let opened_metadata = file.metadata()?;
    validate_decision_evidence_regular_file(&opened_metadata)?;
    validate_same_decision_evidence_file(&pre_open_metadata, &opened_metadata)?;
    let post_open_metadata = fs::symlink_metadata(path)?;
    validate_decision_evidence_regular_file(&post_open_metadata)?;
    validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
    Ok(file)
}

pub(super) fn open_decision_evidence_append_file(path: &Path) -> std::io::Result<fs::File> {
    match fs::symlink_metadata(path) {
        Ok(pre_open_metadata) => {
            validate_decision_evidence_regular_file(&pre_open_metadata)?;
            let file = open_decision_evidence_append_existing_no_follow(path)?;
            let opened_metadata = file.metadata()?;
            validate_decision_evidence_regular_file(&opened_metadata)?;
            validate_same_decision_evidence_file(&pre_open_metadata, &opened_metadata)?;
            let post_open_metadata = fs::symlink_metadata(path)?;
            validate_decision_evidence_regular_file(&post_open_metadata)?;
            validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = open_decision_evidence_append_new_no_follow(path)?;
            let opened_metadata = file.metadata()?;
            validate_decision_evidence_regular_file(&opened_metadata)?;
            let post_open_metadata = fs::symlink_metadata(path)?;
            validate_decision_evidence_regular_file(&post_open_metadata)?;
            validate_same_decision_evidence_file(&opened_metadata, &post_open_metadata)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

fn open_decision_evidence_append_existing_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.append(true);
    configure_decision_evidence_append_options(&mut options);
    options.open(path)
}

fn open_decision_evidence_append_new_no_follow(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.append(true).create_new(true);
    configure_decision_evidence_append_options(&mut options);
    options.open(path)
}

#[cfg(unix)]
fn configure_decision_evidence_append_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .mode(PRIVATE_ARTIFACT_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn configure_decision_evidence_append_options(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn open_decision_evidence_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_decision_evidence_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new().read(true).open(path)
}

fn validate_decision_evidence_regular_file(metadata: &fs::Metadata) -> std::io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bolt-v3 decision evidence path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_same_decision_evidence_file(
    left: &fs::Metadata,
    right: &fs::Metadata,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if left.dev() != right.dev() || left.ino() != right.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid bolt-v3 decision evidence file identity during open",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_same_decision_evidence_file(
    _left: &fs::Metadata,
    _right: &fs::Metadata,
) -> std::io::Result<()> {
    Ok(())
}

fn settlement_fact_into_evidence(fact: facts::SettlementFact) -> BoltV3SettlementEvidence {
    BoltV3SettlementEvidence {
        strategy_id: fact.strategy_id,
        settlement_key: fact.settlement_key,
        market_id: fact.market_id,
        position_id: fact.position_id,
        instrument_id: fact.instrument_id,
        product_id: fact.product_id,
        outcome_side: match fact.outcome_side {
            facts::SettlementOutcomeSide::Up => BoltV3OutcomeSide::Up,
            facts::SettlementOutcomeSide::Down => BoltV3OutcomeSide::Down,
        },
        entry_order_side: match fact.entry_order_side {
            facts::SettlementOrderSide::Buy => SUBMIT_RESERVATION_METADATA_SIDE_BUY.to_string(),
            facts::SettlementOrderSide::Sell => SUBMIT_RESERVATION_METADATA_SIDE_SELL.to_string(),
        },
        quantity: fact.quantity.normalize().to_string(),
        entry_price: fact.entry_price.normalize().to_string(),
        family_key: fact.family_key,
        strike_price: fact.strike_price.normalize().to_string(),
        resolution_instrument_id: fact.resolution_instrument_id,
        resolution_ts_event_ns: fact.resolution_ts_event_ns,
        reference_close_price: fact.reference_close_price.normalize().to_string(),
        payout_per_share: fact.payout_per_share.normalize().to_string(),
        terminal_value: fact.terminal_value.normalize().to_string(),
        realized_pnl: fact.realized_pnl.normalize().to_string(),
        settlement_currency: fact.settlement_currency,
    }
}

fn booking_error_fact_into_evidence(
    fact: facts::SettlementBookingErrorFact,
) -> BoltV3SettlementBookingErrorEvidence {
    BoltV3SettlementBookingErrorEvidence {
        strategy_id: fact.strategy_id,
        settlement_key: fact.settlement_key,
        market_id: fact.market_id,
        position_id: fact.position_id,
        instrument_id: fact.instrument_id,
        resolution_instrument_id: fact.resolution_instrument_id,
        reason: match fact.reason {
            facts::SettlementBookingErrorReason::ResolutionFeedMissing => {
                BoltV3SettlementBookingErrorReason::ResolutionFeedMissing
            }
            facts::SettlementBookingErrorReason::SettlementAlreadyBooked => {
                BoltV3SettlementBookingErrorReason::SettlementAlreadyBooked
            }
            facts::SettlementBookingErrorReason::SettlementInputInvalid => {
                BoltV3SettlementBookingErrorReason::SettlementInputInvalid
            }
            facts::SettlementBookingErrorReason::SettlementBlocked => {
                BoltV3SettlementBookingErrorReason::SettlementBlocked
            }
        },
        detail: fact.detail,
        observed_at_ns: fact.observed_at_ns,
    }
}

fn lifecycle_fact_into_evidence(fact: facts::OrderLifecycleFact) -> BoltV3OrderLifecycleEvidence {
    BoltV3OrderLifecycleEvidence {
        strategy_id: fact.strategy_id,
        transition: match fact.transition {
            facts::OrderLifecycleTransition::BoundaryReclassification => {
                BoltV3OrderLifecycleTransition::BoundaryReclassification
            }
            facts::OrderLifecycleTransition::EntryFillMaterialized => {
                BoltV3OrderLifecycleTransition::EntryFillMaterialized
            }
            facts::OrderLifecycleTransition::EntryReconcilePending => {
                BoltV3OrderLifecycleTransition::EntryReconcilePending
            }
            facts::OrderLifecycleTransition::PositionTruthRematerialized => {
                BoltV3OrderLifecycleTransition::PositionTruthRematerialized
            }
            facts::OrderLifecycleTransition::PositionClosed => {
                BoltV3OrderLifecycleTransition::PositionClosed
            }
            facts::OrderLifecycleTransition::ResidualRemanaged => {
                BoltV3OrderLifecycleTransition::ResidualRemanaged
            }
            facts::OrderLifecycleTransition::RestartOpenOrderAdopted => {
                BoltV3OrderLifecycleTransition::RestartOpenOrderAdopted
            }
            facts::OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked => {
                BoltV3OrderLifecycleTransition::RestartOpenOrderRecoveryBlocked
            }
            facts::OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked => {
                BoltV3OrderLifecycleTransition::SettlementEvidenceRecoveryBlocked
            }
            facts::OrderLifecycleTransition::SettlementBookingTerminal => {
                BoltV3OrderLifecycleTransition::SettlementBookingTerminal
            }
            facts::OrderLifecycleTransition::OrderDenied => {
                BoltV3OrderLifecycleTransition::OrderDenied
            }
            facts::OrderLifecycleTransition::OrderRejected => {
                BoltV3OrderLifecycleTransition::OrderRejected
            }
            facts::OrderLifecycleTransition::OrderCanceled => {
                BoltV3OrderLifecycleTransition::OrderCanceled
            }
            facts::OrderLifecycleTransition::OrderExpired => {
                BoltV3OrderLifecycleTransition::OrderExpired
            }
            facts::OrderLifecycleTransition::OrderFilled => {
                BoltV3OrderLifecycleTransition::OrderFilled
            }
            facts::OrderLifecycleTransition::ReconcileQueryFailed => {
                BoltV3OrderLifecycleTransition::ReconcileQueryFailed
            }
        },
        outcome: match fact.outcome {
            facts::OrderLifecycleOutcome::PendingEntry => BoltV3OrderLifecycleOutcome::PendingEntry,
            facts::OrderLifecycleOutcome::Managed => BoltV3OrderLifecycleOutcome::Managed,
            facts::OrderLifecycleOutcome::ExitPending => BoltV3OrderLifecycleOutcome::ExitPending,
            facts::OrderLifecycleOutcome::EntryReconcilePending => {
                BoltV3OrderLifecycleOutcome::EntryReconcilePending
            }
            facts::OrderLifecycleOutcome::UnsupportedObserved => {
                BoltV3OrderLifecycleOutcome::UnsupportedObserved
            }
            facts::OrderLifecycleOutcome::BlindRecovery => {
                BoltV3OrderLifecycleOutcome::BlindRecovery
            }
            facts::OrderLifecycleOutcome::Flat => BoltV3OrderLifecycleOutcome::Flat,
        },
        source: fact.source,
        market_id: fact.market_id,
        instrument_id: fact.instrument_id,
        position_id: fact.position_id,
        client_order_id: fact.client_order_id,
        prior_client_order_id: fact.prior_client_order_id,
        raw_reason_text: fact.raw_reason_text,
        order_side: fact.order_side.map(|side| match side {
            facts::OrderLifecycleSide::Buy => SUBMIT_RESERVATION_METADATA_SIDE_BUY.to_string(),
            facts::OrderLifecycleSide::Sell => SUBMIT_RESERVATION_METADATA_SIDE_SELL.to_string(),
        }),
        filled_quantity: fact
            .filled_quantity
            .map(|value| value.normalize().to_string()),
        residual_quantity: fact
            .residual_quantity
            .map(|value| value.normalize().to_string()),
        ts_event_ns: fact.ts_event_ns,
    }
}

fn terminal_fact_into_evidence(
    fact: facts::TerminalSettlementFact,
) -> BoltV3TerminalSettlementEvidence {
    BoltV3TerminalSettlementEvidence {
        settlement_key: fact.settlement_key,
        booking_error: fact.booking_error.map(booking_error_fact_into_evidence),
        lifecycle: lifecycle_fact_into_evidence(fact.lifecycle),
    }
}

fn read_settlement_evidence_records(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    let mut records = Vec::new();
    for fact in decode::read_registered_facts(path.as_ref(), max_bytes)? {
        if let Some(facts::SettlementRecoveryEvent::Settlement(fact)) =
            facts::route_settlement_recovery(fact)?
        {
            records.push(settlement_fact_into_evidence(fact));
        }
    }
    Ok(records)
}

pub fn read_settlement_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    let records = read_settlement_evidence_records(path, max_bytes)?;
    fail_closed_on_duplicate_settlement_keys(&records)?;
    Ok(records)
}

pub fn read_settlement_booking_error_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3SettlementBookingErrorEvidence>> {
    let mut records = Vec::new();
    for fact in decode::read_registered_facts(path.as_ref(), max_bytes)? {
        if let Some(facts::BookingRecoveryEvent::BookingError(fact)) =
            facts::route_booking_recovery(fact)?
        {
            records.push(booking_error_fact_into_evidence(fact));
        }
    }
    Ok(records)
}

pub fn read_terminal_settlement_evidence(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<Vec<BoltV3TerminalSettlementEvidence>> {
    let mut records = Vec::new();
    for fact in decode::read_registered_facts(path.as_ref(), max_bytes)? {
        if let Some(facts::BookingRecoveryEvent::TerminalSettlement(fact)) =
            facts::route_booking_recovery(fact)?
        {
            records.push(terminal_fact_into_evidence(*fact));
        }
    }
    Ok(records)
}

pub fn read_terminal_settlement_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    Ok(read_terminal_settlement_evidence(path, max_bytes)?
        .into_iter()
        .filter_map(|evidence| {
            recovery_scope_settlement_keys
                .contains(&evidence.settlement_key)
                .then_some(evidence.settlement_key)
        })
        .collect())
}

pub fn read_settlement_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let settlements = read_settlement_evidence_records(path, max_bytes)?;
    let mut recovered = BTreeSet::new();
    for evidence in settlements {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !recovered.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
    }
    Ok(recovered)
}

pub fn read_settlement_booking_error_keys_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    let path = path.as_ref();
    let booking_errors = read_settlement_booking_error_evidence(path, max_bytes)?;
    let mut recovered = BTreeSet::new();
    for evidence in booking_errors {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !recovered.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
    }
    for evidence in read_terminal_settlement_evidence(path, max_bytes)? {
        if recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            recovered.insert(evidence.settlement_key);
        }
    }
    Ok(recovered)
}

pub fn read_settlement_evidence_for_recovery_scope(
    path: impl AsRef<Path>,
    max_bytes: u64,
    recovery_scope_settlement_keys: &BTreeSet<String>,
) -> Result<Vec<BoltV3SettlementEvidence>> {
    let settlements = read_settlement_evidence_records(path, max_bytes)?;
    let mut seen = BTreeSet::new();
    let mut recovered = Vec::new();
    for evidence in settlements {
        if !recovery_scope_settlement_keys.contains(&evidence.settlement_key) {
            continue;
        }
        if !seen.insert(evidence.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&evidence.settlement_key));
        }
        recovered.push(evidence);
    }
    Ok(recovered)
}
fn fail_closed_on_duplicate_settlement_keys(records: &[BoltV3SettlementEvidence]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for record in records {
        if !seen.insert(record.settlement_key.clone()) {
            return Err(duplicate_settlement_key_error(&record.settlement_key));
        }
    }
    Ok(())
}

fn duplicate_settlement_key_error(settlement_key: &str) -> anyhow::Error {
    anyhow!("duplicate settlement key `{settlement_key}` in bolt-v3 settlement evidence")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probability_evidence_uses_probability_value_bytes() {
        let probability = Probability::new(0.75).expect("probability should be valid");
        assert_eq!(probability_evidence(probability), "0.75");
    }
}
